//! Refunds (`POST api.php?act=refund`).

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use crate::error::Result;
use crate::money::Money;
use crate::serde_util::default_on_null;
use crate::validation::{Identifier, exactly_one_identifier};

/// Refund request. Exactly one identifier must be set.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct RefundRequest {
    /// EPay order number.
    pub trade_no: Option<String>,
    /// Merchant order number.
    pub out_trade_no: Option<String>,
    /// Amount to refund; must not exceed the paid amount.
    pub money: Money,
}

impl RefundRequest {
    /// Refund by merchant order number.
    pub fn out_trade_no(no: impl Into<String>, money: Money) -> Self {
        Self {
            trade_no: None,
            out_trade_no: Some(no.into()),
            money,
        }
    }

    /// Refund by EPay order number.
    pub fn trade_no(no: impl Into<String>, money: Money) -> Self {
        Self {
            trade_no: Some(no.into()),
            out_trade_no: None,
            money,
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

/// Successful refund acknowledgement.
///
/// Success is decided by [`crate::Endpoint::Refund`] (`code` `0` per the
/// official implementation, `1` on some forks) before this is produced.
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct RefundResponse {
    /// Gateway message, e.g. `退款成功`.
    #[serde(default, deserialize_with = "default_on_null")]
    pub msg: String,
    /// Unmodelled members of the response object.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_message_and_tolerates_null() {
        let ok: RefundResponse = serde_json::from_str(r#"{"code":0,"msg":"退款成功"}"#).unwrap();
        assert_eq!(ok.msg, "退款成功");
        let bare: RefundResponse = serde_json::from_str(r#"{"code":1,"msg":null}"#).unwrap();
        assert_eq!(bare.msg, "");
    }

    #[test]
    fn requires_exactly_one_identifier() {
        let money = Money::from_yuan_str("1.00").unwrap();
        assert!(RefundRequest::out_trade_no("O1", money).validate().is_ok());
        assert!(RefundRequest::trade_no("T1", money).validate().is_ok());
        assert!(RefundRequest::trade_no("", money).validate().is_err());
    }
}
