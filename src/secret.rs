//! Merchant key storage: a single shared copy that is zeroized on drop.

use std::fmt;
use std::sync::Arc;

use zeroize::Zeroizing;

/// The merchant key, shared between [`crate::Config`] and [`crate::Signer`].
///
/// Cloning shares the same allocation; the buffer is wiped when the last
/// clone is dropped. `Debug` never reveals the value.
#[derive(Clone)]
pub(crate) struct MerchantKey(Arc<Zeroizing<String>>);

impl MerchantKey {
    pub(crate) fn new(key: String) -> Self {
        Self(Arc::new(Zeroizing::new(key)))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for MerchantKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_and_debug_redacts() {
        let key = MerchantKey::new("super-secret".to_owned());
        let clone = key.clone();
        assert!(Arc::ptr_eq(&key.0, &clone.0));
        assert_eq!(clone.as_str(), "super-secret");
        assert!(!format!("{key:?}").contains("super"));
        assert!(MerchantKey::new(String::new()).is_empty());
    }
}
