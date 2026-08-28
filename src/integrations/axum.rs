//! Axum 0.8 integration for strict EPay callback and return verification.
//!
//! Use [`VerifiedNotify`] for `notify_url`: every rejection becomes HTTP 200
//! with body `fail`, so EPay can retry. Use [`VerifiedReturn`] for a browser
//! `return_url`: verification errors are delivered to the handler for custom
//! rendering rather than converted into a protocol acknowledgement.

use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};

use axum::body::to_bytes;
use axum::extract::{FromRef, FromRequest, Request};
use axum::handler::Handler;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, get};

use crate::error::Error;
use crate::notify::{NotifyData, NotifyLimits};
use crate::{Client, NOTIFY_FAIL, NOTIFY_SUCCESS, Params, ProtocolContext};

/// Route a handler for both `GET` and `POST`, as EPay callbacks require.
///
/// The gateway delivers `notify_url` and `return_url` with `GET` by default
/// and some forks use `POST`; registering only one method makes the gateway
/// see 405 and retry forever.
///
/// ```rust,no_run
/// use axum::Router;
/// use epay_sdk::axum::{NotifyAck, VerifiedNotify, callback_route};
/// use epay_sdk::ProtocolContext;
///
/// async fn notify(VerifiedNotify(_data): VerifiedNotify) -> NotifyAck {
///     NotifyAck::ok()
/// }
///
/// # fn app(epay: ProtocolContext) -> Router {
/// Router::new()
///     .route("/notify", callback_route(notify))
///     .with_state(epay)
/// # }
/// ```
pub fn callback_route<H, T, S>(handler: H) -> MethodRouter<S>
where
    H: Handler<T, S> + Clone,
    T: 'static,
    S: Clone + Send + Sync + 'static,
{
    get(handler.clone()).post(handler)
}

/// Resolve the payer IP for [`crate::PaymentRequest::client_ip_addr`].
///
/// With `trust_proxy_headers == true` the first address in `X-Forwarded-For`
/// (then `X-Real-IP`) wins; otherwise, or when neither parses, the TCP peer
/// address is used. Enable header trust **only** when every request reaches
/// this service through a reverse proxy you control and that overwrites
/// those headers; otherwise a client can spoof its IP.
///
/// ```rust,no_run
/// use std::net::SocketAddr;
/// use axum::extract::ConnectInfo;
/// use axum::http::HeaderMap;
/// use epay_sdk::axum::payer_ip;
///
/// async fn pay(headers: HeaderMap, ConnectInfo(peer): ConnectInfo<SocketAddr>) {
///     let ip = payer_ip(&headers, peer, true);
///     # let _ = ip;
/// }
/// ```
pub fn payer_ip(headers: &HeaderMap, peer: SocketAddr, trust_proxy_headers: bool) -> IpAddr {
    if trust_proxy_headers {
        for name in ["x-forwarded-for", "x-real-ip"] {
            let forwarded = headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(',').next())
                .map(str::trim)
                .and_then(|value| value.parse::<IpAddr>().ok());
            if let Some(ip) = forwarded {
                return ip;
            }
        }
    }
    peer.ip()
}

/// Authenticated and structurally valid EPay notification.
///
/// Requires `ProtocolContext: FromRef<S>`; [`Client`] satisfies this too.
///
/// Rejections are answered with `200 fail` and logged at **debug** level
/// only: the endpoint is public, so anything louder would let unauthenticated
/// traffic flood the logs. To count or sample rejections, extract
/// `Result<VerifiedNotify, NotifyRejection>` instead and handle
/// [`NotifyRejection`] in the handler (still returning [`NotifyAck::fail`]).
#[derive(Clone, Debug)]
pub struct VerifiedNotify(pub NotifyData);

/// Rejection of [`VerifiedNotify`]; always rendered as HTTP 200 `fail`.
#[derive(Debug)]
pub struct NotifyRejection(pub Error);

impl std::fmt::Display for NotifyRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "epay notify rejected: {}", self.0)
    }
}

impl std::error::Error for NotifyRejection {}

impl IntoResponse for NotifyRejection {
    fn into_response(self) -> Response {
        (StatusCode::OK, NOTIFY_FAIL).into_response()
    }
}

/// Body written back to EPay: `success` or `fail`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotifyAck(&'static str);

impl NotifyAck {
    /// `success`: stop retrying.
    pub fn ok() -> Self {
        Self(NOTIFY_SUCCESS)
    }

    /// `fail`: ask EPay to retry later.
    pub fn fail() -> Self {
        Self(NOTIFY_FAIL)
    }
}

impl From<crate::Result<()>> for NotifyAck {
    fn from(result: crate::Result<()>) -> Self {
        if result.is_ok() {
            Self::ok()
        } else {
            Self::fail()
        }
    }
}

impl IntoResponse for NotifyAck {
    fn into_response(self) -> Response {
        (StatusCode::OK, self.0).into_response()
    }
}

impl FromRef<Client> for ProtocolContext {
    fn from_ref(client: &Client) -> Self {
        client.protocol_context().clone()
    }
}

impl<S> FromRequest<S> for VerifiedNotify
where
    S: Send + Sync,
    ProtocolContext: FromRef<S>,
{
    type Rejection = NotifyRejection;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let protocol = ProtocolContext::from_ref(state);
        let params = read_notify_params(request).await.map_err(|error| {
            sdk_debug!("[epay-sdk] notify input rejected: {error}");
            NotifyRejection(error)
        })?;
        protocol
            .verify_notify(&params)
            .map(VerifiedNotify)
            .map_err(|error| {
                sdk_debug!("[epay-sdk] notify verification rejected: {error}");
                NotifyRejection(error)
            })
    }
}

/// Strictly parsed but **unverified** parameters whose rejection is
/// `200 fail`, for multi-merchant callback routing.
///
/// Use it when one service handles callbacks for several EPay accounts:
/// identify the account from the **path** (never from the unverified `pid`
/// in the payload), load that account's [`ProtocolContext`], then call
/// [`ProtocolContext::verify_notify`].
///
/// ```rust,no_run
/// use axum::extract::{Path, State};
/// use epay_sdk::axum::{NotifyAck, ParsedNotify};
/// use epay_sdk::ProtocolContext;
///
/// # #[derive(Clone)] struct Accounts;
/// # impl Accounts { fn protocol_for(&self, _id: &str) -> Option<ProtocolContext> { None } }
/// async fn notify(
///     State(accounts): State<Accounts>,
///     Path(account_id): Path<String>,
///     ParsedNotify(params): ParsedNotify,
/// ) -> NotifyAck {
///     let Some(epay) = accounts.protocol_for(&account_id) else {
///         return NotifyAck::fail();
///     };
///     match epay.verify_notify(&params).and_then(|data| data.try_into_success()) {
///         Ok(paid) => {
///             // idempotent state transition keyed on paid.data().out_trade_no
///             let _ = paid;
///             NotifyAck::ok()
///         }
///         Err(_) => NotifyAck::fail(),
///     }
/// }
/// ```
#[derive(Clone, Debug)]
pub struct ParsedNotify(pub Params);

impl<S> FromRequest<S> for ParsedNotify
where
    S: Send + Sync,
{
    type Rejection = NotifyRejection;

    async fn from_request(request: Request, _state: &S) -> Result<Self, Self::Rejection> {
        read_notify_params(request)
            .await
            .map(ParsedNotify)
            .map_err(|error| {
                sdk_debug!("[epay-sdk] notify input rejected: {error}");
                NotifyRejection(error)
            })
    }
}

/// Strictly parsed but **unverified** canonical parameter bag whose
/// rejection is HTTP 400.
///
/// Useful for diagnostics endpoints; for gateway callbacks prefer
/// [`VerifiedNotify`] or, for multi-merchant routing, [`ParsedNotify`].
#[derive(Clone, Debug)]
pub struct NotifyParams(pub Params);

/// Rejection of [`NotifyParams`]: HTTP 400.
#[derive(Debug)]
pub struct NotifyInputRejection(pub Error);

impl IntoResponse for NotifyInputRejection {
    fn into_response(self) -> Response {
        (StatusCode::BAD_REQUEST, "invalid epay parameters").into_response()
    }
}

impl<S> FromRequest<S> for NotifyParams
where
    S: Send + Sync,
{
    type Rejection = NotifyInputRejection;

    async fn from_request(request: Request, _state: &S) -> Result<Self, Self::Rejection> {
        read_notify_params(request)
            .await
            .map(NotifyParams)
            .map_err(NotifyInputRejection)
    }
}

/// Verified return-page result with handler-controlled rendering.
///
/// Extraction itself is infallible; `Err` means the return parameters were
/// unreadable, malformed, unauthenticated, or structurally invalid.
#[derive(Debug)]
pub struct VerifiedReturn(pub crate::Result<NotifyData>);

impl<S> FromRequest<S> for VerifiedReturn
where
    S: Send + Sync,
    ProtocolContext: FromRef<S>,
{
    type Rejection = Infallible;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let protocol = ProtocolContext::from_ref(state);
        let result = match read_notify_params(request).await {
            Ok(params) => protocol.verify_notify(&params),
            Err(error) => Err(error),
        };
        Ok(Self(result))
    }
}

async fn read_notify_params(request: Request) -> crate::Result<Params> {
    let limits = NotifyLimits::default();
    let (parts, body) = request.into_parts();
    let query = parts.uri.query().unwrap_or_default();
    if query.len() > limits.max_encoded_bytes {
        return Err(Error::notify(format!(
            "encoded notification exceeds {} bytes",
            limits.max_encoded_bytes
        )));
    }
    let remaining = limits.max_encoded_bytes - query.len();
    let bytes = to_bytes(body, remaining)
        .await
        .map_err(|_| Error::notify("notify body unreadable or too large"))?;
    if !bytes.is_empty() && !is_form_content_type(parts.headers.get(header::CONTENT_TYPE)) {
        return Err(Error::notify(
            "non-empty notify body must be application/x-www-form-urlencoded",
        ));
    }
    let body = if bytes.is_empty() {
        None
    } else {
        Some(
            std::str::from_utf8(&bytes)
                .map_err(|_| Error::notify("notify body is not valid UTF-8"))?,
        )
    };
    crate::notify::parse_raw_notify_params_with_limits(
        (!query.is_empty()).then_some(query),
        body,
        limits,
    )?
    .canonicalize()
}

fn is_form_content_type(value: Option<&axum::http::HeaderValue>) -> bool {
    value
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| {
            value
                .trim()
                .eq_ignore_ascii_case("application/x-www-form-urlencoded")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer() -> SocketAddr {
        "10.0.0.7:5000".parse().unwrap()
    }

    #[test]
    fn payer_ip_uses_peer_unless_proxy_headers_are_trusted() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "203.0.113.9, 10.0.0.1".parse().unwrap());
        headers.insert("x-real-ip", "198.51.100.2".parse().unwrap());
        assert_eq!(payer_ip(&headers, peer(), false).to_string(), "10.0.0.7");
        assert_eq!(payer_ip(&headers, peer(), true).to_string(), "203.0.113.9");

        headers.remove("x-forwarded-for");
        assert_eq!(payer_ip(&headers, peer(), true).to_string(), "198.51.100.2");

        headers.insert("x-real-ip", "not-an-ip".parse().unwrap());
        assert_eq!(payer_ip(&headers, peer(), true).to_string(), "10.0.0.7");

        headers.insert("x-forwarded-for", " ::1 ".parse().unwrap());
        assert_eq!(payer_ip(&headers, peer(), true).to_string(), "::1");
    }
}
