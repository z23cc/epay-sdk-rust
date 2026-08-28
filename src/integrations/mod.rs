//! Optional web-framework extractors and notify helpers.
//!
//! Enable with Cargo features:
//! - `axum` — Axum 0.8 `FromRequest` extractor
//!
//! Other frameworks: read bounded query/form input, call the fallible
//! [`crate::parse_notify_params`], then [`crate::ProtocolContext::verify_notify`].

#[cfg(feature = "axum")]
#[cfg_attr(docsrs, doc(cfg(feature = "axum")))]
pub mod axum;
