//! HTTP transport abstraction, the built-in reqwest transport and a mock.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use url::Url;

use crate::error::Result;

/// Boxed future used by [`Transport`].
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Per-request settings handed to a [`Transport`]; construct with
/// [`TransportOptions::new`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct TransportOptions {
    /// Stop reading once the body would exceed this many bytes.
    pub max_response_bytes: usize,
    /// Per-attempt timeout for this call; `None` keeps the transport's own
    /// default.
    pub timeout: Option<Duration>,
}

impl TransportOptions {
    /// Options with a response limit and no per-request timeout.
    pub fn new(max_response_bytes: usize) -> Self {
        Self {
            max_response_bytes,
            timeout: None,
        }
    }

    /// Set a per-request timeout.
    pub fn with_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout;
        self
    }
}

/// HTTP transport used by [`crate::Client`].
///
/// Implementations must stop reading once `options.max_response_bytes` would
/// be exceeded and return [`crate::Error::ResponseTooLarge`]; `Client` also
/// checks the returned length as defense in depth. Errors should be built
/// with [`crate::Error::transport`] so request URLs (which carry the merchant
/// key on GET) never reach logs.
pub trait Transport: Send + Sync {
    /// Perform a GET and return the raw body of a 2xx response.
    fn get<'a>(&'a self, url: &'a Url, options: TransportOptions)
    -> BoxFuture<'a, Result<Vec<u8>>>;

    /// POST `form` as `application/x-www-form-urlencoded` and return the raw
    /// body of a 2xx response.
    fn post_form<'a>(
        &'a self,
        url: &'a Url,
        form: &'a [(&'a str, &'a str)],
        options: TransportOptions,
    ) -> BoxFuture<'a, Result<Vec<u8>>>;

    /// Asynchronous sleep used for retry back-off, e.g.
    /// `Box::pin(tokio::time::sleep(duration))`.
    fn sleep(&self, duration: Duration) -> BoxFuture<'_, ()>;
}

#[cfg(any(feature = "reqwest-rustls", feature = "reqwest-native-tls"))]
pub(crate) use reqwest_impl::ReqwestTransport;

#[cfg(any(feature = "reqwest-rustls", feature = "reqwest-native-tls"))]
mod reqwest_impl {
    use std::time::Duration;

    use url::Url;

    use super::{BoxFuture, Transport, TransportOptions};
    use crate::error::{Error, Result, TransportErrorKind};

    #[derive(Clone)]
    pub struct ReqwestTransport {
        client: reqwest::Client,
    }

    impl ReqwestTransport {
        pub fn new(timeout: Duration, connect_timeout: Option<Duration>) -> Result<Self> {
            let mut builder = reqwest::Client::builder()
                .timeout(timeout)
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(concat!("epay-sdk-rust/", env!("CARGO_PKG_VERSION")));
            if let Some(connect_timeout) = connect_timeout {
                builder = builder.connect_timeout(connect_timeout);
            }
            #[cfg(feature = "reqwest-rustls")]
            let builder = builder.use_rustls_tls();
            #[cfg(all(feature = "reqwest-native-tls", not(feature = "reqwest-rustls")))]
            let builder = builder.use_native_tls();
            let client = builder.build().map_err(map_reqwest_error)?;
            Ok(Self { client })
        }

        /// Caller-supplied clients retain their redirect/TLS policy. Response
        /// limits, per-request timeouts and URL-free error classification are
        /// still enforced.
        pub fn from_client(client: reqwest::Client) -> Self {
            Self { client }
        }

        async fn send(
            request: reqwest::RequestBuilder,
            options: TransportOptions,
        ) -> Result<Vec<u8>> {
            let request = match options.timeout {
                Some(timeout) => request.timeout(timeout),
                None => request,
            };
            let response = request.send().await.map_err(map_reqwest_error)?;
            Self::read_response(response, options.max_response_bytes).await
        }

        async fn read_response(
            mut response: reqwest::Response,
            max_response_bytes: usize,
        ) -> Result<Vec<u8>> {
            let status = response.status();
            if response
                .content_length()
                .is_some_and(|length| length > max_response_bytes as u64)
            {
                return Err(Error::ResponseTooLarge {
                    limit: max_response_bytes,
                });
            }
            let mut body = Vec::with_capacity(
                response
                    .content_length()
                    .unwrap_or(0)
                    .min(max_response_bytes as u64) as usize,
            );
            while let Some(chunk) = response.chunk().await.map_err(map_reqwest_error)? {
                if body.len().saturating_add(chunk.len()) > max_response_bytes {
                    return Err(Error::ResponseTooLarge {
                        limit: max_response_bytes,
                    });
                }
                body.extend_from_slice(&chunk);
            }
            if !status.is_success() {
                return Err(Error::http(status.as_u16(), &body));
            }
            Ok(body)
        }
    }

    impl Transport for ReqwestTransport {
        fn get<'a>(
            &'a self,
            url: &'a Url,
            options: TransportOptions,
        ) -> BoxFuture<'a, Result<Vec<u8>>> {
            Box::pin(Self::send(self.client.get(url.clone()), options))
        }

        fn post_form<'a>(
            &'a self,
            url: &'a Url,
            form: &'a [(&'a str, &'a str)],
            options: TransportOptions,
        ) -> BoxFuture<'a, Result<Vec<u8>>> {
            Box::pin(Self::send(
                self.client.post(url.clone()).form(form),
                options,
            ))
        }

        fn sleep(&self, duration: Duration) -> BoxFuture<'_, ()> {
            Box::pin(tokio::time::sleep(duration))
        }
    }

    /// Classify a reqwest error without retaining its (URL-bearing) text.
    fn map_reqwest_error(error: reqwest::Error) -> Error {
        let (kind, message) = if error.is_timeout() {
            (TransportErrorKind::Timeout, "request timed out")
        } else if error.is_connect() {
            (TransportErrorKind::Connect, "connection failed")
        } else if error.is_redirect() {
            (TransportErrorKind::Redirect, "redirect rejected")
        } else if error.is_body() || error.is_decode() {
            (TransportErrorKind::Body, "response body read failed")
        } else if error.is_builder() || error.is_request() {
            (
                TransportErrorKind::Request,
                "request could not be built or sent",
            )
        } else {
            (TransportErrorKind::Other, "request failed")
        };
        Error::transport(kind, message)
    }
}

#[cfg(any(test, feature = "test-util"))]
pub use mock::{MockTransport, RecordedRequest};

#[cfg(any(test, feature = "test-util"))]
mod mock {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use url::Url;

    use super::{BoxFuture, Transport, TransportOptions};
    use crate::error::{Error, Result, TransportErrorKind};
    use crate::params::Params;

    /// Scripted transport for tests: responses are served in FIFO order and
    /// every request is recorded. Retry back-off sleeps are recorded instead
    /// of waited for.
    #[derive(Clone, Default)]
    pub struct MockTransport {
        inner: Arc<MockInner>,
    }

    #[derive(Default)]
    struct MockInner {
        responses: Mutex<VecDeque<Result<Vec<u8>>>>,
        requests: Mutex<Vec<RecordedRequest>>,
        sleeps: Mutex<Vec<Duration>>,
    }

    /// A request observed by [`MockTransport`].
    ///
    /// The query string and form are kept as [`Params`], whose `Debug`
    /// output masks `key` / `sign`, so a failing test never prints a real
    /// merchant key; assert on them with [`Params::get`].
    #[derive(Clone, Debug)]
    pub struct RecordedRequest {
        /// `GET` or `POST`.
        pub method: &'static str,
        /// Request URL without its query string.
        pub url: String,
        /// Decoded query parameters.
        pub query: Params,
        /// Form parameters for POST requests; empty for GET.
        pub form: Params,
        /// Options the client passed for this request.
        pub options: TransportOptions,
    }

    impl std::fmt::Debug for MockTransport {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let remaining = self
                .inner
                .responses
                .lock()
                .map(|queue| queue.len())
                .unwrap_or(0);
            f.debug_struct("MockTransport")
                .field("queued_responses", &remaining)
                .finish()
        }
    }

    impl MockTransport {
        /// Empty transport; every request fails until a response is queued.
        pub fn new() -> Self {
            Self::default()
        }

        /// Queue a successful response body.
        pub fn push_json(self, body: impl Into<String>) -> Self {
            self.push_bytes(body.into().into_bytes())
        }

        /// Queue a successful raw response body.
        pub fn push_bytes(self, body: Vec<u8>) -> Self {
            self.inner
                .responses
                .lock()
                .expect("mock transport mutex")
                .push_back(Ok(body));
            self
        }

        /// Queue a transport failure.
        pub fn push_err(self, err: Error) -> Self {
            self.inner
                .responses
                .lock()
                .expect("mock transport mutex")
                .push_back(Err(err));
            self
        }

        /// Requests observed so far, in order.
        pub fn recorded(&self) -> Vec<RecordedRequest> {
            self.inner
                .requests
                .lock()
                .expect("mock transport mutex")
                .clone()
        }

        /// Back-off sleeps requested by the client, in order.
        pub fn recorded_sleeps(&self) -> Vec<Duration> {
            self.inner
                .sleeps
                .lock()
                .expect("mock transport mutex")
                .clone()
        }

        fn next(
            &self,
            method: &'static str,
            url: &Url,
            form: Params,
            options: TransportOptions,
        ) -> Result<Vec<u8>> {
            let query: Params = url.query_pairs().collect();
            let mut bare = url.clone();
            bare.set_query(None);
            self.inner
                .requests
                .lock()
                .expect("mock transport mutex")
                .push(RecordedRequest {
                    method,
                    url: bare.to_string(),
                    query,
                    form,
                    options,
                });
            let response = self
                .inner
                .responses
                .lock()
                .expect("mock transport mutex")
                .pop_front()
                .unwrap_or_else(|| {
                    Err(Error::transport(
                        TransportErrorKind::Other,
                        "mock transport has no scripted response",
                    ))
                })?;
            if response.len() > options.max_response_bytes {
                return Err(Error::ResponseTooLarge {
                    limit: options.max_response_bytes,
                });
            }
            Ok(response)
        }
    }

    impl Transport for MockTransport {
        fn get<'a>(
            &'a self,
            url: &'a Url,
            options: TransportOptions,
        ) -> BoxFuture<'a, Result<Vec<u8>>> {
            Box::pin(async move { self.next("GET", url, Params::new(), options) })
        }

        fn post_form<'a>(
            &'a self,
            url: &'a Url,
            form: &'a [(&'a str, &'a str)],
            options: TransportOptions,
        ) -> BoxFuture<'a, Result<Vec<u8>>> {
            let form: Params = form.iter().copied().collect();
            Box::pin(async move { self.next("POST", url, form, options) })
        }

        fn sleep(&self, duration: Duration) -> BoxFuture<'_, ()> {
            self.inner
                .sleeps
                .lock()
                .expect("mock transport mutex")
                .push(duration);
            Box::pin(async {})
        }
    }
}
