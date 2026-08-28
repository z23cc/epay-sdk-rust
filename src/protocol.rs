//! Transport-free protocol context: signing, forms and callback verification.

use std::sync::Arc;

use crate::config::{Config, ConfigParts, env};
use crate::error::Result;
use crate::notify::{NotifyData, verify_notify};
use crate::params::Params;
use crate::payment::{FormPaymentRequest, auto_submit_form};
use crate::signer::Signer;
use crate::types::path;

/// Transport-free EPay protocol context.
///
/// This is the preferred Axum state for services that only build forms or
/// verify callbacks. It is cheap to clone and contains no HTTP client.
#[derive(Clone)]
pub struct ProtocolContext {
    inner: Arc<ProtocolInner>,
}

struct ProtocolInner {
    config: Config,
    signer: Signer,
}

impl std::fmt::Debug for ProtocolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtocolContext")
            .field("config", &self.inner.config)
            .finish()
    }
}

impl ProtocolContext {
    /// Build a context with default settings.
    pub fn new(pid: u64, key: impl Into<String>, api_base_url: impl Into<String>) -> Result<Self> {
        Self::builder(pid, key, api_base_url).build()
    }

    /// Start a fluent builder.
    pub fn builder(
        pid: u64,
        key: impl Into<String>,
        api_base_url: impl Into<String>,
    ) -> ProtocolContextBuilder {
        ProtocolContextBuilder::new(pid, key, api_base_url)
    }

    /// Build from `EPAY_*` environment variables.
    ///
    /// See [`ProtocolContextBuilder::from_env`] for the recognized variables.
    pub fn from_env() -> Result<Self> {
        ProtocolContextBuilder::from_env()?.build()
    }

    /// Validated configuration.
    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    /// MD5 signer bound to the merchant key.
    pub fn signer(&self) -> &Signer {
        &self.inner.signer
    }

    /// Sign a parameter map.
    pub fn sign(&self, params: &Params) -> String {
        self.inner.signer.sign(params)
    }

    /// Verify a signature against a parameter map in constant time.
    pub fn verify(&self, params: &Params, sign: &str) -> bool {
        self.inner.signer.verify(params, sign)
    }

    /// Verify signature, PID and structure of a notification.
    ///
    /// `params` should come from [`crate::parse_notify_params`] or another
    /// duplicate-aware parser.
    pub fn verify_notify(&self, params: &Params) -> Result<NotifyData> {
        verify_notify(&self.inner.signer, self.inner.config.pid(), params)
    }

    /// Signed `submit.php` URL for a browser redirect.
    pub fn build_form_payment_url(&self, req: &FormPaymentRequest) -> Result<String> {
        let signed = self.form_params(req)?;
        let mut endpoint = self.inner.config.endpoint(path::SUBMIT)?;
        endpoint.set_query(Some(&signed.encode()));
        Ok(endpoint.into())
    }

    /// Self-submitting HTML form that posts to `submit.php`.
    pub fn build_form_payment(&self, req: &FormPaymentRequest) -> Result<String> {
        let signed = self.form_params(req)?;
        let endpoint = self.inner.config.endpoint(path::SUBMIT)?;
        Ok(auto_submit_form(endpoint.as_str(), &signed))
    }

    pub(crate) fn base_params(&self) -> Params {
        let mut params = Params::new();
        params.insert("pid", self.inner.config.pid().to_string());
        params
    }

    fn form_params(&self, req: &FormPaymentRequest) -> Result<Params> {
        let callbacks = req.prepare(&self.inner.config)?;
        let mut params = self.base_params();
        params.insert("out_trade_no", req.out_trade_no.clone());
        params.insert("notify_url", callbacks.notify_url);
        params.insert_opt("return_url", callbacks.return_url);
        params.insert("name", req.name.clone());
        params.insert("money", req.money.to_epay_string());
        params.insert_opt(
            "type",
            req.pay_type.as_ref().map(|value| value.as_str().to_owned()),
        );
        params.insert_opt("param", req.param.clone());
        params.insert_opt("sitename", req.sitename.clone());
        Ok(self.inner.signer.sign_with_params(params))
    }
}

/// Builder for [`ProtocolContext`]; also embedded in
/// [`crate::ClientBuilder`].
#[derive(Clone, Debug)]
pub struct ProtocolContextBuilder {
    parts: ConfigParts,
}

impl ProtocolContextBuilder {
    /// Start with merchant id, key and gateway base URL.
    pub fn new(pid: u64, key: impl Into<String>, api_base_url: impl Into<String>) -> Self {
        Self {
            parts: ConfigParts::new(pid, key, api_base_url),
        }
    }

    /// Read protocol settings from the process environment.
    ///
    /// | Variable | Required | Meaning |
    /// |---|---|---|
    /// | `EPAY_PID` | yes | merchant id |
    /// | `EPAY_KEY` | yes | merchant key |
    /// | `EPAY_API_URL` | yes | gateway base URL |
    /// | `EPAY_NOTIFY_URL` | no | default notify URL |
    /// | `EPAY_RETURN_URL` | no | default return URL |
    /// | `EPAY_DEBUG` | no | `true`/`false` |
    /// | `EPAY_ALLOW_INSECURE_HTTP` | no | `true`/`false` (plain-HTTP gateway) |
    /// | `EPAY_ALLOW_INSECURE_CALLBACK_HTTP` | no | `true`/`false` (plain-HTTP callbacks) |
    ///
    /// Malformed typed values fail instead of falling back to defaults.
    pub fn from_env() -> Result<Self> {
        Self::from_env_with(&env::process)
    }

    pub(crate) fn from_env_with(lookup: env::Lookup<'_>) -> Result<Self> {
        let pid = env::required(lookup, "EPAY_PID", "EPAY_PID is required")?
            .trim()
            .parse::<u64>()
            .map_err(|_| crate::Error::INVALID_PID)?;
        let key = env::required(lookup, "EPAY_KEY", "EPAY_KEY is required")?;
        let api_url = env::required(lookup, "EPAY_API_URL", "EPAY_API_URL is required")?;
        let mut builder = Self::new(pid, key, api_url);
        if let Some(url) = env::optional(lookup, "EPAY_NOTIFY_URL") {
            builder = builder.notify_url(url);
        }
        if let Some(url) = env::optional(lookup, "EPAY_RETURN_URL") {
            builder = builder.return_url(url);
        }
        const DEBUG: &str = "EPAY_DEBUG must be a boolean";
        if let Some(value) = env::typed(lookup, "EPAY_DEBUG", DEBUG)? {
            builder = builder.debug(env::parse_bool(&value, DEBUG)?);
        }
        const INSECURE: &str = "EPAY_ALLOW_INSECURE_HTTP must be a boolean";
        if let Some(value) = env::typed(lookup, "EPAY_ALLOW_INSECURE_HTTP", INSECURE)? {
            builder = builder.allow_insecure_http(env::parse_bool(&value, INSECURE)?);
        }
        const INSECURE_CALLBACK: &str = "EPAY_ALLOW_INSECURE_CALLBACK_HTTP must be a boolean";
        if let Some(value) = env::typed(
            lookup,
            "EPAY_ALLOW_INSECURE_CALLBACK_HTTP",
            INSECURE_CALLBACK,
        )? {
            builder =
                builder.allow_insecure_callback_http(env::parse_bool(&value, INSECURE_CALLBACK)?);
        }
        Ok(builder)
    }

    /// Enable redacted debug logging through the `log` crate.
    pub fn debug(mut self, debug: bool) -> Self {
        self.parts.debug = debug;
        self
    }

    /// Default asynchronous notification URL.
    pub fn notify_url(mut self, url: impl Into<String>) -> Self {
        self.parts.notify_url = Some(url.into());
        self
    }

    /// Default browser return URL.
    pub fn return_url(mut self, url: impl Into<String>) -> Self {
        self.parts.return_url = Some(url.into());
        self
    }

    /// Allow a plain-HTTP gateway (legacy deployments only).
    pub fn allow_insecure_http(mut self, allow: bool) -> Self {
        self.parts.allow_insecure_http = allow;
        self
    }

    /// Allow plain-HTTP `notify_url` / `return_url` (local testing only).
    /// Applies to configured defaults and to request-level overrides.
    pub fn allow_insecure_callback_http(mut self, allow: bool) -> Self {
        self.parts.allow_insecure_callback_http = allow;
        self
    }

    /// Validate and build the context.
    pub fn build(self) -> Result<ProtocolContext> {
        let config = Config::from_parts(self.parts)?;
        let signer = Signer::from_key(config.merchant_key().clone())?;
        Ok(ProtocolContext {
            inner: Arc::new(ProtocolInner { config, signer }),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::{Error, Money, PayType};

    #[test]
    fn protocol_context_needs_no_transport() {
        let context = ProtocolContext::builder(1001, "k", "https://pay.example.com/epay")
            .notify_url("https://shop.example.com/notify")
            .build()
            .unwrap();
        let html = context
            .build_form_payment(
                &FormPaymentRequest::new(
                    "ORDER001",
                    "Product",
                    Money::from_yuan_str("1.00").unwrap(),
                )
                .pay_type(PayType::Alipay),
            )
            .unwrap();
        assert!(html.contains("https://pay.example.com/epay/submit.php"));
    }

    fn lookup(vars: &HashMap<&str, &str>) -> impl Fn(&str) -> Result<String, std::env::VarError> {
        move |name| {
            vars.get(name)
                .map(|value| (*value).to_owned())
                .ok_or(std::env::VarError::NotPresent)
        }
    }

    #[test]
    fn from_env_parses_protocol_variables() {
        let vars: HashMap<&str, &str> = [
            ("EPAY_PID", "1001"),
            ("EPAY_KEY", "k"),
            ("EPAY_API_URL", "http://legacy.example.com"),
            ("EPAY_NOTIFY_URL", "http://localhost:8080/notify"),
            ("EPAY_RETURN_URL", ""),
            ("EPAY_DEBUG", "true"),
            ("EPAY_ALLOW_INSECURE_HTTP", "1"),
            ("EPAY_ALLOW_INSECURE_CALLBACK_HTTP", "yes"),
        ]
        .into();
        let context = ProtocolContextBuilder::from_env_with(&lookup(&vars))
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(context.config().pid(), 1001);
        assert!(context.config().debug());
        assert!(context.config().allow_insecure_http());
        assert!(context.config().allow_insecure_callback_http());

        let mut strict = vars.clone();
        strict.remove("EPAY_ALLOW_INSECURE_CALLBACK_HTTP");
        assert!(
            ProtocolContextBuilder::from_env_with(&lookup(&strict))
                .unwrap()
                .build()
                .is_err(),
            "http notify_url needs the explicit opt-in"
        );
        assert!(context.config().notify_url().is_some());
        assert!(context.config().return_url().is_none());
    }

    #[test]
    fn from_env_fails_closed_on_bad_values() {
        let base: HashMap<&str, &str> = [
            ("EPAY_PID", "1001"),
            ("EPAY_KEY", "k"),
            ("EPAY_API_URL", "https://pay.example.com"),
        ]
        .into();
        for (name, value) in [
            ("EPAY_DEBUG", "maybe"),
            ("EPAY_DEBUG", ""),
            ("EPAY_PID", "x"),
        ] {
            let mut vars = base.clone();
            vars.insert(name, value);
            assert!(
                matches!(
                    ProtocolContextBuilder::from_env_with(&lookup(&vars)),
                    Err(Error::Config(_))
                ),
                "{name}={value}"
            );
        }
        let mut missing = base.clone();
        missing.remove("EPAY_KEY");
        assert!(ProtocolContextBuilder::from_env_with(&lookup(&missing)).is_err());
    }
}
