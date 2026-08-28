//! Gateway endpoints addressed by [`crate::Client`].

use std::fmt;

use crate::types::path;

/// HTTP method used by an endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Method {
    Get,
    Post,
}

/// 彩虹易支付 V1 gateway endpoint.
///
/// Carried by [`crate::Error::Api`] and [`crate::Error::InvalidResponse`] so
/// callers can tell which operation failed without parsing messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Endpoint {
    /// `POST mapi.php` — create an API payment.
    CreatePayment,
    /// `GET api.php?act=order` — query one order.
    QueryOrder,
    /// `GET api.php?act=orders` — list orders.
    QueryOrders,
    /// `POST api.php?act=refund` — refund an order.
    Refund,
    /// `GET api.php?act=query` — merchant profile and balance.
    QueryMerchant,
    /// `GET api.php?act=settle` — settlement records.
    QuerySettlements,
}

impl Endpoint {
    /// Gateway path relative to the API base URL.
    pub fn path(self) -> &'static str {
        match self {
            Self::CreatePayment => path::MAPI,
            _ => path::API,
        }
    }

    /// `act` query parameter for `api.php` endpoints.
    pub fn act(self) -> Option<&'static str> {
        match self {
            Self::CreatePayment => None,
            Self::QueryOrder => Some("order"),
            Self::QueryOrders => Some("orders"),
            Self::Refund => Some("refund"),
            Self::QueryMerchant => Some("query"),
            Self::QuerySettlements => Some("settle"),
        }
    }

    /// Whether a response `code` denotes success for this endpoint.
    ///
    /// Every endpoint answers `1` on success except refund, which the official
    /// implementation documents as `{"code":0,"msg":"退款成功"}`. Some forks
    /// answer `1` there, so both are accepted for [`Endpoint::Refund`].
    pub fn is_success_code(self, code: i32) -> bool {
        match self {
            Self::Refund => matches!(code, 0 | 1),
            _ => code == 1,
        }
    }

    /// Human-readable label used in error messages.
    pub fn label(self) -> &'static str {
        match self {
            Self::CreatePayment => "mapi.php",
            Self::QueryOrder => "api.php?act=order",
            Self::QueryOrders => "api.php?act=orders",
            Self::Refund => "api.php?act=refund",
            Self::QueryMerchant => "api.php?act=query",
            Self::QuerySettlements => "api.php?act=settle",
        }
    }

    pub(crate) fn method(self) -> Method {
        match self {
            Self::CreatePayment | Self::Refund => Method::Post,
            _ => Method::Get,
        }
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refund_accepts_official_zero_and_fork_one() {
        assert!(Endpoint::Refund.is_success_code(0));
        assert!(Endpoint::Refund.is_success_code(1));
        assert!(!Endpoint::Refund.is_success_code(-1));
        assert!(!Endpoint::QueryOrder.is_success_code(0));
        assert!(Endpoint::QueryOrder.is_success_code(1));
    }

    #[test]
    fn paths_methods_and_acts() {
        assert_eq!(Endpoint::CreatePayment.path(), "/mapi.php");
        assert_eq!(Endpoint::CreatePayment.act(), None);
        assert_eq!(Endpoint::CreatePayment.method(), Method::Post);
        assert_eq!(Endpoint::Refund.method(), Method::Post);
        assert_eq!(Endpoint::QueryOrders.method(), Method::Get);
        assert_eq!(Endpoint::QuerySettlements.act(), Some("settle"));
        assert_eq!(Endpoint::QueryMerchant.to_string(), "api.php?act=query");
    }
}
