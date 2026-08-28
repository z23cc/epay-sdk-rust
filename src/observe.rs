//! Internal logging shims: diagnostics go to `tracing` when that feature is
//! enabled, otherwise to `log` — never both, so applications bridging `log`
//! into `tracing` do not see duplicate events.

#[cfg(feature = "tracing")]
macro_rules! sdk_debug {
    ($($arg:tt)*) => {
        tracing::debug!($($arg)*)
    };
}

#[cfg(not(feature = "tracing"))]
macro_rules! sdk_debug {
    ($($arg:tt)*) => {
        log::debug!($($arg)*)
    };
}

#[cfg(feature = "tracing")]
macro_rules! sdk_warn {
    ($($arg:tt)*) => {
        tracing::warn!($($arg)*)
    };
}

#[cfg(not(feature = "tracing"))]
macro_rules! sdk_warn {
    ($($arg:tt)*) => {
        log::warn!($($arg)*)
    };
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
