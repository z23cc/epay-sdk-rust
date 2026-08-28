//! EPay (易支付) SDK for Rust.
//!
//! The protocol core is transport-free. Use [`ProtocolContext`] for signed
//! forms and callback verification, and [`Client`] when making async EPay API
//! requests. The optional `axum` feature adds strict extractors without
//! enabling reqwest.
//!
//! ```no_run
//! use epay_sdk::{FormPaymentRequest, Money, PayType, ProtocolContext};
//!
//! # fn demo() -> epay_sdk::Result<()> {
//! let epay = ProtocolContext::builder(1001, "merchant-key", "https://pay.example.com")
//!     .notify_url("https://shop.example.com/notify")
//!     .return_url("https://shop.example.com/return")
//!     .build()?;
//! let html = epay.build_form_payment(
//!     &FormPaymentRequest::new("ORDER001", "VIP会员", Money::from_yuan_str("9.90")?)
//!         .pay_type(PayType::Alipay),
//! )?;
//! # let _ = html;
//! # Ok(())
//! # }
//! ```
//!
//! # Feature flags
//!
//! | Feature | Default | Effect |
//! |---|---|---|
//! | `reqwest-rustls` | yes | built-in transport with rustls |
//! | `reqwest-native-tls` | no | built-in transport with native TLS |
//! | `axum` | no | Axum 0.8 extractors ([`axum`]) |
//! | `blocking` | no | [`blocking::Client`] for non-async programs |
//! | `tracing` | no | `tracing` spans/events for every gateway call (in addition to `log`) |
//! | `test-util` | no | [`test_util`]: [`MockTransport`], [`test_util::NotifyFixture`] |

#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(missing_debug_implementations)]
#![warn(missing_docs)]

#[macro_use]
mod observe;

#[cfg(feature = "blocking")]
#[cfg_attr(docsrs, doc(cfg(feature = "blocking")))]
pub mod blocking;
pub mod client;
pub mod config;
pub mod endpoint;
pub mod error;
pub mod limits;
pub mod merchant;
pub mod money;
pub mod notify;
pub mod order;
pub mod params;
pub mod payment;
pub mod protocol;
pub mod refund;
pub mod signer;
#[cfg(any(test, feature = "test-util"))]
#[cfg_attr(docsrs, doc(cfg(feature = "test-util")))]
pub mod test_util;
pub mod transport;
pub mod types;

mod integrations;
mod secret;
mod serde_util;
mod validation;

pub use client::{CallOptions, Client, ClientBuilder};
pub use config::{Config, HttpConfig, RetryPolicy};
pub use endpoint::Endpoint;
pub use error::{Error, Result, TransportErrorKind};
pub use merchant::{MerchantInfo, Settlement, SettlementListResponse};
pub use money::Money;
pub use notify::{
    NotifyData, NotifyLimits, NotifySource, RawNotifyEntry, RawNotifyParams, SuccessfulNotify,
    parse_notify_params, parse_raw_notify_params, parse_raw_notify_params_with_limits,
};
pub use order::{OrderDetail, OrderListResponse, OrderQueryRequest, generate_out_trade_no};
pub use params::Params;
pub use payment::{FormPaymentRequest, PaymentRequest, PaymentResponse};
pub use protocol::{ProtocolContext, ProtocolContextBuilder};
pub use refund::{RefundRequest, RefundResponse};
pub use signer::Signer;
pub use transport::{Transport, TransportOptions};
pub use types::{Device, NOTIFY_FAIL, NOTIFY_SUCCESS, OrderStatus, PayType, TradeStatus};
pub use url::Url;

#[cfg(any(test, feature = "test-util"))]
#[cfg_attr(docsrs, doc(cfg(feature = "test-util")))]
pub use transport::{MockTransport, RecordedRequest};

#[cfg(feature = "axum")]
#[cfg_attr(docsrs, doc(cfg(feature = "axum")))]
pub use integrations::axum;

/// Compile the README examples as doctests (`cargo test --all-features`).
#[cfg(all(doctest, feature = "axum", feature = "test-util"))]
#[doc = include_str!("../README.md")]
mod readme_doctests {}

/// Compile the guide examples as doctests (`cargo test --all-features`).
#[cfg(all(doctest, feature = "axum", feature = "test-util"))]
#[doc = include_str!("../docs/GUIDE.md")]
mod guide_doctests {}

/// Commonly used protocol and client types.
pub mod prelude {
    pub use crate::{
        Client, Error, FormPaymentRequest, Money, NOTIFY_FAIL, NOTIFY_SUCCESS, NotifyData,
        OrderDetail, OrderQueryRequest, PayType, PaymentRequest, PaymentResponse, ProtocolContext,
        RefundRequest, Result, SuccessfulNotify,
    };

    #[cfg(feature = "axum")]
    pub use crate::axum::{NotifyAck, VerifiedNotify, VerifiedReturn};
}
