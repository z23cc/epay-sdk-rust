//! Merchant profile (`act=query`) and settlement records (`act=settle`).

use std::collections::BTreeMap;
use std::str::FromStr;

use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::Value;

use crate::endpoint::Endpoint;
use crate::error::{Error, Result};
use crate::serde_util::{default_on_null, flex_i32, flex_u64};

/// Merchant profile from `act=query`.
///
/// The gateway echoes the merchant `key` in this response; [`crate::Client`]
/// strips it, and `Debug` masks any `key` / `sign` left in
/// [`MerchantInfo::extra`] when the struct is deserialized directly. The
/// settlement `account` is shown as a length only.
#[derive(Clone, Deserialize)]
#[non_exhaustive]
pub struct MerchantInfo {
    /// Merchant id.
    #[serde(default, deserialize_with = "flex_u64")]
    pub pid: u64,
    /// `1` when the merchant account is active.
    #[serde(default, deserialize_with = "flex_i32")]
    pub active: i32,
    /// Balance as a decimal string.
    #[serde(default, deserialize_with = "default_on_null")]
    pub money: String,
    /// Settlement channel type (`type`).
    #[serde(default, rename = "type", deserialize_with = "flex_i32")]
    pub settle_type: i32,
    /// Settlement account.
    #[serde(default, deserialize_with = "default_on_null")]
    pub account: String,
    /// Settlement account holder name.
    #[serde(default, deserialize_with = "default_on_null")]
    pub username: String,
    /// Unmodelled members of the response object.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl std::fmt::Debug for MerchantInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted = Value::String("***".to_owned());
        let extra: BTreeMap<&str, &Value> = self
            .extra
            .iter()
            .map(|(key, value)| {
                let shown = if matches!(key.as_str(), "key" | "sign") {
                    &redacted
                } else {
                    value
                };
                (key.as_str(), shown)
            })
            .collect();
        f.debug_struct("MerchantInfo")
            .field("pid", &self.pid)
            .field("active", &self.active)
            .field("money", &self.money)
            .field("settle_type", &self.settle_type)
            .field("account", &format_args!("<{} bytes>", self.account.len()))
            .field("username", &self.username)
            .field("extra", &extra)
            .finish()
    }
}

impl MerchantInfo {
    /// Whether the merchant account is active.
    pub fn is_active(&self) -> bool {
        self.active == 1
    }

    /// Parse [`MerchantInfo::money`] as a decimal balance.
    pub fn balance(&self) -> Result<Decimal> {
        Decimal::from_str(self.money.trim()).map_err(|_| {
            Error::invalid_response_message(Endpoint::QueryMerchant, "returned balance is invalid")
        })
    }

    pub(crate) fn validate_success(&self, expected_pid: u64) -> Result<()> {
        if self.pid != expected_pid {
            return Err(Error::invalid_response_message(
                Endpoint::QueryMerchant,
                "returned pid does not match the configured pid",
            ));
        }
        if self.balance()? < Decimal::ZERO {
            return Err(Error::invalid_response_message(
                Endpoint::QueryMerchant,
                "returned balance is negative",
            ));
        }
        Ok(())
    }
}

/// Settlement record from `act=settle`. `Debug` shows the destination
/// `account` as a length only.
#[derive(Clone, Deserialize)]
#[non_exhaustive]
pub struct Settlement {
    /// Settlement id.
    #[serde(default, deserialize_with = "flex_i32")]
    pub id: i32,
    /// Merchant id.
    #[serde(default, deserialize_with = "flex_u64")]
    pub uid: u64,
    /// Settled amount as a decimal string.
    #[serde(default, deserialize_with = "default_on_null")]
    pub money: String,
    /// Destination account.
    #[serde(default, deserialize_with = "default_on_null")]
    pub account: String,
    /// Fork-specific settlement status.
    #[serde(default, deserialize_with = "flex_i32")]
    pub status: i32,
    /// Creation time (`addtime`), `YYYY-MM-DD HH:MM:SS` in the gateway's
    /// local time zone.
    #[serde(default, alias = "addtime", deserialize_with = "default_on_null")]
    pub add_time: String,
    /// Unmodelled members of the settlement row.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl std::fmt::Debug for Settlement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let extra_keys: Vec<&str> = self.extra.keys().map(String::as_str).collect();
        f.debug_struct("Settlement")
            .field("id", &self.id)
            .field("uid", &self.uid)
            .field("money", &self.money)
            .field("account", &format_args!("<{} bytes>", self.account.len()))
            .field("status", &self.status)
            .field("add_time", &self.add_time)
            .field("extra_keys", &extra_keys)
            .finish()
    }
}

/// Settlement list from `act=settle`.
///
/// Rows are bound to the configured PID: a non-zero `uid` that differs from
/// it is rejected. A missing or `null` `uid` (some forks omit it) is
/// tolerated.
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct SettlementListResponse {
    /// Total number of records reported by the gateway.
    #[serde(default, deserialize_with = "flex_i32")]
    pub count: i32,
    /// Settlement records.
    #[serde(
        default,
        alias = "orders",
        alias = "data",
        deserialize_with = "default_on_null"
    )]
    pub settlements: Vec<Settlement>,
    /// Unmodelled members of the response object.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SettlementListResponse {
    pub(crate) fn validate_success(&self, expected_pid: u64) -> Result<()> {
        let endpoint = Endpoint::QuerySettlements;
        if self.count < 0 {
            return Err(Error::invalid_response_message(
                endpoint,
                "returned count is negative",
            ));
        }
        for settlement in &self.settlements {
            if settlement.uid != 0 && settlement.uid != expected_pid {
                return Err(Error::invalid_response_message(
                    endpoint,
                    "a settlement uid does not match the configured pid",
                ));
            }
            let money = Decimal::from_str(settlement.money.trim()).map_err(|_| {
                Error::invalid_response_message(endpoint, "a settlement has invalid money")
            })?;
            if money < Decimal::ZERO {
                return Err(Error::invalid_response_message(
                    endpoint,
                    "a settlement has negative money",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merchant_ignores_echoed_key_and_accepts_nulls() {
        let info: MerchantInfo = serde_json::from_str(
            r#"{"code":1,"pid":1001,"key":"super-secret","active":"1","money":"12.00","type":1,"account":null,"username":null}"#,
        )
        .unwrap();
        let s = format!("{info:?}");
        assert!(!s.contains("super-secret"), "{s}");
        assert!(s.contains("***"), "{s}");
        assert_eq!(
            info.extra["key"], "super-secret",
            "direct deserialization keeps it"
        );
        assert_eq!(info.settle_type, 1);
        assert!(info.is_active());
        assert_eq!(info.account, "");
        assert_eq!(info.balance().unwrap(), Decimal::from(12));
        assert!(info.validate_success(1001).is_ok());
        assert!(info.validate_success(2002).is_err());
    }

    #[test]
    fn settlements_accept_nulls_and_reject_negative_money() {
        let list: SettlementListResponse = serde_json::from_str(
            r#"{"code":1,"count":"1","data":[{"id":1,"uid":1001,"money":"1.00","account":null,"status":1,"addtime":null}]}"#,
        )
        .unwrap();
        assert_eq!(list.settlements.len(), 1);
        assert_eq!(list.settlements[0].account, "");
        assert!(list.validate_success(1001).is_ok());

        let pii: SettlementListResponse = serde_json::from_str(
            r#"{"code":1,"count":1,"data":[{"id":1,"uid":1001,"money":"1.00","account":"6222000011112222"}]}"#,
        )
        .unwrap();
        assert!(!format!("{pii:?}").contains("6222000011112222"));
        assert!(list.validate_success(2002).is_err());

        let empty: SettlementListResponse =
            serde_json::from_str(r#"{"code":1,"count":0,"data":null}"#).unwrap();
        assert!(empty.settlements.is_empty());
        assert!(empty.validate_success(1001).is_ok());

        let without_uid: SettlementListResponse = serde_json::from_str(
            r#"{"code":1,"count":1,"data":[{"id":1,"uid":null,"money":"1.00"}]}"#,
        )
        .unwrap();
        assert!(without_uid.validate_success(1001).is_ok());

        let negative: SettlementListResponse = serde_json::from_str(
            r#"{"code":1,"count":1,"orders":[{"id":1,"uid":1001,"money":"-1.00"}]}"#,
        )
        .unwrap();
        assert!(negative.validate_success(1001).is_err());
    }
}
