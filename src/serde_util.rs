//! Lenient deserializers for PHP-shaped JSON.
//!
//! EPay gateways are PHP applications: integers arrive as numbers, strings
//! or `1.0`; nullable database columns arrive as explicit `null`; empty
//! result sets may be `null` instead of `[]`. Every wire struct in this crate
//! uses these helpers so one fork's quirk never becomes a hard failure.

use serde::Deserialize;
use serde::de::{self, Deserializer};

/// Deserialize a nullable wire value, mapping JSON `null` to `T::default()`.
///
/// `#[serde(default)]` only handles a missing member; PHP-backed EPay forks
/// commonly serialize nullable database columns as an explicit `null`.
/// Always pair with `#[serde(default)]` so a missing member is accepted too.
pub fn default_on_null<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

/// Deserialize an `i32` from a number, numeric string, integral float, `""`
/// or `null` (the latter two map to `0`).
pub fn flex_i32<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: Deserializer<'de>,
{
    let v = flex_i64(deserializer)?;
    i32::try_from(v)
        .map_err(|_| <D::Error as de::Error>::custom(format!("integer {v} out of range for i32")))
}

/// Deserialize an `i64` with the same leniency as [`flex_i32`].
pub fn flex_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_any(IntVisitor)
}

/// Deserialize a `u64` with the same leniency as [`flex_i32`]; negative
/// values are rejected.
pub fn flex_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_any(UIntVisitor)
}

/// PHP occasionally emits `1.0` for an integer column; accept that, but
/// reject fractional, non-finite and out-of-range values.
fn integral_f64<E: de::Error>(v: f64) -> Result<i128, E> {
    if v.is_finite() && v.fract() == 0.0 {
        Ok(v as i128)
    } else {
        Err(E::custom(format!("expected an integer, found {v}")))
    }
}

struct IntVisitor;

impl<'de> de::Visitor<'de> for IntVisitor {
    type Value = i64;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("an integer or a string integer")
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<i64, E> {
        Ok(v)
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<i64, E> {
        i64::try_from(v).map_err(E::custom)
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<i64, E> {
        if v.is_empty() {
            return Ok(0);
        }
        v.parse().map_err(E::custom)
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<i64, E> {
        self.visit_str(&v)
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<i64, E> {
        i64::try_from(integral_f64(v)?).map_err(E::custom)
    }

    fn visit_none<E: de::Error>(self) -> Result<i64, E> {
        Ok(0)
    }

    fn visit_unit<E: de::Error>(self) -> Result<i64, E> {
        Ok(0)
    }
}

struct UIntVisitor;

impl<'de> de::Visitor<'de> for UIntVisitor {
    type Value = u64;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("an unsigned integer or a string integer")
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<u64, E> {
        u64::try_from(v).map_err(E::custom)
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<u64, E> {
        Ok(v)
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<u64, E> {
        if v.is_empty() {
            return Ok(0);
        }
        v.parse().map_err(E::custom)
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<u64, E> {
        self.visit_str(&v)
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<u64, E> {
        u64::try_from(integral_f64(v)?).map_err(E::custom)
    }

    fn visit_none<E: de::Error>(self) -> Result<u64, E> {
        Ok(0)
    }

    fn visit_unit<E: de::Error>(self) -> Result<u64, E> {
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    struct I {
        #[serde(deserialize_with = "flex_i32")]
        v: i32,
    }

    #[derive(Deserialize)]
    struct U {
        #[serde(deserialize_with = "flex_u64")]
        v: u64,
    }

    #[derive(Deserialize)]
    struct S {
        #[serde(default, deserialize_with = "default_on_null")]
        v: String,
    }

    #[derive(Deserialize)]
    struct L {
        #[serde(default, deserialize_with = "default_on_null")]
        v: Vec<i32>,
    }

    fn i(json: &str) -> Result<i32, serde_json::Error> {
        serde_json::from_str::<I>(json).map(|x| x.v)
    }

    fn u(json: &str) -> Result<u64, serde_json::Error> {
        serde_json::from_str::<U>(json).map(|x| x.v)
    }

    #[test]
    fn accepts_ints_strings_nulls_and_integral_floats() {
        assert_eq!(i(r#"{"v":1}"#).unwrap(), 1);
        assert_eq!(i(r#"{"v":"-1"}"#).unwrap(), -1);
        assert_eq!(i(r#"{"v":""}"#).unwrap(), 0);
        assert_eq!(i(r#"{"v":null}"#).unwrap(), 0);
        assert_eq!(i(r#"{"v":1.0}"#).unwrap(), 1);
        assert_eq!(u(r#"{"v":"1001"}"#).unwrap(), 1001);
        assert_eq!(u(r#"{"v":1001.0}"#).unwrap(), 1001);
    }

    #[test]
    fn default_on_null_accepts_missing_null_and_string() {
        assert_eq!(serde_json::from_str::<S>(r#"{}"#).unwrap().v, "");
        assert_eq!(serde_json::from_str::<S>(r#"{"v":null}"#).unwrap().v, "");
        assert_eq!(serde_json::from_str::<S>(r#"{"v":"x"}"#).unwrap().v, "x");
        assert!(serde_json::from_str::<S>(r#"{"v":1}"#).is_err());
        assert!(serde_json::from_str::<L>(r#"{}"#).unwrap().v.is_empty());
        assert!(
            serde_json::from_str::<L>(r#"{"v":null}"#)
                .unwrap()
                .v
                .is_empty()
        );
        assert_eq!(serde_json::from_str::<L>(r#"{"v":[1]}"#).unwrap().v, [1]);
    }

    #[test]
    fn rejects_out_of_range_and_fractional_values() {
        // u64::MAX must not wrap to -1 and then pass the i32 check.
        assert!(i(r#"{"v":18446744073709551615}"#).is_err());
        assert!(i(r#"{"v":2147483648}"#).is_err());
        assert!(i(r#"{"v":1.5}"#).is_err());
        assert!(i(r#"{"v":1e30}"#).is_err());
        assert!(u(r#"{"v":-1}"#).is_err());
        assert!(u(r#"{"v":-1.0}"#).is_err());
        assert!(u(r#"{"v":"abc"}"#).is_err());
    }
}
