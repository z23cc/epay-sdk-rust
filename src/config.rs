//! Validated protocol configuration and HTTP client settings.

use std::fmt;
use std::time::Duration;

use url::Url;

use crate::error::{Error, Result};
use crate::secret::MerchantKey;

/// Validated protocol configuration shared by [`crate::ProtocolContext`] and
/// [`crate::Client`].
///
/// Contains merchant identity, gateway location and callback defaults only.
/// HTTP-specific settings live in [`HttpConfig`].
#[derive(Clone)]
pub struct Config {
    pid: u64,
    key: MerchantKey,
    api_base_url: Url,
    debug: bool,
    notify_url: Option<Url>,
    return_url: Option<Url>,
    allow_insecure_http: bool,
    allow_insecure_callback_http: bool,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("pid", &self.pid)
            .field("key", &"***")
            .field("api_base_url", &self.api_base_url)
            .field("debug", &self.debug)
            .field("notify_url", &self.notify_url.as_ref().map(redacted_url))
            .field("return_url", &self.return_url.as_ref().map(redacted_url))
            .field("allow_insecure_http", &self.allow_insecure_http)
            .field(
                "allow_insecure_callback_http",
                &self.allow_insecure_callback_http,
            )
            .finish()
    }
}

/// Unvalidated inputs collected by the builders.
#[derive(Clone)]
pub(crate) struct ConfigParts {
    pub pid: u64,
    pub key: MerchantKey,
    pub api_base_url: String,
    pub debug: bool,
    pub notify_url: Option<String>,
    pub return_url: Option<String>,
    pub allow_insecure_http: bool,
    pub allow_insecure_callback_http: bool,
}

impl ConfigParts {
    pub(crate) fn new(pid: u64, key: impl Into<String>, api_base_url: impl Into<String>) -> Self {
        Self {
            pid,
            key: MerchantKey::new(key.into()),
            api_base_url: api_base_url.into(),
            debug: false,
            notify_url: None,
            return_url: None,
            allow_insecure_http: false,
            allow_insecure_callback_http: false,
        }
    }
}

impl fmt::Debug for ConfigParts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfigParts")
            .field("pid", &self.pid)
            .field("key", &"***")
            .field("api_base_url", &self.api_base_url)
            .field("debug", &self.debug)
            .field("notify_url_set", &self.notify_url.is_some())
            .field("return_url_set", &self.return_url.is_some())
            .field("allow_insecure_http", &self.allow_insecure_http)
            .field(
                "allow_insecure_callback_http",
                &self.allow_insecure_callback_http,
            )
            .finish()
    }
}

impl Config {
    /// Build a strict HTTPS configuration without callback defaults.
    pub fn new(pid: u64, key: impl Into<String>, api_base_url: impl Into<String>) -> Result<Self> {
        Self::from_parts(ConfigParts::new(pid, key, api_base_url))
    }

    pub(crate) fn from_parts(parts: ConfigParts) -> Result<Self> {
        if parts.pid == 0 {
            return Err(Error::INVALID_PID);
        }
        if parts.key.is_empty() {
            return Err(Error::INVALID_KEY);
        }

        let mut api_base_url =
            Url::parse(parts.api_base_url.trim()).map_err(|_| Error::INVALID_API_URL)?;
        validate_api_base_url(&api_base_url, parts.allow_insecure_http)?;
        let path = api_base_url.path().trim_end_matches('/');
        let normalized_path = if path.is_empty() {
            "/".to_owned()
        } else {
            format!("{path}/")
        };
        api_base_url.set_path(&normalized_path);

        let allow_callback_http = parts.allow_insecure_callback_http;
        let notify_url = parts
            .notify_url
            .as_deref()
            .map(|url| {
                let url = parse_callback_url(url, Error::INVALID_NOTIFY_URL)?;
                require_https_callback(&url, allow_callback_http, Error::INSECURE_NOTIFY_URL)?;
                Ok::<_, Error>(url)
            })
            .transpose()?;
        let return_url = parts
            .return_url
            .as_deref()
            .map(|url| {
                let url = parse_callback_url(url, Error::INVALID_RETURN_URL)?;
                require_https_callback(&url, allow_callback_http, Error::INSECURE_RETURN_URL)?;
                Ok::<_, Error>(url)
            })
            .transpose()?;

        Ok(Self {
            pid: parts.pid,
            key: parts.key,
            api_base_url,
            debug: parts.debug,
            notify_url,
            return_url,
            allow_insecure_http: parts.allow_insecure_http,
            allow_insecure_callback_http: allow_callback_http,
        })
    }

    /// Merchant id.
    pub fn pid(&self) -> u64 {
        self.pid
    }

    /// Normalized gateway base URL; always ends with `/`.
    pub fn api_base_url(&self) -> &Url {
        &self.api_base_url
    }

    /// Whether redacted request/response debug logging is enabled.
    pub fn debug(&self) -> bool {
        self.debug
    }

    /// Default asynchronous notification URL.
    pub fn notify_url(&self) -> Option<&Url> {
        self.notify_url.as_ref()
    }

    /// Default browser return URL.
    pub fn return_url(&self) -> Option<&Url> {
        self.return_url.as_ref()
    }

    /// Whether a plain-HTTP gateway was explicitly allowed.
    pub fn allow_insecure_http(&self) -> bool {
        self.allow_insecure_http
    }

    /// Whether plain-HTTP `notify_url` / `return_url` values were explicitly
    /// allowed (local testing only).
    pub fn allow_insecure_callback_http(&self) -> bool {
        self.allow_insecure_callback_http
    }

    pub(crate) fn key(&self) -> &str {
        self.key.as_str()
    }

    pub(crate) fn merchant_key(&self) -> &MerchantKey {
        &self.key
    }

    pub(crate) fn endpoint(&self, path: &str) -> Result<Url> {
        self.api_base_url
            .join(path.trim_start_matches('/'))
            .map_err(|_| Error::INVALID_API_URL)
    }
}

/// Retry policy for read-only (GET) gateway calls.
///
/// Applies only to GET endpoints and only to errors where
/// [`crate::Error::is_retryable`] is true; `mapi.php` payments and refunds
/// are never retried automatically. Back-off doubles from `initial_backoff`
/// up to `max_backoff` (no jitter).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    max_retries: u32,
    initial_backoff: Duration,
    max_backoff: Duration,
}

impl RetryPolicy {
    /// Never retry (the default).
    pub const NONE: Self = Self {
        max_retries: 0,
        initial_backoff: Duration::ZERO,
        max_backoff: Duration::ZERO,
    };

    /// Retry up to `max_retries` times with 200 ms → 2 s exponential back-off.
    pub fn new(max_retries: u32) -> Self {
        Self {
            max_retries,
            initial_backoff: Duration::from_millis(200),
            max_backoff: Duration::from_secs(2),
        }
    }

    /// Sensible default for read-only calls: two retries.
    pub fn reads() -> Self {
        Self::new(2)
    }

    /// Override the back-off window.
    pub fn with_backoff(mut self, initial: Duration, max: Duration) -> Self {
        self.initial_backoff = initial;
        self.max_backoff = max.max(initial);
        self
    }

    /// Maximum number of retries after the first attempt.
    pub fn max_retries(&self) -> u32 {
        self.max_retries
    }

    /// Back-off before the first retry.
    pub fn initial_backoff(&self) -> Duration {
        self.initial_backoff
    }

    /// Upper bound for the back-off.
    pub fn max_backoff(&self) -> Duration {
        self.max_backoff
    }

    /// Delay before retry number `retry` (1-based).
    pub fn backoff(&self, retry: u32) -> Duration {
        let exponent = retry.saturating_sub(1).min(31);
        self.initial_backoff
            .checked_mul(1 << exponent)
            .map_or(self.max_backoff, |delay| delay.min(self.max_backoff))
    }

    /// Whether another attempt should follow `retries_so_far` failed retries.
    pub(crate) fn should_retry(&self, retries_so_far: u32, error: &Error) -> bool {
        retries_so_far < self.max_retries && error.is_retryable()
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::NONE
    }
}

/// HTTP settings used by [`crate::Client`] only.
///
/// `timeout` is **per attempt**: with a [`RetryPolicy`] each retry gets the
/// full timeout again and back-off delays are not counted, so the worst
/// case for a GET is `(retries + 1) * timeout + total back-off`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpConfig {
    timeout: Duration,
    connect_timeout: Option<Duration>,
    max_response_body_bytes: usize,
    retry: RetryPolicy,
}

impl HttpConfig {
    /// Default request timeout (30 seconds).
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
    /// Default response body limit (1 MiB).
    pub const DEFAULT_MAX_RESPONSE_BODY_BYTES: usize = 1024 * 1024;

    /// Validate a timeout and response limit; both must be non-zero. Retries
    /// are disabled; see [`HttpConfig::with_retry`].
    pub fn new(timeout: Duration, max_response_body_bytes: usize) -> Result<Self> {
        if timeout.is_zero() {
            return Err(Error::Config("timeout must be greater than 0"));
        }
        if max_response_body_bytes == 0 {
            return Err(Error::Config(
                "max response body size must be greater than 0",
            ));
        }
        Ok(Self {
            timeout,
            connect_timeout: None,
            max_response_body_bytes,
            retry: RetryPolicy::NONE,
        })
    }

    /// Retry policy for GET endpoints.
    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Separate TCP/TLS connect timeout for the SDK-created reqwest client;
    /// `None` leaves only the per-attempt timeout. Zero is rejected rather
    /// than silently disabling the limit. Not applied to caller-supplied
    /// `reqwest::Client`s or custom transports.
    pub fn with_connect_timeout(mut self, connect_timeout: Option<Duration>) -> Result<Self> {
        if connect_timeout.is_some_and(|value| value.is_zero()) {
            return Err(Error::Config("connect timeout must be greater than 0"));
        }
        self.connect_timeout = connect_timeout;
        Ok(self)
    }

    /// Per-attempt request timeout applied by the built-in transport.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Connect timeout for the built-in transport, if set.
    pub fn connect_timeout(&self) -> Option<Duration> {
        self.connect_timeout
    }

    /// Maximum accepted response body size in bytes.
    pub fn max_response_body_bytes(&self) -> usize {
        self.max_response_body_bytes
    }

    /// Retry policy for GET endpoints.
    pub fn retry(&self) -> &RetryPolicy {
        &self.retry
    }
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            timeout: Self::DEFAULT_TIMEOUT,
            connect_timeout: None,
            max_response_body_bytes: Self::DEFAULT_MAX_RESPONSE_BODY_BYTES,
            retry: RetryPolicy::NONE,
        }
    }
}

/// Reject plain-HTTP callbacks unless explicitly allowed. Callbacks carry
/// the signed payment result; over HTTP an on-path attacker can read and
/// replay it.
pub(crate) fn require_https_callback(url: &Url, allow_http: bool, error: Error) -> Result<()> {
    if url.scheme() == "https" || allow_http {
        Ok(())
    } else {
        Err(error)
    }
}

pub(crate) fn parse_callback_url(raw: &str, error: Error) -> Result<Url> {
    let raw = raw.trim();
    let Ok(url) = Url::parse(raw) else {
        return Err(error);
    };
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(error);
    }
    Ok(url)
}

fn validate_api_base_url(url: &Url, allow_insecure_http: bool) -> Result<()> {
    match url.scheme() {
        "https" => {}
        "http" if allow_insecure_http => {}
        "http" => return Err(Error::INSECURE_API_URL),
        _ => return Err(Error::INVALID_API_URL),
    }
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::INVALID_API_URL);
    }
    Ok(())
}

fn redacted_url(url: &Url) -> String {
    let mut redacted = url.clone();
    if redacted.query().is_some() {
        redacted.set_query(Some("***"));
    }
    redacted.to_string()
}

/// Environment-variable access shared by the builders.
///
/// Every reader takes a lookup function so parsing can be tested without
/// touching process state.
pub(crate) mod env {
    use std::env::VarError;

    use crate::error::{Error, Result};

    /// Variable lookup with `std::env::var` semantics.
    pub(crate) type Lookup<'a> = &'a dyn Fn(&str) -> std::result::Result<String, VarError>;

    /// Lookup backed by the process environment.
    pub(crate) fn process(name: &str) -> std::result::Result<String, VarError> {
        std::env::var(name)
    }

    /// A variable that must be present and non-empty.
    pub(crate) fn required(
        lookup: Lookup<'_>,
        name: &str,
        message: &'static str,
    ) -> Result<String> {
        lookup(name)
            .ok()
            .filter(|value| !value.is_empty())
            .ok_or(Error::Config(message))
    }

    /// A free-form variable where absent and empty both mean "unset".
    pub(crate) fn optional(lookup: Lookup<'_>, name: &str) -> Option<String> {
        lookup(name).ok().filter(|value| !value.is_empty())
    }

    /// A typed variable: absent means "unset", empty or non-UTF-8 is an error.
    pub(crate) fn typed(
        lookup: Lookup<'_>,
        name: &str,
        message: &'static str,
    ) -> Result<Option<String>> {
        match lookup(name) {
            Ok(value) if value.is_empty() => Err(Error::Config(message)),
            Ok(value) => Ok(Some(value)),
            Err(VarError::NotPresent) => Ok(None),
            Err(VarError::NotUnicode(_)) => Err(Error::Config(message)),
        }
    }

    /// Parse `1/true/yes` and `0/false/no` (case-insensitive).
    pub(crate) fn parse_bool(value: &str, message: &'static str) -> Result<bool> {
        match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" => Ok(true),
            "0" | "false" | "no" => Ok(false),
            _ => Err(Error::Config(message)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_and_normalizes_base_url() {
        assert!(Config::new(0, "k", "https://pay.example.com").is_err());
        assert!(Config::new(1, "", "https://pay.example.com").is_err());
        assert!(Config::new(1, "k", "http://pay.example.com").is_err());
        assert!(Config::new(1, "k", "https://u:p@pay.example.com").is_err());
        assert!(Config::new(1, "k", "https://pay.example.com?key=x").is_err());

        let c = Config::new(1, "k", "https://pay.example.com/epay//").unwrap();
        assert_eq!(c.api_base_url().as_str(), "https://pay.example.com/epay/");
        assert_eq!(
            c.endpoint("/api.php").unwrap().as_str(),
            "https://pay.example.com/epay/api.php"
        );
    }

    #[test]
    fn http_requires_explicit_opt_in() {
        let mut parts = ConfigParts::new(1, "k", "http://pay.example.com");
        assert!(Config::from_parts(parts.clone()).is_err());
        parts.allow_insecure_http = true;
        let c = Config::from_parts(parts).unwrap();
        assert_eq!(c.api_base_url().scheme(), "http");
    }

    #[test]
    fn callback_urls_are_validated_and_debug_redacted() {
        let mut parts = ConfigParts::new(1001, "super-secret", "https://pay.example.com");
        parts.notify_url = Some("http://localhost:8080/notify?token=secret".into());
        assert!(matches!(
            Config::from_parts(parts.clone()),
            Err(Error::Param(m)) if m.starts_with("notify_url must use HTTPS")
        ));
        parts.allow_insecure_callback_http = true;
        let c = Config::from_parts(parts.clone()).unwrap();
        assert!(c.allow_insecure_callback_http());
        let text = format!("{c:?}");
        assert!(!text.contains("super-secret"));
        assert!(!text.contains("token=secret"));
        assert!(text.contains("***"));

        parts.return_url = Some("ftp://x".into());
        assert!(matches!(
            Config::from_parts(parts),
            Err(Error::Param(m)) if m.starts_with("return_url")
        ));
    }

    #[test]
    fn http_config_rejects_zero_values() {
        assert!(HttpConfig::new(Duration::ZERO, 1).is_err());
        assert!(HttpConfig::new(Duration::from_secs(1), 0).is_err());
        let http = HttpConfig::default();
        assert_eq!(http.timeout(), HttpConfig::DEFAULT_TIMEOUT);
        assert_eq!(
            http.max_response_body_bytes(),
            HttpConfig::DEFAULT_MAX_RESPONSE_BODY_BYTES
        );
        assert_eq!(http.retry(), &RetryPolicy::NONE);
        assert_eq!(http.connect_timeout(), None);
        let http = http
            .with_connect_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        assert_eq!(http.connect_timeout(), Some(Duration::from_secs(5)));
        assert!(matches!(
            http.with_connect_timeout(Some(Duration::ZERO)),
            Err(Error::Config(m)) if m.starts_with("connect timeout")
        ));
    }

    #[test]
    fn retry_policy_backoff_doubles_and_caps() {
        let policy = RetryPolicy::reads();
        assert_eq!(policy.max_retries(), 2);
        assert_eq!(policy.backoff(1), Duration::from_millis(200));
        assert_eq!(policy.backoff(2), Duration::from_millis(400));
        assert_eq!(policy.backoff(10), Duration::from_secs(2));
        assert_eq!(policy.backoff(64), Duration::from_secs(2));

        let custom = RetryPolicy::new(1).with_backoff(Duration::from_secs(5), Duration::ZERO);
        assert_eq!(
            custom.max_backoff(),
            Duration::from_secs(5),
            "max >= initial"
        );

        let timeout = Error::transport(crate::TransportErrorKind::Timeout, "t");
        assert!(policy.should_retry(0, &timeout));
        assert!(policy.should_retry(1, &timeout));
        assert!(!policy.should_retry(2, &timeout));
        assert!(!RetryPolicy::NONE.should_retry(0, &timeout));
        assert!(!policy.should_retry(0, &Error::VerifyFailed));
    }

    #[test]
    fn env_helpers() {
        use std::collections::HashMap;
        let vars: HashMap<&str, &str> = [("A", "x"), ("EMPTY", ""), ("B", " Yes ")].into();
        let lookup = |name: &str| {
            vars.get(name)
                .map(|value| (*value).to_owned())
                .ok_or(std::env::VarError::NotPresent)
        };
        assert_eq!(env::required(&lookup, "A", "m").unwrap(), "x");
        assert!(env::required(&lookup, "EMPTY", "m").is_err());
        assert!(env::required(&lookup, "MISSING", "m").is_err());
        assert_eq!(env::optional(&lookup, "EMPTY"), None);
        assert_eq!(env::typed(&lookup, "MISSING", "m").unwrap(), None);
        assert!(env::typed(&lookup, "EMPTY", "m").is_err());
        assert!(env::parse_bool(&env::typed(&lookup, "B", "m").unwrap().unwrap(), "m").unwrap());
        assert!(env::parse_bool("maybe", "m").is_err());
    }
}
