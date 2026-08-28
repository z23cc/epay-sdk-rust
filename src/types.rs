//! Protocol enums and constants.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Payment channel.
///
/// Unknown channels offered by a fork are preserved in [`PayType::Other`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum PayType {
    /// 支付宝.
    Alipay,
    /// 微信支付.
    Wxpay,
    /// QQ 钱包.
    Qqpay,
    /// Any other channel identifier.
    #[serde(untagged)]
    Other(String),
}

impl PayType {
    /// Wire value of [`PayType::Alipay`].
    pub const ALIPAY: &'static str = "alipay";
    /// Wire value of [`PayType::Wxpay`].
    pub const WXPAY: &'static str = "wxpay";
    /// Wire value of [`PayType::Qqpay`].
    pub const QQPAY: &'static str = "qqpay";

    /// Wire value sent as `type`.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Alipay => Self::ALIPAY,
            Self::Wxpay => Self::WXPAY,
            Self::Qqpay => Self::QQPAY,
            Self::Other(s) => s,
        }
    }

    /// Chinese display name for the well-known channels.
    pub fn display_name(&self) -> &str {
        match self {
            Self::Alipay => "支付宝",
            Self::Wxpay => "微信支付",
            Self::Qqpay => "QQ钱包",
            Self::Other(s) => s,
        }
    }
}

impl fmt::Display for PayType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<&str> for PayType {
    fn from(s: &str) -> Self {
        match s {
            Self::ALIPAY => Self::Alipay,
            Self::WXPAY => Self::Wxpay,
            Self::QQPAY => Self::Qqpay,
            other => Self::Other(other.to_owned()),
        }
    }
}

impl From<String> for PayType {
    fn from(s: String) -> Self {
        PayType::from(s.as_str())
    }
}

/// Device hint sent to `mapi.php`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Device {
    /// Desktop browser.
    Pc,
    /// Mobile browser.
    Mobile,
    /// WeChat in-app browser.
    Wechat,
    /// Alipay in-app browser.
    Alipay,
    /// Any other device identifier.
    #[serde(untagged)]
    Other(String),
}

impl Device {
    /// Wire value sent as `device`.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Pc => "pc",
            Self::Mobile => "mobile",
            Self::Wechat => "wechat",
            Self::Alipay => "alipay",
            Self::Other(s) => s,
        }
    }
}

impl fmt::Display for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<&str> for Device {
    fn from(s: &str) -> Self {
        match s {
            "pc" => Self::Pc,
            "mobile" => Self::Mobile,
            "wechat" => Self::Wechat,
            "alipay" => Self::Alipay,
            other => Self::Other(other.to_owned()),
        }
    }
}

/// Asynchronous notify trade status (`trade_status`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TradeStatus(pub String);

impl TradeStatus {
    /// The only status that denotes a completed payment.
    pub const SUCCESS: &'static str = "TRADE_SUCCESS";

    /// Exact, case-sensitive comparison against [`TradeStatus::SUCCESS`].
    pub fn is_success(&self) -> bool {
        self.0 == Self::SUCCESS
    }
}

impl fmt::Display for TradeStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for TradeStatus {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// Order payment status returned by `act=order`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum OrderStatus {
    /// `0` — not paid.
    Unpaid,
    /// `1` — paid.
    Paid,
    /// Any other fork-specific value.
    Other(i32),
}

impl OrderStatus {
    /// Map a wire `status` integer.
    pub fn from_i32(v: i32) -> Self {
        match v {
            0 => Self::Unpaid,
            1 => Self::Paid,
            other => Self::Other(other),
        }
    }

    /// Whether the order is paid.
    pub fn is_paid(self) -> bool {
        matches!(self, Self::Paid)
    }

    /// Wire `status` integer.
    pub fn as_i32(self) -> i32 {
        match self {
            Self::Unpaid => 0,
            Self::Paid => 1,
            Self::Other(v) => v,
        }
    }
}

/// Known V1 API paths.
pub mod path {
    /// API payment endpoint.
    pub const MAPI: &str = "/mapi.php";
    /// Browser form/redirect endpoint.
    pub const SUBMIT: &str = "/submit.php";
    /// Merchant query/refund endpoint.
    pub const API: &str = "/api.php";
}

/// Body that EPay expects from `notify_url`.
pub const NOTIFY_SUCCESS: &str = "success";
/// Body that causes EPay to retry the notify.
pub const NOTIFY_FAIL: &str = "fail";
