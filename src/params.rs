//! Sorted, log-safe parameter map used for signing and encoding.

use std::collections::BTreeMap;
use std::fmt;

/// Sorted EPay parameter map.
///
/// Keys are kept in ASCII order via [`BTreeMap`] so signing and query
/// encoding are deterministic. Empty values are dropped on insert, matching
/// the V1 sign spec (`sign` / `sign_type` / empty values are not signed).
///
/// `Debug` and `Display` redact `key` and `sign` so the map is safe to log.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Params {
    inner: BTreeMap<String, String>,
}

impl Params {
    /// Empty map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a parameter. Empty values are ignored.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) -> &mut Self {
        let key = key.into();
        let value = value.into();
        if !value.is_empty() {
            self.inner.insert(key, value);
        }
        self
    }

    /// Insert a parameter if `value` is `Some`; empty values are ignored.
    pub fn insert_opt(
        &mut self,
        key: impl Into<String>,
        value: Option<impl Into<String>>,
    ) -> &mut Self {
        if let Some(v) = value {
            self.insert(key, v);
        }
        self
    }

    /// Value for `key`, if present.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.inner.get(key).map(String::as_str)
    }

    /// Whether `key` is present.
    pub fn contains(&self, key: &str) -> bool {
        self.inner.contains_key(key)
    }

    /// Remove a parameter, returning its previous value.
    pub fn remove(&mut self, key: &str) -> Option<String> {
        self.inner.remove(key)
    }

    /// Iterate in ASCII key order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.inner.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Number of parameters.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the map is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Merge `other` into self; keys in `other` win.
    pub fn merge(&mut self, other: Params) -> &mut Self {
        self.inner.extend(other.inner);
        self
    }

    /// Lenient `application/x-www-form-urlencoded` parser (last duplicate
    /// wins, malformed escapes tolerated).
    ///
    /// Do **not** feed gateway notifications through this: use
    /// [`crate::parse_notify_params`], which rejects duplicates, conflicts
    /// and malformed encoding before anything is signed or verified.
    pub fn from_urlencoded(input: &str) -> Self {
        let mut params = Self::new();
        for (k, v) in url::form_urlencoded::parse(input.as_bytes()) {
            params.insert(k.into_owned(), v.into_owned());
        }
        params
    }

    /// Sign payload: skip empty (already), skip `sign` and `sign_type`.
    /// Values are **not** URL-encoded (彩虹易支付 V1 spec).
    pub fn sign_payload(&self) -> String {
        let mut parts = Vec::with_capacity(self.inner.len());
        for (k, v) in &self.inner {
            if k == "sign" || k == "sign_type" {
                continue;
            }
            parts.push(format!("{k}={v}"));
        }
        parts.join("&")
    }

    /// URL-encoded query string (`application/x-www-form-urlencoded`).
    pub fn encode(&self) -> String {
        let mut ser = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in &self.inner {
            ser.append_pair(k, v);
        }
        ser.finish()
    }

    /// Borrowed key/value pairs for form encoding.
    pub fn as_pairs(&self) -> Vec<(&str, &str)> {
        self.iter().collect()
    }

    /// Redact secrets for logs (`key`, `sign`).
    pub fn redacted_encode(&self) -> String {
        let mut ser = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in &self.inner {
            if is_secret(k) {
                ser.append_pair(k, "***");
            } else {
                ser.append_pair(k, &redacted_value(k, v));
            }
        }
        ser.finish()
    }
}

impl fmt::Display for Params {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.redacted_encode())
    }
}

/// Parameters that must never reach a log: the merchant key, and the
/// signature (which would make a logged URL replayable).
fn is_secret(key: &str) -> bool {
    matches!(key, "key" | "sign")
}

fn redacted_value(key: &str, value: &str) -> String {
    if key.ends_with("_url") {
        if let Ok(mut url) = url::Url::parse(value) {
            if url.query().is_some() {
                url.set_query(Some("***"));
            }
            return url.to_string();
        }
    }
    value.to_owned()
}

impl fmt::Debug for Params {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut map = f.debug_map();
        for (k, v) in &self.inner {
            if is_secret(k) {
                map.entry(k, &"***");
            } else {
                map.entry(k, &redacted_value(k, v));
            }
        }
        map.finish()
    }
}

impl<K, V> FromIterator<(K, V)> for Params
where
    K: Into<String>,
    V: Into<String>,
{
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut params = Self::new();
        for (k, v) in iter {
            params.insert(k, v);
        }
        params
    }
}

impl From<BTreeMap<String, String>> for Params {
    fn from(inner: BTreeMap<String, String>) -> Self {
        let mut params = Self::new();
        for (k, v) in inner {
            params.insert(k, v);
        }
        params
    }
}

impl From<std::collections::HashMap<String, String>> for Params {
    fn from(map: std::collections::HashMap<String, String>) -> Self {
        map.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_payload_sorts_and_skips() {
        let mut p = Params::new();
        p.insert("c", "3");
        p.insert("a", "1");
        p.insert("b", "2");
        p.insert("sign", "deadbeef");
        p.insert("sign_type", "MD5");
        p.insert("empty", "");
        assert_eq!(p.sign_payload(), "a=1&b=2&c=3");
    }

    #[test]
    fn debug_and_display_redact_secrets() {
        let mut p = Params::new();
        p.insert("pid", "1001");
        p.insert("key", "super-secret");
        p.insert("sign", "deadbeef");
        p.insert(
            "notify_url",
            "https://shop.example/notify?token=callback-secret",
        );
        for s in [format!("{p:?}"), p.to_string()] {
            assert!(!s.contains("super-secret"), "{s}");
            assert!(!s.contains("deadbeef"), "{s}");
            assert!(!s.contains("callback-secret"), "{s}");
            assert!(s.contains("1001"), "{s}");
        }
    }

    #[test]
    fn from_urlencoded() {
        let p = Params::from_urlencoded("pid=1001&out_trade_no=ORDER001&money=10.00");
        assert_eq!(p.get("pid"), Some("1001"));
        assert_eq!(p.get("out_trade_no"), Some("ORDER001"));
        assert_eq!(p.get("money"), Some("10.00"));
    }
}
