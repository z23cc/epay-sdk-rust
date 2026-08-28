//! Internal logging shims: every diagnostic goes to `log`, and additionally
//! to `tracing` when the `tracing` feature is enabled.

macro_rules! sdk_debug {
    ($($arg:tt)*) => {{
        log::debug!($($arg)*);
        #[cfg(feature = "tracing")]
        {
            tracing::debug!($($arg)*);
        }
    }};
}

macro_rules! sdk_warn {
    ($($arg:tt)*) => {{
        log::warn!($($arg)*);
        #[cfg(feature = "tracing")]
        {
            tracing::warn!($($arg)*);
        }
    }};
}

/// Wrap a gateway call in a `tracing` span when the feature is enabled.
#[cfg(feature = "tracing")]
pub(crate) fn instrument<F: std::future::Future>(
    endpoint: crate::endpoint::Endpoint,
    method: &'static str,
    future: F,
) -> tracing::instrument::Instrumented<F> {
    use tracing::Instrument;
    let span = tracing::debug_span!("epay.request", endpoint = %endpoint, method);
    future.instrument(span)
}

#[cfg(not(feature = "tracing"))]
pub(crate) fn instrument<F: std::future::Future>(
    _endpoint: crate::endpoint::Endpoint,
    _method: &'static str,
    future: F,
) -> F {
    future
}
