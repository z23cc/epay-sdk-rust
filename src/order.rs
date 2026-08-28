//! Order queries (`act=order`, `act=orders`) and order-number generation.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::Value;

use crate::endpoint::Endpoint;
use crate::error::{Error, Result};
use crate::money::Money;
use crate::serde_util::{default_on_null, flex_i32, flex_u64};
use crate::types::OrderStatus;
use crate::validation::{
    Identifier, exactly_one_identifier, validate_out_trade_no, validate_trade_no,
};

/// Query a single order by merchant id or EPay id.
///
/// Exactly one identifier must be set.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct OrderQueryRequest {
    /// EPay order number.
    pub trade_no: Option<String>,
    /// Merchant order number.
    pub out_trade_no: Option<String>,
}

impl OrderQueryRequest {
    /// Query by merchant order number.
    pub fn out_trade_no(no: impl Into<String>) -> Self {
        Self {
            trade_no: None,
            out_trade_no: Some(no.into()),
        }
    }

    /// Query by EPay order number.
    pub fn trade_no(no: impl Into<String>) -> Self {
        Self {
            trade_no: Some(no.into()),
            out_trade_no: None,
        }
    }

    /// Check that exactly one well-formed identifier is present.
    pub fn validate(&self) -> Result<()> {
        self.identifier().map(|_| ())
    }

    pub(crate) fn identifier(&self) -> Result<Identifier<'_>> {
        exactly_one_identifier(self.trade_no.as_deref(), self.out_trade_no.as_deref())
    }
}

/// Order row returned by `act=order` / `act=orders`.
///
/// Text columns that the gateway reports as `null` are exposed as empty
/// strings. Successful single-order queries are additionally checked to
/// match the requested identifier and the configured PID. Anything the
/// gateway returns beyond the modelled columns is kept in
/// [`OrderDetail::extra`].
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct OrderDetail {
    /// EPay order number.
    #[serde(default, deserialize_with = "default_on_null")]
    pub trade_no: String,
    /// Merchant order number.
    #[serde(default, deserialize_with = "default_on_null")]
    pub out_trade_no: String,
    /// Upstream channel transaction id.
    #[serde(default, deserialize_with = "default_on_null")]
    pub api_trade_no: String,
    /// Payment channel (`type`).
    #[serde(default, rename = "type", deserialize_with = "default_on_null")]
    pub pay_type: String,
    /// Merchant id.
    #[serde(default, deserialize_with = "flex_u64")]
    pub pid: u64,
    /// Creation time (`addtime`) as `YYYY-MM-DD HH:MM:SS` in the gateway's
    /// local time zone (Beijing time, UTC+8, on official deployments).
    #[serde(default, alias = "addtime", deserialize_with = "default_on_null")]
    pub add_time: String,
    /// Completion time (`endtime`), same format as [`OrderDetail::add_time`];
    /// empty while unpaid.
    #[serde(default, alias = "endtime", deserialize_with = "default_on_null")]
    pub end_time: String,
    /// Product name.
    #[serde(default, deserialize_with = "default_on_null")]
    pub name: String,
    /// Amount as a decimal string (`"10.00"`).
    #[serde(default, deserialize_with = "default_on_null")]
    pub money: String,
    /// Raw status integer; see [`OrderDetail::status`].
    #[serde(default, deserialize_with = "flex_i32")]
    pub status: i32,
    /// Opaque merchant data from the original request.
    #[serde(default, deserialize_with = "default_on_null")]
    pub param: String,
    /// Payer account, when the channel reports it.
    #[serde(default, deserialize_with = "default_on_null")]
    pub buyer: String,
    /// Fields returned by the gateway that this SDK does not model, e.g.
    /// fork-specific `bill_trade_no` or `pay_fee`. The client strips the
    /// envelope `code`; `msg` may be present.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl OrderDetail {
    /// Typed payment status.
    pub fn status(&self) -> OrderStatus {
        OrderStatus::from_i32(self.status)
    }

    /// Whether the order is paid.
    pub fn is_paid(&self) -> bool {
        self.status().is_paid()
    }

    /// Whether the order is unpaid (status `0`).
    pub fn is_unpaid(&self) -> bool {
        matches!(self.status(), OrderStatus::Unpaid)
    }

    /// Parse [`OrderDetail::money`] as a strict [`Money`].
    pub fn amount(&self) -> Result<Money> {
        Money::from_yuan_str(&self.money)
    }

    pub(crate) fn validate_success(&self, request: &OrderQueryRequest, pid: u64) -> Result<()> {
        let endpoint = Endpoint::QueryOrder;
        match request.identifier()? {
            Identifier::TradeNo(expected) if self.trade_no != expected => {
                return Err(Error::invalid_response_message(
                    endpoint,
                    "returned trade_no does not match the request",
                ));
            }
            Identifier::OutTradeNo(expected) if self.out_trade_no != expected => {
                return Err(Error::invalid_response_message(
                    endpoint,
                    "returned out_trade_no does not match the request",
                ));
            }
            _ => {}
        }
        self.validate_row(endpoint, pid)
    }

    fn validate_row(&self, endpoint: Endpoint, pid: u64) -> Result<()> {
        validate_trade_no(&self.trade_no).map_err(|_| {
            Error::invalid_response_message(endpoint, "returned trade_no is invalid")
        })?;
        validate_out_trade_no(&self.out_trade_no).map_err(|_| {
            Error::invalid_response_message(endpoint, "returned out_trade_no is invalid")
        })?;
        if self.pid != pid {
            return Err(Error::invalid_response_message(
                endpoint,
                "returned pid does not match the configured pid",
            ));
        }
        self.amount()
            .map_err(|_| Error::invalid_response_message(endpoint, "returned money is invalid"))?;
        Ok(())
    }
}

/// Batch order query result (`act=orders`).
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct OrderListResponse {
    /// Order count reported by the gateway: the total on some forks, the
    /// page size on others. Pagination helpers ignore it.
    #[serde(default, deserialize_with = "flex_i32")]
    pub count: i32,
    /// Orders on the requested page.
    #[serde(default, alias = "data", deserialize_with = "default_on_null")]
    pub orders: Vec<OrderDetail>,
    /// Unmodelled members of the response object.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl OrderListResponse {
    /// Whether another page probably exists: the page was full for the
    /// `limit` that was requested (clamped like the client clamps it).
    pub fn has_more(&self, limit: u32) -> bool {
        self.orders.len() as u64 >= u64::from(limit.clamp(1, 50))
    }

    /// Page number to request next, if [`OrderListResponse::has_more`].
    pub fn next_page(&self, page: u32, limit: u32) -> Option<u32> {
        self.has_more(limit).then(|| page.max(1).saturating_add(1))
    }

    pub(crate) fn validate_success(&self, pid: u64) -> Result<()> {
        let endpoint = Endpoint::QueryOrders;
        if self.count < 0 {
            return Err(Error::invalid_response_message(
                endpoint,
                "returned count is negative",
            ));
        }
        for order in &self.orders {
            order.validate_row(endpoint, pid)?;
        }
        Ok(())
    }
}

/// Generate a protocol-valid, probabilistically unique merchant order number.
///
/// The result is `prefix` + nanosecond timestamp + a 4-digit in-process
/// sequence. Multiple processes can still collide in theory; persist your
/// own identifiers when strict uniqueness is required.
pub fn generate_out_trade_no(prefix: &str) -> Result<String> {
    static SEQUENCE: AtomicU32 = AtomicU32::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::Param("system clock is before the Unix epoch"))?
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed) % 10_000;
    let value = format!("{prefix}{nanos}{sequence:04}");
    validate_out_trade_no(&value)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nullable_text_columns_from_php_database_rows() {
        let json = r#"{
            "code":1,
            "msg":"succ",
            "trade_no":"T202608280001",
            "out_trade_no":"ORDER001",
            "api_trade_no":"UPSTREAM001",
            "bill_trade_no":null,
            "type":"wxpay",
            "pid":"1001",
            "addtime":"2026-08-28 12:00:00",
            "endtime":null,
            "name":null,
            "money":"0.01",
            "status":"1",
            "param":null,
            "buyer":null
        }"#;
        let order: OrderDetail = serde_json::from_str(json).unwrap();
        assert_eq!(order.end_time, "");
        assert_eq!(order.name, "");
        assert_eq!(order.param, "");
        assert_eq!(order.buyer, "");
        assert!(order.is_paid());
        assert!(order.extra["bill_trade_no"].is_null());
        assert!(!order.extra.contains_key("addtime"), "aliases are modelled");
        assert_eq!(order.amount().unwrap().to_epay_string(), "0.01");
        assert!(
            order
                .validate_success(&OrderQueryRequest::out_trade_no("ORDER001"), 1001)
                .is_ok()
        );
        assert!(
            order
                .validate_success(&OrderQueryRequest::out_trade_no("OTHER"), 1001)
                .is_err()
        );
    }

    #[test]
    fn parses_list_with_string_count_data_alias_and_null_data() {
        let json = r#"{"code":"1","msg":"ok","count":"1","data":[{"trade_no":"T1","out_trade_no":"O1","status":"1","money":"10.00","pid":"1001"}]}"#;
        let list: OrderListResponse = serde_json::from_str(json).unwrap();
        assert_eq!(list.count, 1);
        assert_eq!(list.orders.len(), 1);
        assert!(list.orders[0].is_paid());
        assert_eq!(list.orders[0].pid, 1001);
        assert!(list.validate_success(1001).is_ok());
        assert!(list.validate_success(2002).is_err());

        let empty: OrderListResponse =
            serde_json::from_str(r#"{"code":1,"count":0,"data":null}"#).unwrap();
        assert!(empty.orders.is_empty());
        assert!(empty.validate_success(1001).is_ok());
        assert!(!empty.has_more(1));
        assert_eq!(empty.next_page(1, 1), None);
        assert!(list.has_more(1));
        assert_eq!(list.next_page(0, 1), Some(2));
        assert_eq!(list.next_page(7, 1), Some(8));
    }

    #[test]
    fn generated_order_number_is_validated() {
        let first = generate_out_trade_no("ORDER").unwrap();
        let second = generate_out_trade_no("ORDER").unwrap();
        assert_ne!(first, second);
        assert!(first.starts_with("ORDER"));
        assert!(validate_out_trade_no(&generate_out_trade_no("").unwrap()).is_ok());
        assert!(generate_out_trade_no("bad/").is_err());
    }
}
