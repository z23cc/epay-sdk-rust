//! Test helpers (feature `test-util`): a scripted [`MockTransport`] and a
//! [`NotifyFixture`] that produces correctly signed gateway notifications.

pub use crate::transport::{MockTransport, RecordedRequest};

use crate::money::Money;
use crate::params::Params;
use crate::signer::Signer;
use crate::types::{PayType, TradeStatus};

/// Builder for a signed `notify_url` / `return_url` payload.
///
/// Defaults: `trade_no = "20260828000000000001"`, `type = alipay`,
/// `name = "Test Product"`, `trade_status = TRADE_SUCCESS`, no `param`.
///
/// ```rust
/// use epay_sdk::test_util::NotifyFixture;
/// use epay_sdk::{Money, ProtocolContext};
///
/// # fn main() -> epay_sdk::Result<()> {
/// let epay = ProtocolContext::new(1001, "testkey123", "https://pay.example.com")?;
/// let query = NotifyFixture::new(1001, "testkey123", "ORDER001", Money::from_yuan_str("9.90")?)
///     .query();
/// let params = epay_sdk::parse_notify_params(Some(&query), None)?;
/// assert!(epay.verify_notify(&params)?.is_success());
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct NotifyFixture {
    pid: u64,
    key: String,
    trade_no: String,
    out_trade_no: String,
    pay_type: PayType,
    name: String,
    money: Money,
    trade_status: TradeStatus,
    param: Option<String>,
}

impl NotifyFixture {
    /// Fixture for a successful payment of `money` for `out_trade_no`.
    pub fn new(
        pid: u64,
        key: impl Into<String>,
        out_trade_no: impl Into<String>,
        money: Money,
    ) -> Self {
        Self {
            pid,
            key: key.into(),
            trade_no: "20260828000000000001".to_owned(),
            out_trade_no: out_trade_no.into(),
            pay_type: PayType::Alipay,
            name: "Test Product".to_owned(),
            money,
            trade_status: TradeStatus::from(TradeStatus::SUCCESS),
            param: None,
        }
    }

    /// EPay order number.
    pub fn trade_no(mut self, trade_no: impl Into<String>) -> Self {
        self.trade_no = trade_no.into();
        self
    }

    /// Payment channel.
    pub fn pay_type(mut self, pay_type: impl Into<PayType>) -> Self {
        self.pay_type = pay_type.into();
        self
    }

    /// Product name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Trade status, e.g. to simulate a non-success notification.
    pub fn trade_status(mut self, status: impl Into<TradeStatus>) -> Self {
        self.trade_status = status.into();
        self
    }

    /// Opaque merchant data.
    pub fn param(mut self, param: impl Into<String>) -> Self {
        self.param = Some(param.into());
        self
    }

    /// Unsigned parameters, for tests that tamper before signing.
    pub fn unsigned_params(&self) -> Params {
        let mut params = Params::new();
        params.insert("pid", self.pid.to_string());
        params.insert("trade_no", self.trade_no.clone());
        params.insert("out_trade_no", self.out_trade_no.clone());
        params.insert("type", self.pay_type.as_str());
        params.insert("name", self.name.clone());
        params.insert("money", self.money.to_epay_string());
        params.insert("trade_status", self.trade_status.0.clone());
        params.insert_opt("param", self.param.clone());
        params
    }

    /// Signed parameters (`sign` + `sign_type=MD5`).
    ///
    /// # Panics
    ///
    /// If `key` is empty.
    pub fn params(&self) -> Params {
        Signer::new(self.key.clone())
            .expect("NotifyFixture key must not be empty")
            .sign_with_params(self.unsigned_params())
    }

    /// Signed parameters as a URL query string / form body.
    pub fn query(&self) -> String {
        self.params().encode()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use crate::notify::{parse_notify_params, verify_notify};

    fn fixture() -> NotifyFixture {
        NotifyFixture::new(
            1001,
            "testkey123",
            "ORDER001",
            Money::from_yuan_str("9.90").unwrap(),
        )
    }

    #[test]
    fn fixture_verifies_and_carries_overrides() {
        let query = fixture()
            .trade_no("T42")
            .pay_type(PayType::Wxpay)
            .name("VIP")
            .param("u=7")
            .query();
        let params = parse_notify_params(Some(&query), None).unwrap();
        let signer = Signer::new("testkey123").unwrap();
        let data = verify_notify(&signer, 1001, &params).unwrap();
        assert!(data.is_success());
        assert_eq!(data.trade_no, "T42");
        assert_eq!(data.pay_type, PayType::Wxpay);
        assert_eq!(data.name, "VIP");
        assert_eq!(data.param, "u=7");
        assert_eq!(data.money.to_epay_string(), "9.90");
    }

    #[test]
    fn fixture_supports_failure_and_tampering_scenarios() {
        let signer = Signer::new("testkey123").unwrap();
        let pending = fixture().trade_status("WAIT_BUYER_PAY").params();
        assert!(!verify_notify(&signer, 1001, &pending).unwrap().is_success());

        let mut tampered = fixture().params();
        tampered.insert("money", "0.01");
        assert!(matches!(
            verify_notify(&signer, 1001, &tampered),
            Err(Error::VerifyFailed)
        ));

        let mut unsigned = fixture().unsigned_params();
        assert!(unsigned.get("sign").is_none());
        unsigned.insert("sign", "0".repeat(32));
        assert!(matches!(
            verify_notify(&signer, 1001, &unsigned),
            Err(Error::Notify { .. }) | Err(Error::VerifyFailed)
        ));
    }
}
