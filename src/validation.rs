use crate::config::{parse_callback_url, require_https_callback};
use crate::error::{Error, Result};
use crate::types::{Device, PayType};

pub(crate) fn validate_out_trade_no(value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(Error::MISSING_OUT_TRADE_NO);
    }
    if value.len() > 64
        || value.trim() != value
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'|' | b'-'))
    {
        return Err(Error::INVALID_OUT_TRADE_NO);
    }
    Ok(())
}

pub(crate) fn validate_trade_no(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || value.trim() != value
        || !value.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return Err(Error::INVALID_TRADE_NO);
    }
    Ok(())
}

pub(crate) fn validate_name(value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::MISSING_NAME);
    }
    Ok(())
}

pub(crate) fn validate_pay_type(value: &PayType) -> Result<()> {
    if value.as_str().trim().is_empty() {
        return Err(Error::INVALID_PAY_TYPE);
    }
    Ok(())
}

pub(crate) fn validate_device(value: &Device) -> Result<()> {
    if value.as_str().trim().is_empty() {
        return Err(Error::INVALID_DEVICE);
    }
    Ok(())
}

/// Structural check only; the HTTPS policy is applied when the request is
/// resolved against a [`crate::Config`].
pub(crate) fn validate_notify_url(raw: &str) -> Result<()> {
    parse_callback_url(raw, Error::INVALID_NOTIFY_URL).map(|_| ())
}

/// Structural check only; see [`validate_notify_url`].
pub(crate) fn validate_return_url(raw: &str) -> Result<()> {
    parse_callback_url(raw, Error::INVALID_RETURN_URL).map(|_| ())
}

pub(crate) fn enforce_notify_url_policy(raw: &str, allow_http: bool) -> Result<()> {
    let url = parse_callback_url(raw, Error::INVALID_NOTIFY_URL)?;
    require_https_callback(&url, allow_http, Error::INSECURE_NOTIFY_URL)
}

pub(crate) fn enforce_return_url_policy(raw: &str, allow_http: bool) -> Result<()> {
    let url = parse_callback_url(raw, Error::INVALID_RETURN_URL)?;
    require_https_callback(&url, allow_http, Error::INSECURE_RETURN_URL)
}

pub(crate) fn exactly_one_identifier<'a>(
    trade_no: Option<&'a str>,
    out_trade_no: Option<&'a str>,
) -> Result<Identifier<'a>> {
    match (
        trade_no.filter(|value| !value.is_empty()),
        out_trade_no.filter(|value| !value.is_empty()),
    ) {
        (Some(trade_no), None) => {
            validate_trade_no(trade_no)?;
            Ok(Identifier::TradeNo(trade_no))
        }
        (None, Some(out_trade_no)) => {
            validate_out_trade_no(out_trade_no)?;
            Ok(Identifier::OutTradeNo(out_trade_no))
        }
        _ => Err(Error::MISSING_TRADE_NO),
    }
}

pub(crate) enum Identifier<'a> {
    TradeNo(&'a str),
    OutTradeNo(&'a str),
}

impl Identifier<'_> {
    /// Wire parameter name and value.
    pub(crate) fn as_param(&self) -> (&'static str, &str) {
        match self {
            Self::TradeNo(value) => ("trade_no", value),
            Self::OutTradeNo(value) => ("out_trade_no", value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_identifiers() {
        assert!(validate_out_trade_no("ORDER-1._|").is_ok());
        assert!(validate_out_trade_no(" bad").is_err());
        assert!(validate_out_trade_no("bad/slash").is_err());
        assert!(validate_trade_no("T20260101").is_ok());
        assert!(validate_trade_no("T-1").is_err());
    }

    #[test]
    fn requires_exactly_one_identifier() {
        assert!(exactly_one_identifier(None, Some("ORDER1")).is_ok());
        assert!(exactly_one_identifier(Some("T1"), Some("ORDER1")).is_err());
        assert!(exactly_one_identifier(None, None).is_err());
        let id = exactly_one_identifier(Some("T1"), None).unwrap();
        assert_eq!(id.as_param(), ("trade_no", "T1"));
    }

    #[test]
    fn callback_urls() {
        assert!(validate_notify_url("https://shop.example.com/notify?x=1").is_ok());
        assert!(matches!(
            validate_notify_url("javascript:alert(1)"),
            Err(Error::Param(m)) if m.starts_with("notify_url")
        ));
        assert!(matches!(
            validate_return_url("https://u:p@shop.example.com/r"),
            Err(Error::Param(m)) if m.starts_with("return_url")
        ));
        assert!(
            validate_notify_url("http://localhost/notify").is_ok(),
            "structural only"
        );
        assert!(matches!(
            enforce_notify_url_policy("http://localhost/notify", false),
            Err(Error::Param(m)) if m.starts_with("notify_url must use HTTPS")
        ));
        assert!(enforce_notify_url_policy("http://localhost/notify", true).is_ok());
        assert!(enforce_return_url_policy("https://shop.example.com/r", false).is_ok());
    }
}
