//! Payment requests for `mapi.php` (API) and `submit.php` (form).

use std::collections::BTreeMap;
use std::net::IpAddr;

use serde::Deserialize;
use serde_json::Value;

use crate::config::Config;
use crate::endpoint::Endpoint;
use crate::error::{Error, Result};
use crate::money::Money;
use crate::serde_util::default_on_null;
use crate::types::{Device, PayType};
use crate::validation::{
    validate_device, validate_name, validate_notify_url, validate_out_trade_no, validate_pay_type,
    validate_return_url,
};

/// Request-level URL if it is non-blank, else the client default if non-blank;
/// a blank override must not shadow a configured default.
pub(crate) fn first_non_blank<'a>(
    primary: Option<&'a str>,
    fallback: Option<&'a str>,
) -> Option<&'a str> {
    let non_blank = |s: Option<&'a str>| s.map(str::trim).filter(|s| !s.is_empty());
    non_blank(primary).or_else(|| non_blank(fallback))
}

/// Callback URLs after merging request overrides with configured defaults.
pub(crate) struct ResolvedCallbacks<'a> {
    pub notify_url: &'a str,
    pub return_url: Option<&'a str>,
}

/// Merge request-level callbacks with the configured defaults and validate
/// the result. `notify_url` must be available from one of the two sources.
pub(crate) fn resolve_callbacks<'a>(
    notify_url: Option<&'a str>,
    return_url: Option<&'a str>,
    config: &'a Config,
) -> Result<ResolvedCallbacks<'a>> {
    let notify_url = first_non_blank(notify_url, config.notify_url().map(url::Url::as_str))
        .ok_or(Error::MISSING_NOTIFY_URL)?;
    validate_notify_url(notify_url)?;
    let return_url = first_non_blank(return_url, config.return_url().map(url::Url::as_str));
    if let Some(return_url) = return_url {
        validate_return_url(return_url)?;
    }
    Ok(ResolvedCallbacks {
        notify_url,
        return_url,
    })
}

/// Validate request-level callback overrides without consulting a config.
fn validate_callback_overrides(notify_url: Option<&str>, return_url: Option<&str>) -> Result<()> {
    if let Some(url) = first_non_blank(notify_url, None) {
        validate_notify_url(url)?;
    }
    if let Some(url) = first_non_blank(return_url, None) {
        validate_return_url(url)?;
    }
    Ok(())
}

/// API (`mapi.php`) payment request.
///
/// `client_ip` is **required** by EPay for this endpoint: the server answers
/// `用户IP地址(clientip)不能为空` / `用户IP地址不合法` otherwise, so
/// [`PaymentRequest::validate`] enforces a well-formed IPv4/IPv6 address.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct PaymentRequest {
    /// Payment channel.
    pub pay_type: PayType,
    /// Merchant order number.
    pub out_trade_no: String,
    /// Override of the configured notify URL.
    pub notify_url: Option<String>,
    /// Override of the configured return URL.
    pub return_url: Option<String>,
    /// Product name shown to the payer.
    pub name: String,
    /// Amount to charge.
    pub money: Money,
    /// Payer IP address as seen by your server.
    pub client_ip: Option<String>,
    /// Device hint.
    pub device: Option<Device>,
    /// Opaque merchant data echoed back in the notification.
    pub param: Option<String>,
}

impl PaymentRequest {
    /// Start a request; call [`PaymentRequest::client_ip`] before sending.
    pub fn new(
        pay_type: impl Into<PayType>,
        out_trade_no: impl Into<String>,
        name: impl Into<String>,
        money: Money,
    ) -> Self {
        Self {
            pay_type: pay_type.into(),
            out_trade_no: out_trade_no.into(),
            notify_url: None,
            return_url: None,
            name: name.into(),
            money,
            client_ip: None,
            device: None,
            param: None,
        }
    }

    /// Override the configured notify URL.
    pub fn notify_url(mut self, url: impl Into<String>) -> Self {
        self.notify_url = Some(url.into());
        self
    }

    /// Override the configured return URL.
    pub fn return_url(mut self, url: impl Into<String>) -> Self {
        self.return_url = Some(url.into());
        self
    }

    /// Payer IP as seen by your server. Required by `mapi.php`; must be a
    /// valid IPv4/IPv6 address.
    pub fn client_ip(mut self, ip: impl Into<String>) -> Self {
        self.client_ip = Some(ip.into());
        self
    }

    /// Typed variant of [`PaymentRequest::client_ip`]. Behind a reverse proxy
    /// derive the address explicitly (with the `axum` feature,
    /// [`crate::axum::payer_ip`]); a bare `SocketAddr::ip()` is the proxy.
    pub fn client_ip_addr(self, ip: IpAddr) -> Self {
        self.client_ip(ip.to_string())
    }

    /// Device hint.
    pub fn device(mut self, device: impl Into<Device>) -> Self {
        self.device = Some(device.into());
        self
    }

    /// Opaque merchant data echoed back in the notification.
    pub fn param(mut self, param: impl Into<String>) -> Self {
        self.param = Some(param.into());
        self
    }

    /// Validate every field that does not depend on client configuration.
    ///
    /// Whether a `notify_url` is available is checked when the request is
    /// sent, because the client may supply a default.
    pub fn validate(&self) -> Result<()> {
        validate_out_trade_no(&self.out_trade_no)?;
        validate_pay_type(&self.pay_type)?;
        validate_callback_overrides(self.notify_url.as_deref(), self.return_url.as_deref())?;
        validate_name(&self.name)?;
        match self.client_ip.as_deref().map(str::trim) {
            None | Some("") => return Err(Error::MISSING_CLIENT_IP),
            Some(ip) if ip.parse::<IpAddr>().is_err() => return Err(Error::INVALID_CLIENT_IP),
            Some(_) => {}
        }
        if let Some(device) = &self.device {
            validate_device(device)?;
        }
        Ok(())
    }

    pub(crate) fn prepare<'a>(&'a self, config: &'a Config) -> Result<ResolvedCallbacks<'a>> {
        self.validate()?;
        resolve_callbacks(
            self.notify_url.as_deref(),
            self.return_url.as_deref(),
            config,
        )
    }
}

/// Page-jump (`submit.php`) payment request.
///
/// `pay_type` is optional: omit it to land on the EPay cashier.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct FormPaymentRequest {
    /// Payment channel; `None` shows the cashier page.
    pub pay_type: Option<PayType>,
    /// Merchant order number.
    pub out_trade_no: String,
    /// Override of the configured notify URL.
    pub notify_url: Option<String>,
    /// Override of the configured return URL.
    pub return_url: Option<String>,
    /// Product name shown to the payer.
    pub name: String,
    /// Amount to charge.
    pub money: Money,
    /// Opaque merchant data echoed back in the notification.
    pub param: Option<String>,
    /// Site name shown on the cashier page.
    pub sitename: Option<String>,
}

impl FormPaymentRequest {
    /// Start a request for the cashier page.
    pub fn new(out_trade_no: impl Into<String>, name: impl Into<String>, money: Money) -> Self {
        Self {
            pay_type: None,
            out_trade_no: out_trade_no.into(),
            notify_url: None,
            return_url: None,
            name: name.into(),
            money,
            param: None,
            sitename: None,
        }
    }

    /// Pre-select a payment channel.
    pub fn pay_type(mut self, pay_type: impl Into<PayType>) -> Self {
        self.pay_type = Some(pay_type.into());
        self
    }

    /// Override the configured notify URL.
    pub fn notify_url(mut self, url: impl Into<String>) -> Self {
        self.notify_url = Some(url.into());
        self
    }

    /// Override the configured return URL.
    pub fn return_url(mut self, url: impl Into<String>) -> Self {
        self.return_url = Some(url.into());
        self
    }

    /// Opaque merchant data echoed back in the notification.
    pub fn param(mut self, param: impl Into<String>) -> Self {
        self.param = Some(param.into());
        self
    }

    /// Site name shown on the cashier page.
    pub fn sitename(mut self, name: impl Into<String>) -> Self {
        self.sitename = Some(name.into());
        self
    }

    /// Validate every field that does not depend on client configuration.
    pub fn validate(&self) -> Result<()> {
        validate_out_trade_no(&self.out_trade_no)?;
        validate_callback_overrides(self.notify_url.as_deref(), self.return_url.as_deref())?;
        validate_name(&self.name)?;
        if let Some(pay_type) = &self.pay_type {
            validate_pay_type(pay_type)?;
        }
        Ok(())
    }

    pub(crate) fn prepare<'a>(&'a self, config: &'a Config) -> Result<ResolvedCallbacks<'a>> {
        self.validate()?;
        resolve_callbacks(
            self.notify_url.as_deref(),
            self.return_url.as_deref(),
            config,
        )
    }
}

/// Successful `mapi.php` response.
///
/// The gateway returns whichever destination fits the channel/device; at
/// least one of `pay_url`, `qr_code` and `url_scheme` is guaranteed non-empty.
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct PaymentResponse {
    /// EPay order number.
    #[serde(default, deserialize_with = "default_on_null")]
    pub trade_no: String,
    /// Cashier/redirect URL.
    #[serde(default, alias = "payurl", deserialize_with = "default_on_null")]
    pub pay_url: String,
    /// QR-code payload.
    #[serde(default, alias = "qrcode", deserialize_with = "default_on_null")]
    pub qr_code: String,
    /// App URL scheme (`alipays://…`, `weixin://…`).
    #[serde(default, alias = "urlscheme", deserialize_with = "default_on_null")]
    pub url_scheme: String,
    /// Unmodelled members of the response object.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl PaymentResponse {
    pub(crate) fn validate_success(&self) -> Result<()> {
        if self.trade_no.trim().is_empty() {
            return Err(Error::invalid_response_message(
                Endpoint::CreatePayment,
                "successful response is missing trade_no",
            ));
        }
        if self.pay_url.trim().is_empty()
            && self.qr_code.trim().is_empty()
            && self.url_scheme.trim().is_empty()
        {
            return Err(Error::invalid_response_message(
                Endpoint::CreatePayment,
                "successful response has no payment destination",
            ));
        }
        Ok(())
    }
}

pub(crate) fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

pub(crate) fn auto_submit_form(action: &str, params: &crate::params::Params) -> String {
    let mut fields = String::new();
    for (k, v) in params.iter() {
        fields.push_str(&format!(
            r#"<input type="hidden" name="{}" value="{}">"#,
            html_escape(k),
            html_escape(v)
        ));
    }
    format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>正在跳转到支付页面...</title>
</head>
<body>
    <form id="payForm" method="POST" action="{action}">
        {fields}
    </form>
    <script>document.getElementById('payForm').submit();</script>
</body>
</html>"#,
        action = html_escape(action),
        fields = fields
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn money() -> Money {
        Money::from_yuan_str("10.00").unwrap()
    }

    fn config(notify: Option<&str>) -> Config {
        let mut parts = crate::config::ConfigParts::new(1001, "k", "https://pay.example.com");
        parts.notify_url = notify.map(str::to_owned);
        Config::from_parts(parts).unwrap()
    }

    #[test]
    fn validate_payment_request() {
        let req = PaymentRequest::new(PayType::Alipay, "ORDER001", "Test Product", money())
            .notify_url("https://example.com/notify")
            .client_ip("127.0.0.1");
        assert!(req.validate().is_ok());
        assert!(req.prepare(&config(None)).is_ok());

        let bad = PaymentRequest::new(PayType::Alipay, "", "n", money()).client_ip("127.0.0.1");
        assert!(bad.validate().is_err());
        let bad_url = PaymentRequest::new(PayType::Alipay, "O", "n", money())
            .client_ip("127.0.0.1")
            .return_url("javascript:alert(1)");
        assert!(matches!(bad_url.validate(), Err(Error::Param(m)) if m.starts_with("return_url")));
    }

    #[test]
    fn validate_requires_valid_clientip() {
        let base = || PaymentRequest::new(PayType::Alipay, "ORDER001", "P", money());
        assert!(matches!(
            base().validate(),
            Err(Error::Param(m)) if m.starts_with("clientip is required")
        ));
        assert!(matches!(
            base().client_ip(" ").validate(),
            Err(Error::Param(m)) if m.starts_with("clientip is required")
        ));
        assert!(matches!(
            base().client_ip("not-an-ip").validate(),
            Err(Error::Param(m)) if m.starts_with("clientip must be")
        ));
        assert!(base().client_ip("203.0.113.9").validate().is_ok());
        assert!(base().client_ip("::1").validate().is_ok());
        let typed = base().client_ip_addr("203.0.113.9".parse().unwrap());
        assert_eq!(typed.client_ip.as_deref(), Some("203.0.113.9"));
    }

    #[test]
    fn blank_notify_url_uses_default_or_fails() {
        let req = FormPaymentRequest::new("ORDER001", "P", money()).notify_url("   ");
        assert!(req.validate().is_ok());
        let with_default = config(Some("https://example.com/notify"));
        let resolved = req.prepare(&with_default).unwrap();
        assert_eq!(resolved.notify_url, "https://example.com/notify");
        assert!(matches!(
            req.prepare(&config(None)),
            Err(Error::Param(m)) if m.starts_with("notify_url")
        ));
        assert_eq!(first_non_blank(Some(" "), Some(" x ")), Some("x"));
        assert_eq!(first_non_blank(Some("a"), Some("b")), Some("a"));
        assert_eq!(first_non_blank(None, None), None);
    }

    #[test]
    fn request_callbacks_override_defaults() {
        let req = FormPaymentRequest::new("ORDER001", "P", money())
            .notify_url("https://override.example.com/n")
            .return_url("https://override.example.com/r");
        let with_default = config(Some("https://example.com/notify"));
        let resolved = req.prepare(&with_default).unwrap();
        assert_eq!(resolved.notify_url, "https://override.example.com/n");
        assert_eq!(resolved.return_url, Some("https://override.example.com/r"));
    }

    #[test]
    fn html_escape_quotes() {
        assert_eq!(html_escape(r#"a&b<"c">"#), "a&amp;b&lt;&quot;c&quot;&gt;");
    }

    #[test]
    fn payment_response_aliases_and_nulls() {
        let json = r#"{"code":"1","msg":"ok","trade_no":"T1","payurl":"https://p","qrcode":"https://q","urlscheme":"alipays://x"}"#;
        let resp: PaymentResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.pay_url, "https://p");
        assert_eq!(resp.qr_code, "https://q");
        assert_eq!(resp.url_scheme, "alipays://x");

        let nulls = r#"{"code":1,"msg":null,"trade_no":"T1","payurl":"https://p","qrcode":null,"urlscheme":null}"#;
        let resp: PaymentResponse = serde_json::from_str(nulls).unwrap();
        assert!(resp.validate_success().is_ok());
        assert_eq!(resp.qr_code, "");
        assert!(resp.extra["msg"].is_null());
        assert!(!resp.extra.contains_key("payurl"));
    }
}
