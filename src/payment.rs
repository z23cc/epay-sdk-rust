//! Payment requests for `mapi.php` (API) and `submit.php` (form).

use std::collections::BTreeMap;
use std::fmt;
use std::net::IpAddr;

use serde::Deserialize;
use serde_json::Value;
use url::Url;

use crate::config::Config;
use crate::endpoint::Endpoint;
use crate::error::{Error, Result};
use crate::money::Money;
use crate::params::{Params, redact_url_query};
use crate::serde_util::default_on_null;
use crate::types::{Device, PayType};
use crate::validation::{
    enforce_notify_url_policy, enforce_return_url_policy, validate_device, validate_name,
    validate_notify_url, validate_out_trade_no, validate_param, validate_pay_type,
    validate_return_url, validate_sitename, validate_trade_no,
};

/// `Debug` helper: callback URLs with masked query, opaque data as a length.
fn debug_url(value: &Option<String>) -> Option<String> {
    value.as_deref().map(redact_url_query)
}

/// `Debug` helper for payment links, which may carry one-time tokens in the
/// path as well as the query: keep only scheme and host.
fn debug_payment_link(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    match Url::parse(value) {
        Ok(url) => format!(
            "{}://{}/<redacted>",
            url.scheme(),
            url.host_str().unwrap_or_default()
        ),
        Err(_) => format!("<invalid url: {} bytes>", value.len()),
    }
}

fn non_blank(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn debug_len(value: &Option<String>) -> Option<String> {
    value
        .as_ref()
        .map(|value| format!("<{} bytes>", value.len()))
}

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
/// the result, including the HTTPS policy. `notify_url` must be available
/// from one of the two sources.
pub(crate) fn resolve_callbacks<'a>(
    notify_url: Option<&'a str>,
    return_url: Option<&'a str>,
    config: &'a Config,
) -> Result<ResolvedCallbacks<'a>> {
    let allow_http = config.allow_insecure_callback_http();
    let notify_url = first_non_blank(notify_url, config.notify_url().map(url::Url::as_str))
        .ok_or(Error::MISSING_NOTIFY_URL)?;
    enforce_notify_url_policy(notify_url, allow_http)?;
    let return_url = first_non_blank(return_url, config.return_url().map(url::Url::as_str));
    if let Some(return_url) = return_url {
        enforce_return_url_policy(return_url, allow_http)?;
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
///
/// `Debug` masks callback query strings and shows `param` as a length only.
#[derive(Clone)]
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

impl fmt::Debug for PaymentRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaymentRequest")
            .field("pay_type", &self.pay_type)
            .field("out_trade_no", &self.out_trade_no)
            .field("notify_url", &debug_url(&self.notify_url))
            .field("return_url", &debug_url(&self.return_url))
            .field("name", &self.name)
            .field("money", &self.money)
            .field("client_ip", &self.client_ip)
            .field("device", &self.device)
            .field("param", &debug_len(&self.param))
            .finish()
    }
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
    /// Whether a `notify_url` is available, and whether callbacks may use
    /// plain HTTP, is checked when the request is sent against the client's
    /// configuration.
    pub fn validate(&self) -> Result<()> {
        validate_out_trade_no(&self.out_trade_no)?;
        validate_pay_type(&self.pay_type)?;
        validate_callback_overrides(self.notify_url.as_deref(), self.return_url.as_deref())?;
        validate_name(&self.name)?;
        if let Some(param) = &self.param {
            validate_param(param)?;
        }
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
/// `Debug` masks callback query strings and shows `param` as a length only.
#[derive(Clone)]
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

impl fmt::Debug for FormPaymentRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FormPaymentRequest")
            .field("pay_type", &self.pay_type)
            .field("out_trade_no", &self.out_trade_no)
            .field("notify_url", &debug_url(&self.notify_url))
            .field("return_url", &debug_url(&self.return_url))
            .field("name", &self.name)
            .field("money", &self.money)
            .field("param", &debug_len(&self.param))
            .field("sitename", &self.sitename)
            .finish()
    }
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
        if let Some(param) = &self.param {
            validate_param(param)?;
        }
        if let Some(sitename) = &self.sitename {
            validate_sitename(sitename)?;
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
/// `Debug` reduces `pay_url` / `url_scheme` to scheme and host, shows
/// `qr_code` (arbitrary text) as a length and `extra` as its keys, so
/// one-time payment links do not land in logs even when the token is in the
/// path.
#[derive(Clone, Deserialize)]
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

/// One way to send the payer to the gateway, taken from [`PaymentResponse`].
///
/// The SDK does not allow-list schemes or hosts: before redirecting a browser
/// to a [`PaymentDestination::PayUrl`] or handing an
/// [`PaymentDestination::UrlScheme`] to an app, apply your own policy (e.g.
/// HTTPS only, known `alipays://` / `weixin://` schemes). Values are
/// trimmed. `Debug` shows only scheme and host.
#[derive(Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PaymentDestination<'a> {
    /// Cashier / redirect URL (`payurl`).
    PayUrl(&'a str),
    /// QR-code payload (`qrcode`); usually a URL, not guaranteed.
    QrCode(&'a str),
    /// App URL scheme (`urlscheme`).
    UrlScheme(&'a str),
}

impl PaymentDestination<'_> {
    /// Raw wire value.
    pub fn as_str(&self) -> &str {
        match self {
            Self::PayUrl(value) | Self::QrCode(value) | Self::UrlScheme(value) => value,
        }
    }

    /// Parse the value as a URL so scheme and host can be checked.
    pub fn parse_url(&self) -> Result<Url> {
        Url::parse(self.as_str()).map_err(|_| {
            Error::invalid_response_message(
                Endpoint::CreatePayment,
                "payment destination is not a valid URL",
            )
        })
    }
}

impl fmt::Debug for PaymentDestination<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (name, value) = match self {
            Self::PayUrl(value) => ("PayUrl", *value),
            Self::QrCode(value) => ("QrCode", *value),
            Self::UrlScheme(value) => ("UrlScheme", *value),
        };
        f.debug_tuple(name)
            .field(&debug_payment_link(value))
            .finish()
    }
}

impl fmt::Debug for PaymentResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let extra_keys: Vec<&str> = self.extra.keys().map(String::as_str).collect();
        f.debug_struct("PaymentResponse")
            .field("trade_no", &self.trade_no)
            .field("pay_url", &debug_payment_link(&self.pay_url))
            .field("qr_code", &format_args!("<{} bytes>", self.qr_code.len()))
            .field("url_scheme", &debug_payment_link(&self.url_scheme))
            .field("extra_keys", &extra_keys)
            .finish()
    }
}

impl PaymentResponse {
    /// All non-blank destinations (trimmed) in the order `pay_url`,
    /// `qr_code`, `url_scheme` — the same test [`Client`] applies when it
    /// validates the response.
    ///
    /// [`Client`]: crate::Client
    pub fn destinations(&self) -> Vec<PaymentDestination<'_>> {
        let mut out = Vec::with_capacity(3);
        if let Some(value) = non_blank(&self.pay_url) {
            out.push(PaymentDestination::PayUrl(value));
        }
        if let Some(value) = non_blank(&self.qr_code) {
            out.push(PaymentDestination::QrCode(value));
        }
        if let Some(value) = non_blank(&self.url_scheme) {
            out.push(PaymentDestination::UrlScheme(value));
        }
        out
    }

    /// First available destination (see [`PaymentResponse::destinations`]).
    pub fn destination(&self) -> Option<PaymentDestination<'_>> {
        self.destinations().into_iter().next()
    }

    pub(crate) fn validate_success(&self) -> Result<()> {
        validate_trade_no(&self.trade_no).map_err(|_| {
            Error::invalid_response_message(
                Endpoint::CreatePayment,
                "successful response has a missing or invalid trade_no",
            )
        })?;
        if self.destinations().is_empty() {
            return Err(Error::invalid_response_message(
                Endpoint::CreatePayment,
                "successful response has no payment destination",
            ));
        }
        Ok(())
    }
}

/// A signed, validated, size-bounded `submit.php` payload for the
/// application to render.
///
/// [`crate::ProtocolContext::build_form_payment`] wraps this in a page with
/// an inline `<script>` that submits automatically, which a strict CSP
/// blocks. With `PreparedForm` you can render the inputs yourself and submit
/// via a nonce'd script, an external script or a plain button. The parts are
/// read-only so the signature stays valid; [`PreparedForm::into_parts`]
/// hands them over for advanced use. `Debug` masks the signature.
#[derive(Clone, Debug)]
pub struct PreparedForm {
    action: Url,
    params: Params,
}

impl PreparedForm {
    pub(crate) fn new(action: Url, params: Params) -> Self {
        Self { action, params }
    }

    /// `submit.php` endpoint to POST to (or redirect to with
    /// [`PreparedForm::redirect_url`]).
    pub fn action(&self) -> &Url {
        &self.action
    }

    /// Signed parameters, including `sign` and `sign_type`.
    pub fn params(&self) -> &Params {
        &self.params
    }

    /// Take ownership of the parts. Anything you change afterwards is no
    /// longer covered by the signature or the size limits.
    pub fn into_parts(self) -> (Url, Params) {
        (self.action, self.params)
    }

    /// HTML-escaped `<input type="hidden">` elements for every parameter.
    pub fn hidden_inputs(&self) -> String {
        let mut fields = String::new();
        for (key, value) in self.params.iter() {
            fields.push_str(&format!(
                r#"<input type="hidden" name="{}" value="{}">"#,
                html_escape(key),
                html_escape(value)
            ));
        }
        fields
    }

    /// `action` with the signed parameters as its query string, for a GET
    /// redirect.
    pub fn redirect_url(&self) -> Url {
        let mut url = self.action.clone();
        url.set_query(Some(&self.params.encode()));
        url
    }

    /// Complete page that submits itself with an inline script.
    pub fn auto_submit_html(&self) -> String {
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
            action = html_escape(self.action.as_str()),
            fields = self.hidden_inputs()
        )
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
    fn request_callback_overrides_follow_the_https_policy() {
        let req = FormPaymentRequest::new("ORDER001", "P", money())
            .notify_url("http://localhost:8080/notify");
        assert!(req.validate().is_ok(), "structure is fine");
        assert!(matches!(
            req.prepare(&config(None)),
            Err(Error::Param(m)) if m.starts_with("notify_url must use HTTPS")
        ));
        let mut parts = crate::config::ConfigParts::new(1001, "k", "https://pay.example.com");
        parts.allow_insecure_callback_http = true;
        let permissive = Config::from_parts(parts).unwrap();
        assert_eq!(
            req.prepare(&permissive).unwrap().notify_url,
            "http://localhost:8080/notify"
        );
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
    fn debug_masks_callback_queries_and_opaque_data() {
        let request = PaymentRequest::new(PayType::Alipay, "ORDER001", "P", money())
            .notify_url("https://shop.example.com/notify?token=callback-secret")
            .param("user=42&coupon=SECRET")
            .client_ip("::1");
        let text = format!("{request:?}");
        assert!(!text.contains("callback-secret"), "{text}");
        assert!(!text.contains("SECRET"), "{text}");
        assert!(
            text.contains("ORDER001") && text.contains("bytes"),
            "{text}"
        );

        let form = FormPaymentRequest::new("ORDER001", "P", money())
            .return_url("https://shop.example.com/r?sid=abc");
        assert!(!format!("{form:?}").contains("sid=abc"));

        let response: PaymentResponse = serde_json::from_str(
            r#"{"trade_no":"T1","payurl":"https://pay.example.com/cashier/onetime-path?token=onetime","qrcode":"weixin://wxpay/bizpayurl?pr=onetime3","urlscheme":"alipays://platformapi/startapp?appId=1&code=onetime2","fork_field":"fork-secret"}"#,
        )
        .unwrap();
        let text = format!("{response:?}");
        assert!(!text.contains("onetime"), "{text}");
        assert!(!text.contains("fork-secret"), "{text}");
        assert!(
            text.contains("https://pay.example.com/<redacted>"),
            "{text}"
        );
        assert!(text.contains("fork_field") && text.contains("T1"), "{text}");

        let unvalidated = PaymentRequest::new(PayType::Alipay, "O", "P", money())
            .notify_url("not a url?token=secret");
        assert!(!format!("{unvalidated:?}").contains("secret"));
    }

    #[test]
    fn destinations_follow_wire_order_and_parse() {
        let response: PaymentResponse = serde_json::from_str(
            r#"{"trade_no":"T1","qrcode":"weixin://wxpay/bizpayurl?pr=abc","urlscheme":"alipays://x?y=1"}"#,
        )
        .unwrap();
        let all = response.destinations();
        assert_eq!(all.len(), 2);
        assert!(matches!(all[0], PaymentDestination::QrCode(_)));
        assert!(matches!(all[1], PaymentDestination::UrlScheme(_)));
        let first = response.destination().unwrap();
        assert_eq!(first.parse_url().unwrap().scheme(), "weixin");
        assert!(!format!("{first:?}").contains("pr=abc"));

        let empty: PaymentResponse = serde_json::from_str(r#"{"trade_no":"T1"}"#).unwrap();
        assert!(empty.destination().is_none());

        let blank_first: PaymentResponse =
            serde_json::from_str(r#"{"trade_no":"T1","payurl":"  ","qrcode":" weixin://x "}"#)
                .unwrap();
        assert!(blank_first.validate_success().is_ok());
        assert_eq!(
            blank_first.destination(),
            Some(PaymentDestination::QrCode("weixin://x")),
            "blank pay_url is skipped and values are trimmed"
        );
        assert!(PaymentDestination::QrCode("not a url").parse_url().is_err());
    }

    #[test]
    fn prepared_form_renders_inputs_redirect_and_page() {
        let mut params = Params::new();
        params.insert("pid", "1001");
        params.insert("name", "A&B <x>");
        params.insert("sign", "deadbeef");
        let form = PreparedForm::new(
            Url::parse("https://pay.example.com/submit.php").unwrap(),
            params,
        );
        assert_eq!(form.action().as_str(), "https://pay.example.com/submit.php");
        assert_eq!(form.params().get("pid"), Some("1001"));
        let inputs = form.hidden_inputs();
        assert!(
            inputs.contains(r#"name="name" value="A&amp;B &lt;x&gt;""#),
            "{inputs}"
        );
        assert!(!inputs.contains("<script"));
        assert_eq!(
            form.redirect_url().as_str(),
            "https://pay.example.com/submit.php?name=A%26B+%3Cx%3E&pid=1001&sign=deadbeef"
        );
        let page = form.auto_submit_html();
        assert!(page.contains("payForm") && page.contains(&inputs));
        assert!(
            !format!("{form:?}").contains("deadbeef"),
            "sign is redacted"
        );
        let (action, params) = form.into_parts();
        assert_eq!(action.path(), "/submit.php");
        assert_eq!(params.len(), 3);
    }

    #[test]
    fn successful_response_needs_a_well_formed_trade_no() {
        for json in [
            r#"{"trade_no":"","payurl":"https://p"}"#,
            r#"{"trade_no":"T-1","payurl":"https://p"}"#,
            r#"{"trade_no":" T1","payurl":"https://p"}"#,
        ] {
            let response: PaymentResponse = serde_json::from_str(json).unwrap();
            assert!(response.validate_success().is_err(), "{json}");
        }
    }

    #[test]
    fn field_limits_are_enforced() {
        assert!(matches!(
            PaymentRequest::new(PayType::Alipay, "O", "line1\nline2", money())
                .client_ip("::1")
                .validate(),
            Err(Error::Param(m)) if m.contains("control characters")
        ));
        assert!(
            FormPaymentRequest::new("O", "P", money())
                .sitename("shop\u{0}")
                .validate()
                .is_err()
        );
        let too_long_name = "n".repeat(crate::limits::MAX_NAME_BYTES + 1);
        assert!(matches!(
            PaymentRequest::new(PayType::Alipay, "O", too_long_name, money())
                .client_ip("::1")
                .validate(),
            Err(Error::Param(m)) if m.starts_with("name exceeds")
        ));
        assert!(
            FormPaymentRequest::new("O", "P", money())
                .param("p".repeat(crate::limits::MAX_PARAM_BYTES + 1))
                .validate()
                .is_err()
        );
        assert!(
            FormPaymentRequest::new("O", "P", money())
                .sitename("s".repeat(crate::limits::MAX_SITENAME_BYTES + 1))
                .validate()
                .is_err()
        );
        assert!(
            PaymentRequest::new("bad type", "O", "P", money())
                .client_ip("::1")
                .validate()
                .is_err()
        );
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
