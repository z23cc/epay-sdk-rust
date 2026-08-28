//! MD5 signing and constant-time verification (彩虹易支付 V1).

use md5::{Digest, Md5};

use crate::error::{Error, Result};
use crate::params::Params;
use crate::secret::MerchantKey;

/// Default `sign_type` for 彩虹易支付 V1.
pub const SIGN_TYPE_MD5: &str = "MD5";

/// MD5 signer used by EPay V1.
///
/// The algorithm is protocol-mandated: skip `sign`, `sign_type`, and empty
/// values; sort keys; join unencoded `k=v` pairs; append the merchant key; MD5.
#[derive(Clone)]
pub struct Signer {
    key: MerchantKey,
}

impl std::fmt::Debug for Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Signer").field("key", &"***").finish()
    }
}

impl Signer {
    /// Bind a merchant key; empty keys are rejected.
    pub fn new(key: impl Into<String>) -> Result<Self> {
        Self::from_key(MerchantKey::new(key.into()))
    }

    pub(crate) fn from_key(key: MerchantKey) -> Result<Self> {
        if key.is_empty() {
            return Err(Error::INVALID_KEY);
        }
        Ok(Self { key })
    }

    /// Lower-case hex MD5 of the canonical payload plus the key.
    pub fn sign(&self, params: &Params) -> String {
        let payload = params.sign_payload();
        let mut hasher = Md5::new();
        hasher.update(payload.as_bytes());
        hasher.update(self.key.as_str().as_bytes());
        hex_lower(&hasher.finalize())
    }

    /// Constant-time compare against an exact 32-character ASCII-hex MD5.
    pub fn verify(&self, params: &Params, received: &str) -> bool {
        if received.len() != 32 || !received.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return false;
        }
        let expected = self.sign(params);
        let received = received.to_ascii_lowercase();
        constant_time_eq::constant_time_eq(expected.as_bytes(), received.as_bytes())
    }

    /// Add `sign` and `sign_type=MD5` to `params`.
    pub fn sign_with_params(&self, mut params: Params) -> Params {
        let sign = self.sign(&params);
        params.insert("sign", sign);
        params.insert("sign_type", SIGN_TYPE_MD5);
        params
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_params() -> Params {
        let mut params = Params::new();
        params.insert("pid", "1001");
        params.insert("type", "alipay");
        params.insert("out_trade_no", "ORDER001");
        params.insert("notify_url", "https://example.com/notify");
        params.insert("name", "Test Product");
        params.insert("money", "10.00");
        params
    }

    #[test]
    fn rejects_empty_key() {
        assert!(Signer::new("").is_err());
    }

    #[test]
    fn golden_md5() {
        let signer = Signer::new("testkey123").unwrap();
        assert_eq!(
            signer.sign(&sample_params()),
            "22af0227eebf7a4095b64af5fa89ca6d"
        );
    }

    #[test]
    fn verify_is_case_insensitive_but_not_whitespace_tolerant() {
        let signer = Signer::new("testkey123").unwrap();
        let params = sample_params();
        let sign = signer.sign(&params);
        assert!(signer.verify(&params, &sign));
        assert!(signer.verify(&params, &sign.to_uppercase()));
        assert!(!signer.verify(&params, &format!(" {sign}")));
        assert!(!signer.verify(&params, "gggggggggggggggggggggggggggggggg"));
    }
}
