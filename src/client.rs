//! Async HTTP client for `mapi.php` and `api.php`.

use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;

use crate::config::{Config, HttpConfig, RetryPolicy, env};
use crate::endpoint::{Endpoint, Method};
use crate::error::{Error, Result, redact_secrets};
use crate::merchant::{MerchantInfo, SettlementListResponse};
use crate::money::Money;
use crate::notify::NotifyData;
use crate::observe::instrument;
use crate::order::{OrderDetail, OrderListResponse, OrderQueryRequest};
use crate::params::Params;
use crate::payment::{FormPaymentRequest, PaymentRequest, PaymentResponse, PreparedForm};
use crate::protocol::{ProtocolContext, ProtocolContextBuilder};
use crate::refund::{RefundRequest, RefundResponse};
use crate::signer::Signer;
use crate::transport::{Transport, TransportOptions};
use crate::validation::validate_encoded_size;

/// Per-call overrides for [`Client`] methods ending in `_with`.
///
/// Unset fields fall back to the client's [`HttpConfig`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct CallOptions {
    /// Per-attempt timeout for this call (each retry gets it again).
    pub timeout: Option<Duration>,
    /// Retry policy for this call (GET endpoints only).
    pub retry: Option<RetryPolicy>,
}

impl CallOptions {
    /// No overrides.
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the per-attempt timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Override the retry policy.
    pub fn retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = Some(retry);
        self
    }
}

/// Async EPay HTTP client. Cheap to clone and suitable for Axum state.
///
/// Every gateway call validates the request locally, checks the response
/// envelope (`code`/`msg`) and then validates the success payload against the
/// request and the configured PID. Read-only calls honour the configured
/// [`RetryPolicy`]; payments and refunds are never retried automatically.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

struct Inner {
    protocol: ProtocolContext,
    http: HttpConfig,
    transport: Box<dyn Transport>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("config", self.config())
            .field("http", &self.inner.http)
            .finish()
    }
}

impl Client {
    /// Build a client with the default transport and HTTP settings.
    pub fn new(pid: u64, key: impl Into<String>, api_base_url: impl Into<String>) -> Result<Self> {
        ClientBuilder::new(pid, key, api_base_url).build()
    }

    /// Start a fluent builder.
    pub fn builder(
        pid: u64,
        key: impl Into<String>,
        api_base_url: impl Into<String>,
    ) -> ClientBuilder {
        ClientBuilder::new(pid, key, api_base_url)
    }

    /// Build from `EPAY_*` environment variables.
    ///
    /// See [`ClientBuilder::from_env`] for the recognized variables.
    pub fn from_env() -> Result<Self> {
        ClientBuilder::from_env()?.build()
    }

    /// Compose a validated protocol context with a custom transport.
    pub fn with_transport(
        protocol: ProtocolContext,
        http: HttpConfig,
        transport: impl Transport + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                protocol,
                http,
                transport: Box::new(transport),
            }),
        }
    }

    /// Transport-free protocol context shared by this client.
    pub fn protocol_context(&self) -> &ProtocolContext {
        &self.inner.protocol
    }

    /// Protocol configuration.
    pub fn config(&self) -> &Config {
        self.inner.protocol.config()
    }

    /// HTTP settings.
    pub fn http_config(&self) -> &HttpConfig {
        &self.inner.http
    }

    /// MD5 signer bound to the merchant key.
    pub fn signer(&self) -> &Signer {
        self.inner.protocol.signer()
    }

    /// Sign a parameter map. See [`ProtocolContext::sign`].
    pub fn sign(&self, params: &Params) -> String {
        self.inner.protocol.sign(params)
    }

    /// Verify a signature. See [`ProtocolContext::verify`].
    pub fn verify(&self, params: &Params, sign: &str) -> bool {
        self.inner.protocol.verify(params, sign)
    }

    /// Verify a notification. See [`ProtocolContext::verify_notify`].
    pub fn verify_notify(&self, params: &Params) -> Result<NotifyData> {
        self.inner.protocol.verify_notify(params)
    }

    /// Signed `submit.php` URL. See [`ProtocolContext::build_form_payment_url`].
    pub fn build_form_payment_url(&self, request: &FormPaymentRequest) -> Result<String> {
        self.inner.protocol.build_form_payment_url(request)
    }

    /// Self-submitting form. See [`ProtocolContext::build_form_payment`].
    pub fn build_form_payment(&self, request: &FormPaymentRequest) -> Result<String> {
        self.inner.protocol.build_form_payment(request)
    }

    /// Signed form parts for custom rendering. See
    /// [`ProtocolContext::prepare_form_payment`].
    pub fn prepare_form_payment(&self, request: &FormPaymentRequest) -> Result<PreparedForm> {
        self.inner.protocol.prepare_form_payment(request)
    }

    /// Create an API payment (`POST mapi.php`). Never retried.
    pub async fn create_payment(&self, request: &PaymentRequest) -> Result<PaymentResponse> {
        self.create_payment_with(request, &CallOptions::default())
            .await
    }

    /// [`Client::create_payment`] with per-call options.
    pub async fn create_payment_with(
        &self,
        request: &PaymentRequest,
        options: &CallOptions,
    ) -> Result<PaymentResponse> {
        let callbacks = request.prepare(self.config())?;
        let mut params = self.inner.protocol.base_params();
        params.insert("type", request.pay_type.as_str());
        params.insert("out_trade_no", request.out_trade_no.clone());
        params.insert("notify_url", callbacks.notify_url);
        params.insert_opt("return_url", callbacks.return_url);
        params.insert("name", request.name.clone());
        params.insert("money", request.money.to_epay_string());
        params.insert_opt("clientip", request.client_ip.as_deref().map(str::trim));
        params.insert_opt(
            "device",
            request
                .device
                .as_ref()
                .map(|value| value.as_str().to_owned()),
        );
        params.insert_opt("param", request.param.clone());

        let signed = self.signer().sign_with_params(params);
        let response: PaymentResponse = self
            .post_json(Endpoint::CreatePayment, &Params::new(), signed, options)
            .await?;
        response.validate_success()?;
        Ok(response)
    }

    /// Query one order (`GET api.php?act=order`).
    pub async fn query_order(&self, request: &OrderQueryRequest) -> Result<OrderDetail> {
        self.query_order_with(request, &CallOptions::default())
            .await
    }

    /// [`Client::query_order`] with per-call options.
    pub async fn query_order_with(
        &self,
        request: &OrderQueryRequest,
        options: &CallOptions,
    ) -> Result<OrderDetail> {
        let identifier = request.identifier()?;
        let (name, value) = identifier.as_param();
        let mut params = Params::new();
        params.insert(name, value);
        let response: OrderDetail = self
            .merchant_call(Endpoint::QueryOrder, params, options)
            .await?;
        response.validate_success(request, self.config().pid())?;
        Ok(response)
    }

    /// Query one order by merchant order number.
    pub async fn query_order_by_out_trade_no(&self, out_trade_no: &str) -> Result<OrderDetail> {
        self.query_order(&OrderQueryRequest::out_trade_no(out_trade_no))
            .await
    }

    /// Query one order by EPay order number.
    pub async fn query_order_by_trade_no(&self, trade_no: &str) -> Result<OrderDetail> {
        self.query_order(&OrderQueryRequest::trade_no(trade_no))
            .await
    }

    /// List one page of orders (`GET api.php?act=orders`).
    ///
    /// `limit` is clamped to `1..=50` and `page` starts at `1`. Both `page`
    /// and the equivalent `offset` are sent because forks disagree on which
    /// one they read. Use [`OrderListResponse::next_page`] to continue, or
    /// [`Client::query_all_orders`] to collect several pages.
    pub async fn query_orders(&self, limit: u32, page: u32) -> Result<OrderListResponse> {
        self.query_orders_with(limit, page, &CallOptions::default())
            .await
    }

    /// [`Client::query_orders`] with per-call options.
    pub async fn query_orders_with(
        &self,
        limit: u32,
        page: u32,
        options: &CallOptions,
    ) -> Result<OrderListResponse> {
        let limit = limit.clamp(1, 50);
        let page = page.max(1);
        let offset = u64::from(page - 1) * u64::from(limit);
        let mut params = Params::new();
        params.insert("limit", limit.to_string());
        params.insert("page", page.to_string());
        params.insert("offset", offset.to_string());
        let response: OrderListResponse = self
            .merchant_call(Endpoint::QueryOrders, params, options)
            .await?;
        response.validate_success(self.config().pid())?;
        Ok(response)
    }

    /// Collect orders from page `1` while pages are full, stopping after
    /// `max_pages` pages (`0` performs no request).
    pub async fn query_all_orders(&self, limit: u32, max_pages: u32) -> Result<Vec<OrderDetail>> {
        self.query_all_orders_with(limit, max_pages, &CallOptions::default())
            .await
    }

    /// [`Client::query_all_orders`] with per-call options.
    pub async fn query_all_orders_with(
        &self,
        limit: u32,
        max_pages: u32,
        options: &CallOptions,
    ) -> Result<Vec<OrderDetail>> {
        let limit = limit.clamp(1, 50);
        if max_pages == 0 {
            return Ok(Vec::new());
        }
        let mut orders = Vec::new();
        let mut page = 1;
        loop {
            let response = self.query_orders_with(limit, page, options).await?;
            let next = response.next_page(page, limit);
            orders.extend(response.orders);
            match next {
                Some(next_page) if next_page <= max_pages => page = next_page,
                _ => return Ok(orders),
            }
        }
    }

    /// Refund an order (`POST api.php?act=refund`). Never retried.
    pub async fn refund(&self, request: &RefundRequest) -> Result<RefundResponse> {
        self.refund_with(request, &CallOptions::default()).await
    }

    /// [`Client::refund`] with per-call options.
    pub async fn refund_with(
        &self,
        request: &RefundRequest,
        options: &CallOptions,
    ) -> Result<RefundResponse> {
        let identifier = request.identifier()?;
        let (name, value) = identifier.as_param();
        let mut params = Params::new();
        params.insert(name, value);
        params.insert("money", request.money.to_epay_string());
        self.merchant_call(Endpoint::Refund, params, options).await
    }

    /// Refund by merchant order number.
    pub async fn refund_by_out_trade_no(
        &self,
        out_trade_no: &str,
        money: Money,
    ) -> Result<RefundResponse> {
        self.refund(&RefundRequest::out_trade_no(out_trade_no, money))
            .await
    }

    /// Refund by EPay order number.
    pub async fn refund_by_trade_no(&self, trade_no: &str, money: Money) -> Result<RefundResponse> {
        self.refund(&RefundRequest::trade_no(trade_no, money)).await
    }

    /// Merchant profile and balance (`GET api.php?act=query`).
    pub async fn query_merchant(&self) -> Result<MerchantInfo> {
        self.query_merchant_with(&CallOptions::default()).await
    }

    /// [`Client::query_merchant`] with per-call options.
    pub async fn query_merchant_with(&self, options: &CallOptions) -> Result<MerchantInfo> {
        let response: MerchantInfo = self
            .merchant_call(Endpoint::QueryMerchant, Params::new(), options)
            .await?;
        response.validate_success(self.config().pid())?;
        Ok(response)
    }

    /// Settlement records (`GET api.php?act=settle`).
    pub async fn query_settlements(&self) -> Result<SettlementListResponse> {
        self.query_settlements_with(&CallOptions::default()).await
    }

    /// [`Client::query_settlements`] with per-call options.
    pub async fn query_settlements_with(
        &self,
        options: &CallOptions,
    ) -> Result<SettlementListResponse> {
        let response: SettlementListResponse = self
            .merchant_call(Endpoint::QuerySettlements, Params::new(), options)
            .await?;
        response.validate_success(self.config().pid())?;
        Ok(response)
    }

    /// `api.php` call authenticated with `pid` + `key`.
    ///
    /// `api.php` dispatches on `$_GET['act']`, so for POST endpoints `act`
    /// stays in the query string while everything else is form-encoded.
    async fn merchant_call<R: DeserializeOwned>(
        &self,
        endpoint: Endpoint,
        extra: Params,
        options: &CallOptions,
    ) -> Result<R> {
        let mut params = self.inner.protocol.base_params();
        params.merge(extra);
        params.insert("key", self.config().key());
        match endpoint.method() {
            Method::Get => {
                params.insert_opt("act", endpoint.act());
                self.get_json(endpoint, params, options).await
            }
            Method::Post => {
                let mut query = Params::new();
                query.insert_opt("act", endpoint.act());
                self.post_json(endpoint, &query, params, options).await
            }
        }
    }

    fn transport_options(&self, options: &CallOptions) -> TransportOptions {
        TransportOptions::new(self.inner.http.max_response_body_bytes())
            .with_timeout(Some(options.timeout.unwrap_or(self.inner.http.timeout())))
    }

    /// GET with the configured retry policy for retryable failures. Every
    /// outbound request passes the size bound here or in [`Self::post_json`].
    async fn get_json<R: DeserializeOwned>(
        &self,
        endpoint: Endpoint,
        params: Params,
        options: &CallOptions,
    ) -> Result<R> {
        let encoded = params.encode();
        validate_encoded_size(encoded.len())?;
        let mut url = self.config().endpoint(endpoint.path())?;
        url.set_query(Some(&encoded));
        if self.config().debug() {
            let mut safe_url = self.config().endpoint(endpoint.path())?;
            safe_url.set_query(Some(&params.redacted_encode()));
            sdk_debug!("[epay-sdk] GET {safe_url}");
        }
        let transport_options = self.transport_options(options);
        let policy = options.retry.as_ref().unwrap_or(self.inner.http.retry());
        let call = async {
            let mut retries = 0;
            loop {
                let attempt = self
                    .inner
                    .transport
                    .get(&url, transport_options)
                    .await
                    .map_err(Error::sanitized);
                match attempt {
                    Ok(body) => return self.finish(endpoint, body, transport_options),
                    Err(error) if policy.should_retry(retries, &error) => {
                        retries += 1;
                        let delay = policy.backoff(retries);
                        sdk_warn!(
                            "[epay-sdk] {endpoint} failed ({error}); retry {retries}/{} in {delay:?}",
                            policy.max_retries()
                        );
                        self.inner.transport.sleep(delay).await;
                    }
                    Err(error) => return Err(error),
                }
            }
        };
        instrument(endpoint, "GET", call).await
    }

    async fn post_json<R: DeserializeOwned>(
        &self,
        endpoint: Endpoint,
        query: &Params,
        form: Params,
        options: &CallOptions,
    ) -> Result<R> {
        let encoded_query = query.encode();
        validate_encoded_size(encoded_query.len() + form.encode().len())?;
        let mut url = self.config().endpoint(endpoint.path())?;
        if !query.is_empty() {
            url.set_query(Some(&encoded_query));
        }
        if self.config().debug() {
            sdk_debug!("[epay-sdk] POST {url} form={}", form.redacted_encode());
        }
        let transport_options = self.transport_options(options);
        let call = async {
            let pairs = form.as_pairs();
            let body = self
                .inner
                .transport
                .post_form(&url, &pairs, transport_options)
                .await
                .map_err(Error::sanitized)?;
            self.finish(endpoint, body, transport_options)
        };
        instrument(endpoint, "POST", call).await
    }

    fn finish<R: DeserializeOwned>(
        &self,
        endpoint: Endpoint,
        body: Vec<u8>,
        options: TransportOptions,
    ) -> Result<R> {
        if body.len() > options.max_response_bytes {
            return Err(Error::ResponseTooLarge {
                limit: options.max_response_bytes,
            });
        }
        if self.config().debug() {
            let text = redact_secrets(&String::from_utf8_lossy(&body));
            let preview: String = text.chars().take(1024).collect();
            sdk_debug!("[epay-sdk] response {preview}");
        }
        parse_json(endpoint, &body)
    }
}

/// Decode the envelope first so a business error is reported even when the
/// error payload does not fit the success schema. The consumed `code` member
/// and any echoed credentials (`key`, `sign`) are removed so they never
/// surface in `extra` maps.
fn parse_json<T: DeserializeOwned>(endpoint: Endpoint, body: &[u8]) -> Result<T> {
    let mut value: serde_json::Value = serde_json::from_slice(body)
        .map_err(|error| Error::invalid_response(endpoint, error, body))?;
    let code = response_code(endpoint, &value)?;
    if !endpoint.is_success_code(code) {
        let message = value
            .get("msg")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .unwrap_or("server returned an error without a message");
        return Err(Error::api(endpoint, code, message));
    }
    if let Some(object) = value.as_object_mut() {
        object.remove("code");
        object.remove("key");
        object.remove("sign");
    }
    serde_json::from_value(value).map_err(|error| Error::invalid_response(endpoint, error, body))
}

fn response_code(endpoint: Endpoint, value: &serde_json::Value) -> Result<i32> {
    const MESSAGE: &str = "response code is not an i32 integer";
    let code = value.get("code").ok_or_else(|| {
        Error::invalid_response_message(endpoint, "response is missing integer code")
    })?;
    match code {
        serde_json::Value::Number(number) => number
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .ok_or_else(|| Error::invalid_response_message(endpoint, MESSAGE)),
        serde_json::Value::String(value) if !value.is_empty() => value
            .trim()
            .parse::<i32>()
            .map_err(|_| Error::invalid_response_message(endpoint, MESSAGE)),
        _ => Err(Error::invalid_response_message(endpoint, MESSAGE)),
    }
}

/// Fluent async client constructor.
pub struct ClientBuilder {
    protocol: ProtocolContextBuilder,
    timeout: Duration,
    connect_timeout: Option<Duration>,
    max_response_body_bytes: usize,
    retry: RetryPolicy,
    transport: Option<Box<dyn Transport>>,
    #[cfg(any(feature = "reqwest-rustls", feature = "reqwest-native-tls"))]
    reqwest_client: Option<reqwest::Client>,
}

impl std::fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientBuilder")
            .field("protocol", &self.protocol)
            .field("timeout", &self.timeout)
            .field("connect_timeout", &self.connect_timeout)
            .field("max_response_body_bytes", &self.max_response_body_bytes)
            .field("retry", &self.retry)
            .field("custom_transport", &self.transport.is_some())
            .finish()
    }
}

impl ClientBuilder {
    /// Start with merchant id, key and gateway base URL.
    pub fn new(pid: u64, key: impl Into<String>, api_base_url: impl Into<String>) -> Self {
        Self::from_protocol(ProtocolContextBuilder::new(pid, key, api_base_url))
    }

    /// Start from a pre-configured protocol builder.
    pub fn from_protocol(protocol: ProtocolContextBuilder) -> Self {
        Self {
            protocol,
            timeout: HttpConfig::DEFAULT_TIMEOUT,
            connect_timeout: None,
            max_response_body_bytes: HttpConfig::DEFAULT_MAX_RESPONSE_BODY_BYTES,
            retry: RetryPolicy::NONE,
            transport: None,
            #[cfg(any(feature = "reqwest-rustls", feature = "reqwest-native-tls"))]
            reqwest_client: None,
        }
    }

    /// Read protocol and HTTP settings from the process environment.
    ///
    /// Recognizes every variable listed at
    /// [`ProtocolContextBuilder::from_env`] plus:
    ///
    /// | Variable | Meaning |
    /// |---|---|
    /// | `EPAY_TIMEOUT_SECS` | per-attempt request timeout in seconds |
    /// | `EPAY_CONNECT_TIMEOUT_SECS` | TCP/TLS connect timeout in seconds |
    /// | `EPAY_MAX_RESPONSE_BYTES` | response body limit |
    /// | `EPAY_MAX_RETRIES` | retries for GET endpoints (default `0`) |
    pub fn from_env() -> Result<Self> {
        Self::from_env_with(&env::process)
    }

    pub(crate) fn from_env_with(lookup: env::Lookup<'_>) -> Result<Self> {
        let mut builder = Self::from_protocol(ProtocolContextBuilder::from_env_with(lookup)?);
        const TIMEOUT: &str = "EPAY_TIMEOUT_SECS must be a positive integer";
        if let Some(value) = env::typed(lookup, "EPAY_TIMEOUT_SECS", TIMEOUT)? {
            let seconds = value
                .trim()
                .parse::<u64>()
                .map_err(|_| Error::Config(TIMEOUT))?;
            builder = builder.timeout(Duration::from_secs(seconds));
        }
        const CONNECT: &str = "EPAY_CONNECT_TIMEOUT_SECS must be a positive integer";
        if let Some(value) = env::typed(lookup, "EPAY_CONNECT_TIMEOUT_SECS", CONNECT)? {
            let seconds = value
                .trim()
                .parse::<u64>()
                .map_err(|_| Error::Config(CONNECT))?;
            builder = builder.connect_timeout(Duration::from_secs(seconds));
        }
        const MAX_BYTES: &str = "EPAY_MAX_RESPONSE_BYTES must be a positive integer";
        if let Some(value) = env::typed(lookup, "EPAY_MAX_RESPONSE_BYTES", MAX_BYTES)? {
            let bytes = value
                .trim()
                .parse::<usize>()
                .map_err(|_| Error::Config(MAX_BYTES))?;
            builder = builder.max_response_body_bytes(bytes);
        }
        const RETRIES: &str = "EPAY_MAX_RETRIES must be a non-negative integer";
        if let Some(value) = env::typed(lookup, "EPAY_MAX_RETRIES", RETRIES)? {
            let retries = value
                .trim()
                .parse::<u32>()
                .map_err(|_| Error::Config(RETRIES))?;
            builder = builder.retry(RetryPolicy::new(retries));
        }
        Ok(builder)
    }

    /// Per-attempt request timeout for the built-in transport.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// TCP/TLS connect timeout for the SDK-created reqwest client (default:
    /// none beyond the per-attempt timeout). Zero fails at [`Self::build`].
    /// It cannot be applied to a caller-supplied [`Self::reqwest_client`]
    /// (that combination fails at build) or to a custom [`Self::transport`]
    /// (silently not applicable; configure the transport itself).
    pub fn connect_timeout(mut self, connect_timeout: Duration) -> Self {
        self.connect_timeout = Some(connect_timeout);
        self
    }

    /// Maximum accepted response body size in bytes.
    pub fn max_response_body_bytes(mut self, bytes: usize) -> Self {
        self.max_response_body_bytes = bytes;
        self
    }

    /// Retry policy for GET endpoints (default: no retries).
    pub fn retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Enable redacted debug logging. See [`ProtocolContextBuilder::debug`].
    pub fn debug(mut self, debug: bool) -> Self {
        self.protocol = self.protocol.debug(debug);
        self
    }

    /// Default notify URL. See [`ProtocolContextBuilder::notify_url`].
    pub fn notify_url(mut self, url: impl Into<String>) -> Self {
        self.protocol = self.protocol.notify_url(url);
        self
    }

    /// Default return URL. See [`ProtocolContextBuilder::return_url`].
    pub fn return_url(mut self, url: impl Into<String>) -> Self {
        self.protocol = self.protocol.return_url(url);
        self
    }

    /// Allow a plain-HTTP gateway. See
    /// [`ProtocolContextBuilder::allow_insecure_http`].
    pub fn allow_insecure_http(mut self, allow: bool) -> Self {
        self.protocol = self.protocol.allow_insecure_http(allow);
        self
    }

    /// Allow plain-HTTP callbacks. See
    /// [`ProtocolContextBuilder::allow_insecure_callback_http`].
    pub fn allow_insecure_callback_http(mut self, allow: bool) -> Self {
        self.protocol = self.protocol.allow_insecure_callback_http(allow);
        self
    }

    /// Use a custom transport instead of reqwest.
    pub fn transport(mut self, transport: impl Transport + 'static) -> Self {
        self.transport = Some(Box::new(transport));
        self
    }

    /// Reuse an existing `reqwest::Client`. Its TLS and redirect policy are
    /// kept (the SDK does not disable redirects on it); the configured or
    /// per-call timeout is applied per request.
    #[cfg(any(feature = "reqwest-rustls", feature = "reqwest-native-tls"))]
    pub fn reqwest_client(mut self, client: reqwest::Client) -> Self {
        self.reqwest_client = Some(client);
        self
    }

    /// Validate settings and build the client.
    pub fn build(self) -> Result<Client> {
        #[cfg(any(feature = "reqwest-rustls", feature = "reqwest-native-tls"))]
        if self.reqwest_client.is_some() && self.connect_timeout.is_some() {
            return Err(Error::Config(
                "connect_timeout cannot be applied to a caller-supplied reqwest::Client; configure it on that client instead",
            ));
        }
        let http = HttpConfig::new(self.timeout, self.max_response_body_bytes)?
            .with_retry(self.retry)
            .with_connect_timeout(self.connect_timeout)?;
        let protocol = self.protocol.build()?;
        let transport = if let Some(transport) = self.transport {
            transport
        } else {
            #[cfg(any(feature = "reqwest-rustls", feature = "reqwest-native-tls"))]
            {
                let transport = if let Some(client) = self.reqwest_client {
                    crate::transport::ReqwestTransport::from_client(client)
                } else {
                    crate::transport::ReqwestTransport::new(http.timeout(), http.connect_timeout())?
                };
                Box::new(transport)
            }
            #[cfg(not(any(feature = "reqwest-rustls", feature = "reqwest-native-tls")))]
            {
                return Err(Error::Config(
                    "no HTTP transport: enable a reqwest feature or call ClientBuilder::transport()",
                ));
            }
        };
        Ok(Client {
            inner: Arc::new(Inner {
                protocol,
                http,
                transport,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::error::{TransportErrorKind, code};
    use crate::transport::MockTransport;
    use crate::types::{Device, PayType};

    fn client_with(transport: MockTransport) -> Client {
        Client::builder(1001, "testkey123", "https://pay.example.com")
            .notify_url("https://shop.example.com/notify")
            .return_url("https://shop.example.com/return")
            .transport(transport)
            .build()
            .unwrap()
    }

    fn money() -> Money {
        Money::from_yuan_str("10.00").unwrap()
    }

    fn timeout_error() -> Error {
        Error::transport(TransportErrorKind::Timeout, "request timed out")
    }

    #[tokio::test]
    async fn create_payment_posts_validated_signed_form() {
        let transport = MockTransport::new()
            .push_json(r#"{"code":1,"trade_no":"T1","payurl":"https://pay/x"}"#);
        let client = client_with(transport.clone());
        let response = client
            .create_payment(
                &PaymentRequest::new(PayType::Alipay, "ORDER001", "Product", money())
                    .device(Device::Pc)
                    .client_ip("127.0.0.1"),
            )
            .await
            .unwrap();
        assert_eq!(response.trade_no, "T1");
        let recorded = transport.recorded();
        assert_eq!(recorded[0].method, "POST");
        assert_eq!(recorded[0].url, "https://pay.example.com/mapi.php");
        assert!(recorded[0].query.is_empty());
        assert_eq!(recorded[0].form.get("clientip"), Some("127.0.0.1"));
        assert_eq!(
            recorded[0].form.get("notify_url"),
            Some("https://shop.example.com/notify")
        );
        assert_eq!(
            recorded[0].form.get("return_url"),
            Some("https://shop.example.com/return")
        );
        assert_eq!(recorded[0].form.get("key"), None);
        assert_eq!(
            recorded[0].options.timeout,
            Some(HttpConfig::DEFAULT_TIMEOUT),
            "HttpConfig timeout reaches the transport"
        );
        assert!(client.verify(&recorded[0].form, recorded[0].form.get("sign").unwrap()));
    }

    #[tokio::test]
    async fn successful_payment_requires_destination() {
        let transport = MockTransport::new().push_json(r#"{"code":1,"trade_no":"T1"}"#);
        let error = client_with(transport)
            .create_payment(
                &PaymentRequest::new(PayType::Alipay, "ORDER001", "Product", money())
                    .client_ip("127.0.0.1"),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code(), code::INVALID_RESPONSE);
        assert!(matches!(
            error,
            Error::InvalidResponse {
                endpoint: Endpoint::CreatePayment,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn query_order_validates_identity_and_keeps_fork_fields() {
        let transport = MockTransport::new().push_json(
            r#"{"code":1,"msg":"succ","trade_no":"T1","out_trade_no":"ORDER001","pid":1001,"status":1,"money":"10.00","bill_trade_no":null,"pay_fee":"0.06"}"#,
        );
        let client = client_with(transport.clone());
        let order = client
            .query_order_by_out_trade_no("ORDER001")
            .await
            .unwrap();
        assert!(order.is_paid());
        assert_eq!(order.extra["pay_fee"], "0.06");
        assert!(order.extra["bill_trade_no"].is_null());
        assert!(
            !order.extra.contains_key("code"),
            "envelope code is consumed"
        );
        assert!(
            !order.extra.contains_key("trade_no"),
            "modelled fields stay typed"
        );
        let recorded = transport.recorded();
        assert_eq!(recorded[0].method, "GET");
        assert_eq!(recorded[0].url, "https://pay.example.com/api.php");
        assert_eq!(recorded[0].query.get("act"), Some("order"));
        assert_eq!(recorded[0].query.get("key"), Some("testkey123"));
        assert_eq!(recorded[0].query.get("pid"), Some("1001"));
        assert_eq!(recorded[0].query.get("out_trade_no"), Some("ORDER001"));
        let debug = format!("{:?}", recorded[0]);
        assert!(!debug.contains("testkey123"), "{debug}");
    }

    #[tokio::test]
    async fn malformed_success_is_rejected() {
        let transport = MockTransport::new().push_json(r#"{"code":1}"#);
        let error = client_with(transport)
            .query_order_by_out_trade_no("ORDER001")
            .await
            .unwrap_err();
        assert_eq!(error.code(), code::INVALID_RESPONSE);
    }

    #[tokio::test]
    async fn error_envelope_is_reported_before_success_payload_is_decoded() {
        let transport =
            MockTransport::new().push_json(r#"{"code":"-1","msg":"余额不足","pid":{}}"#);
        let error = client_with(transport).query_merchant().await.unwrap_err();
        assert!(matches!(
            error,
            Error::Api {
                endpoint: Endpoint::QueryMerchant,
                code: -1,
                ..
            }
        ));
        assert!(error.to_string().contains("余额不足"));
    }

    #[tokio::test]
    async fn malformed_response_code_is_rejected() {
        for body in [
            r#"{"code":null}"#,
            r#"{"code":1.5}"#,
            r#"{"code":""}"#,
            r#"{"code":2147483648}"#,
            r#"[]"#,
        ] {
            let error = client_with(MockTransport::new().push_json(body))
                .query_merchant()
                .await
                .unwrap_err();
            assert_eq!(error.code(), code::INVALID_RESPONSE, "{body}");
        }
    }

    #[tokio::test]
    async fn refund_uses_official_auth_and_success_code() {
        let transport = MockTransport::new()
            .push_json(r#"{"code":0,"msg":"退款成功"}"#)
            .push_json(r#"{"code":1,"msg":"ok"}"#)
            .push_json(r#"{"code":-1,"msg":"该订单已退款！"}"#);
        let client = client_with(transport.clone());
        let response = client
            .refund_by_out_trade_no("ORDER001", money())
            .await
            .unwrap();
        assert_eq!(response.msg, "退款成功");
        client
            .refund_by_trade_no("T1", money())
            .await
            .expect("fork success code 1");
        let error = client.refund_by_trade_no("T1", money()).await.unwrap_err();
        assert!(matches!(
            error,
            Error::Api {
                endpoint: Endpoint::Refund,
                code: -1,
                ..
            }
        ));

        let recorded = transport.recorded();
        assert_eq!(recorded[0].method, "POST");
        assert_eq!(recorded[0].url, "https://pay.example.com/api.php");
        assert_eq!(recorded[0].query.get("act"), Some("refund"));
        assert_eq!(recorded[0].query.len(), 1, "only act travels in the query");
        assert_eq!(recorded[0].form.get("key"), Some("testkey123"));
        assert_eq!(recorded[0].form.get("pid"), Some("1001"));
        assert_eq!(recorded[0].form.get("money"), Some("10.00"));
        assert_eq!(recorded[0].form.get("out_trade_no"), Some("ORDER001"));
        assert_eq!(recorded[0].form.get("act"), None);
        assert_eq!(recorded[1].form.get("trade_no"), Some("T1"));
    }

    #[tokio::test]
    async fn query_orders_sends_page_and_offset_with_clamped_limit() {
        let transport = MockTransport::new()
            .push_json(
                r#"{"code":1,"count":"2","data":[{"trade_no":"T1","out_trade_no":"O1","pid":1001,"status":1,"money":"1.00"},{"trade_no":"T2","out_trade_no":"O2","pid":1001,"status":0,"money":"2.00","buyer":null}]}"#,
            )
            .push_json(r#"{"code":1,"count":0,"data":null}"#)
            .push_json(
                r#"{"code":1,"count":1,"data":[{"trade_no":"T1","out_trade_no":"O1","pid":2002,"status":1,"money":"1.00"}]}"#,
            );
        let client = client_with(transport.clone());

        let list = client.query_orders(500, 3).await.unwrap();
        assert_eq!(list.count, 2);
        assert_eq!(list.orders.len(), 2);
        assert!(list.orders[0].is_paid());
        assert!(list.orders[1].is_unpaid());
        assert!(!list.has_more(50));
        assert_eq!(list.next_page(3, 2), Some(4));

        let empty = client.query_orders(0, 0).await.unwrap();
        assert!(empty.orders.is_empty());
        assert_eq!(empty.next_page(1, 1), None);

        let error = client.query_orders(10, 1).await.unwrap_err();
        assert!(matches!(
            error,
            Error::InvalidResponse {
                endpoint: Endpoint::QueryOrders,
                ..
            }
        ));

        let recorded = transport.recorded();
        assert_eq!(recorded[0].query.get("act"), Some("orders"));
        assert_eq!(recorded[0].query.get("limit"), Some("50"));
        assert_eq!(recorded[0].query.get("page"), Some("3"));
        assert_eq!(recorded[0].query.get("offset"), Some("100"));
        assert_eq!(recorded[1].query.get("limit"), Some("1"));
        assert_eq!(recorded[1].query.get("page"), Some("1"));
        assert_eq!(recorded[1].query.get("offset"), Some("0"));
    }

    #[tokio::test]
    async fn query_all_orders_pages_while_full_and_respects_bound() {
        fn page(ids: &[&str]) -> String {
            let rows: Vec<String> = ids
                .iter()
                .map(|id| {
                    format!(
                        r#"{{"trade_no":"T{id}","out_trade_no":"O{id}","pid":1001,"status":1,"money":"1.00"}}"#
                    )
                })
                .collect();
            format!(r#"{{"code":1,"count":5,"data":[{}]}}"#, rows.join(","))
        }
        let transport = MockTransport::new()
            .push_json(page(&["1", "2"]))
            .push_json(page(&["3", "4"]))
            .push_json(page(&["5"]))
            .push_json(page(&["1", "2"]))
            .push_json(page(&["3", "4"]));
        let client = client_with(transport.clone());

        let all = client.query_all_orders(2, 10).await.unwrap();
        assert_eq!(all.len(), 5);
        assert_eq!(all[4].out_trade_no, "O5");
        assert_eq!(transport.recorded().len(), 3);

        let bounded = client.query_all_orders(2, 2).await.unwrap();
        assert_eq!(bounded.len(), 4, "stops after max_pages");
        assert_eq!(transport.recorded().len(), 5);
        assert!(client.query_all_orders(2, 0).await.unwrap().is_empty());
        assert_eq!(transport.recorded().len(), 5, "max_pages = 0 sends nothing");
        assert_eq!(transport.recorded()[4].query.get("page"), Some("2"));
    }

    #[tokio::test]
    async fn query_settlements_tolerates_php_nulls_and_binds_pid() {
        let transport = MockTransport::new()
            .push_json(
                r#"{"code":1,"msg":null,"count":1,"data":[{"id":"7","uid":"1001","money":"5.00","account":null,"status":1,"addtime":null}]}"#,
            )
            .push_json(r#"{"code":1,"count":1,"data":[{"id":8,"uid":2002,"money":"5.00"}]}"#);
        let client = client_with(transport.clone());
        let list = client.query_settlements().await.unwrap();
        assert_eq!(list.settlements.len(), 1);
        assert_eq!(list.settlements[0].id, 7);
        assert!(matches!(
            client.query_settlements().await,
            Err(Error::InvalidResponse {
                endpoint: Endpoint::QuerySettlements,
                ..
            })
        ));
        assert_eq!(transport.recorded()[0].query.get("act"), Some("settle"));
    }

    #[tokio::test]
    async fn merchant_pid_is_bound_and_nulls_are_tolerated() {
        let transport = MockTransport::new()
            .push_json(r#"{"code":1,"pid":2002,"money":"0.00"}"#)
            .push_json(
                r#"{"code":1,"pid":"1001","key":"super-secret","active":1,"money":"3.50","type":null,"account":null,"username":null}"#,
            );
        let client = client_with(transport);
        assert!(client.query_merchant().await.is_err());
        let info = client.query_merchant().await.unwrap();
        assert_eq!(info.pid, 1001);
        assert_eq!(info.account, "");
        assert!(!info.extra.contains_key("key"), "echoed key is stripped");
        assert!(!format!("{info:?}").contains("super-secret"));
    }

    #[tokio::test]
    async fn custom_transport_errors_are_resanitized_and_classified() {
        let transport = MockTransport::new().push_err(Error::Transport {
            kind: TransportErrorKind::Timeout,
            message: "failed https://pay.example/api.php?key=super-secret".into(),
        });
        let error = client_with(transport).query_merchant().await.unwrap_err();
        assert!(!error.to_string().contains("super-secret"), "{error}");
        assert!(error.is_timeout());
        assert_eq!(error.code(), code::NETWORK);
    }

    #[tokio::test]
    async fn custom_transport_cannot_bypass_response_limit() {
        let transport = MockTransport::new().push_bytes(vec![b'x'; 20]);
        let client = Client::builder(1001, "k", "https://pay.example.com")
            .max_response_body_bytes(10)
            .transport(transport)
            .build()
            .unwrap();
        assert!(matches!(
            client.query_merchant().await,
            Err(Error::ResponseTooLarge { limit: 10 })
        ));
    }

    #[tokio::test]
    async fn get_endpoints_retry_retryable_failures_with_backoff() {
        let transport = MockTransport::new()
            .push_err(timeout_error())
            .push_err(Error::http(503, b"upstream down"))
            .push_json(r#"{"code":1,"pid":1001,"money":"1.00"}"#);
        let client = Client::builder(1001, "k", "https://pay.example.com")
            .retry(RetryPolicy::reads())
            .transport(transport.clone())
            .build()
            .unwrap();
        let info = client.query_merchant().await.unwrap();
        assert_eq!(info.pid, 1001);
        assert_eq!(transport.recorded().len(), 3);
        assert_eq!(
            transport.recorded_sleeps(),
            vec![Duration::from_millis(200), Duration::from_millis(400)]
        );
    }

    #[tokio::test]
    async fn retries_are_bounded_and_skip_non_retryable_errors() {
        let transport = MockTransport::new()
            .push_err(timeout_error())
            .push_err(timeout_error())
            .push_err(timeout_error());
        let client = Client::builder(1001, "k", "https://pay.example.com")
            .retry(RetryPolicy::reads())
            .transport(transport.clone())
            .build()
            .unwrap();
        assert!(client.query_merchant().await.unwrap_err().is_timeout());
        assert_eq!(transport.recorded().len(), 3, "1 attempt + 2 retries");

        let transport = MockTransport::new().push_json(r#"{"code":-1,"msg":"nope"}"#);
        let client = Client::builder(1001, "k", "https://pay.example.com")
            .retry(RetryPolicy::reads())
            .transport(transport.clone())
            .build()
            .unwrap();
        assert!(matches!(
            client.query_merchant().await,
            Err(Error::Api { .. })
        ));
        assert_eq!(transport.recorded().len(), 1);
    }

    #[tokio::test]
    async fn mutating_endpoints_are_never_retried() {
        let transport = MockTransport::new()
            .push_err(timeout_error())
            .push_err(timeout_error());
        let client = Client::builder(1001, "k", "https://pay.example.com")
            .notify_url("https://shop.example.com/notify")
            .retry(RetryPolicy::reads())
            .transport(transport.clone())
            .build()
            .unwrap();
        assert!(
            client
                .refund_by_out_trade_no("O1", money())
                .await
                .unwrap_err()
                .is_timeout()
        );
        let request = PaymentRequest::new(PayType::Alipay, "O1", "P", money()).client_ip("::1");
        assert!(
            client
                .create_payment_with(&request, &CallOptions::new().retry(RetryPolicy::new(5)))
                .await
                .unwrap_err()
                .is_timeout()
        );
        assert_eq!(transport.recorded().len(), 2);
        assert!(transport.recorded_sleeps().is_empty());
    }

    #[tokio::test]
    async fn call_options_override_timeout_and_retry() {
        let transport = MockTransport::new()
            .push_err(timeout_error())
            .push_json(r#"{"code":1,"pid":1001,"money":"1.00"}"#);
        let client = client_with(transport.clone());
        assert!(
            client.query_merchant().await.is_err(),
            "no retry by default"
        );
        let options = CallOptions::new()
            .timeout(Duration::from_secs(3))
            .retry(RetryPolicy::new(1));
        client.query_merchant_with(&options).await.unwrap();
        let recorded = transport.recorded();
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[1].options.timeout, Some(Duration::from_secs(3)));
        assert_eq!(
            recorded[1].options.max_response_bytes,
            HttpConfig::DEFAULT_MAX_RESPONSE_BODY_BYTES
        );
    }

    #[test]
    fn form_operations_forward_to_protocol_context() {
        let client = client_with(MockTransport::new());
        let html = client
            .build_form_payment(
                &FormPaymentRequest::new("ORDER001", "A&B", money()).pay_type(PayType::Alipay),
            )
            .unwrap();
        assert!(html.contains("payForm"));
        assert!(!html.contains("A&B"));
    }

    #[tokio::test]
    async fn every_endpoint_is_size_bounded_before_the_transport() {
        let transport = MockTransport::new().push_json(r#"{"code":1,"pid":1001,"money":"1.00"}"#);
        let client = Client::builder(1001, "k".repeat(5000), "https://pay.example.com")
            .notify_url("https://shop.example.com/notify")
            .transport(transport.clone())
            .build()
            .unwrap();
        // The merchant key travels in every api.php call (query for GET, form
        // for POST); mapi.php sends only the signature, so it is bounded by
        // its own field limits instead (see protocol tests).
        for result in [
            client.query_merchant().await.err(),
            client.refund_by_out_trade_no("O1", money()).await.err(),
        ] {
            assert!(
                matches!(result, Some(Error::Param(m)) if m.starts_with("encoded request exceeds")),
                "{result:?}"
            );
        }
        assert!(
            transport.recorded().is_empty(),
            "nothing reached the transport"
        );
    }

    #[test]
    fn builder_validates_http_settings() {
        assert!(matches!(
            Client::builder(1001, "k", "https://pay.example.com")
                .timeout(Duration::ZERO)
                .transport(MockTransport::new())
                .build(),
            Err(Error::Config(_))
        ));
        assert!(matches!(
            Client::builder(1001, "k", "https://pay.example.com")
                .connect_timeout(Duration::ZERO)
                .transport(MockTransport::new())
                .build(),
            Err(Error::Config(m)) if m.starts_with("connect timeout")
        ));
        #[cfg(any(feature = "reqwest-rustls", feature = "reqwest-native-tls"))]
        assert!(matches!(
            Client::builder(1001, "k", "https://pay.example.com")
                .connect_timeout(Duration::from_secs(1))
                .reqwest_client(reqwest::Client::new())
                .build(),
            Err(Error::Config(m)) if m.contains("caller-supplied")
        ));
        let client = Client::with_transport(
            ProtocolContext::new(1001, "k", "https://pay.example.com").unwrap(),
            HttpConfig::default(),
            MockTransport::new(),
        );
        assert_eq!(client.http_config().timeout(), HttpConfig::DEFAULT_TIMEOUT);
    }

    #[test]
    fn from_env_reads_http_settings() {
        let vars: HashMap<&str, &str> = [
            ("EPAY_PID", "1001"),
            ("EPAY_KEY", "k"),
            ("EPAY_API_URL", "https://pay.example.com"),
            ("EPAY_TIMEOUT_SECS", "5"),
            ("EPAY_CONNECT_TIMEOUT_SECS", "2"),
            ("EPAY_MAX_RESPONSE_BYTES", "4096"),
            ("EPAY_MAX_RETRIES", "3"),
        ]
        .into();
        let lookup = |name: &str| {
            vars.get(name)
                .map(|value| (*value).to_owned())
                .ok_or(std::env::VarError::NotPresent)
        };
        let client = ClientBuilder::from_env_with(&lookup)
            .unwrap()
            .transport(MockTransport::new())
            .build()
            .unwrap();
        assert_eq!(client.http_config().timeout(), Duration::from_secs(5));
        assert_eq!(
            client.http_config().connect_timeout(),
            Some(Duration::from_secs(2))
        );
        assert_eq!(client.http_config().max_response_body_bytes(), 4096);
        assert_eq!(client.http_config().retry().max_retries(), 3);

        for (name, value) in [
            ("EPAY_TIMEOUT_SECS", "soon"),
            ("EPAY_MAX_RETRIES", "-1"),
            ("EPAY_CONNECT_TIMEOUT_SECS", "0"),
        ] {
            let mut bad = vars.clone();
            bad.insert(name, value);
            let lookup = |name: &str| {
                bad.get(name)
                    .map(|value| (*value).to_owned())
                    .ok_or(std::env::VarError::NotPresent)
            };
            assert!(
                matches!(
                    ClientBuilder::from_env_with(&lookup)
                        .and_then(|builder| builder.transport(MockTransport::new()).build()),
                    Err(Error::Config(_))
                ),
                "{name}={value}"
            );
        }
    }
}
