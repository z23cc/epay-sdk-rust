//! Exact positive CNY amounts.

use std::fmt;
use std::str::FromStr;

use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{Error, Result};

/// Positive CNY amount represented exactly to at most two decimal places.
///
/// Normal constructors never change the numeric value. Use an explicitly
/// named `*_rounded` constructor when rounding is intentional.
///
/// `Money` serializes as the wire string (`"9.90"`) and deserializes through
/// the strict [`Money::new`] rules, so it can be embedded directly in
/// application order records. Only strings and integer JSON numbers are
/// accepted: a fractional JSON number has already been rounded to `f64` by
/// the time serde hands it over, so its exactness cannot be verified. Send
/// amounts as strings (`"9.90"`) or use [`Money::try_from_f64`] explicitly.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Money(Decimal);

impl Money {
    /// Build an exact yuan amount. Values with meaningful precision beyond
    /// cents are rejected (`10.000` is accepted, `9.999` is not).
    pub fn new(amount: Decimal) -> Result<Self> {
        if amount <= Decimal::ZERO {
            return Err(Error::INVALID_MONEY);
        }
        let rounded = amount.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero);
        if rounded != amount {
            return Err(Error::INVALID_MONEY_PRECISION);
        }
        Ok(Self(rounded))
    }

    /// Build a yuan amount while explicitly rounding half away from zero.
    pub fn new_rounded(amount: Decimal) -> Result<Self> {
        let rounded = amount.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero);
        if rounded <= Decimal::ZERO {
            return Err(Error::INVALID_MONEY);
        }
        Ok(Self(rounded))
    }

    /// Parse an exact yuan amount such as `"9.90"`.
    pub fn from_yuan_str(s: &str) -> Result<Self> {
        let amount = Decimal::from_str(s.trim()).map_err(|_| Error::INVALID_MONEY)?;
        Self::new(amount)
    }

    /// Parse a yuan amount, rounding to cents half away from zero.
    pub fn from_yuan_str_rounded(s: &str) -> Result<Self> {
        let amount = Decimal::from_str(s.trim()).map_err(|_| Error::INVALID_MONEY)?;
        Self::new_rounded(amount)
    }

    /// Build from an amount in fen (1/100 yuan).
    pub fn from_fen(fen: i64) -> Result<Self> {
        if fen <= 0 {
            return Err(Error::INVALID_MONEY);
        }
        Self::new(Decimal::from(fen) / Decimal::from(100))
    }

    /// Best-effort exact conversion from `f64`. Prefer string or Decimal input.
    pub fn try_from_f64(v: f64) -> Result<Self> {
        let amount = Decimal::from_f64_retain(v).ok_or(Error::INVALID_MONEY)?;
        Self::new(amount)
    }

    /// Convert from `f64`, rounding to cents half away from zero.
    pub fn try_from_f64_rounded(v: f64) -> Result<Self> {
        let amount = Decimal::from_f64_retain(v).ok_or(Error::INVALID_MONEY)?;
        Self::new_rounded(amount)
    }

    /// Underlying decimal value.
    pub fn decimal(self) -> Decimal {
        self.0
    }

    /// Amount in fen (1/100 yuan), exact because `Money` carries at most two
    /// decimals. Interoperates with integer-minor-unit systems; saturates at
    /// `i64::MAX` for amounts beyond ~9.2e16 fen.
    pub fn to_fen(self) -> i64 {
        self.0
            .checked_mul(Decimal::from(100))
            .and_then(|fen| fen.to_i64())
            .unwrap_or(i64::MAX)
    }

    /// Wire representation with exactly two decimal places (`"9.90"`).
    pub fn to_epay_string(self) -> String {
        format!("{:.2}", self.0)
    }
}

impl fmt::Debug for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Money")
            .field(&self.to_epay_string())
            .finish()
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_epay_string())
    }
}

impl FromStr for Money {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::from_yuan_str(s)
    }
}

impl TryFrom<Decimal> for Money {
    type Error = Error;

    fn try_from(value: Decimal) -> Result<Self> {
        Self::new(value)
    }
}

impl From<Money> for Decimal {
    fn from(value: Money) -> Self {
        value.0
    }
}

impl Serialize for Money {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_epay_string())
    }
}

impl<'de> Deserialize<'de> for Money {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        deserializer.deserialize_any(MoneyVisitor)
    }
}

struct MoneyVisitor;

impl serde::de::Visitor<'_> for MoneyVisitor {
    type Value = Money;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a positive amount string with at most two decimal places")
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<Money, E> {
        Money::from_yuan_str(v).map_err(E::custom)
    }

    fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<Money, E> {
        Money::new(Decimal::from(v)).map_err(E::custom)
    }

    fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<Money, E> {
        Money::new(Decimal::from(v)).map_err(E::custom)
    }

    fn visit_f64<E: serde::de::Error>(self, v: f64) -> std::result::Result<Money, E> {
        Err(E::custom(format!(
            "fractional amount {v} must be sent as a string to preserve exactness"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fen_round_trips_and_saturates() {
        let money = Money::from_yuan_str("9.90").unwrap();
        assert_eq!(money.to_fen(), 990);
        assert_eq!(Money::from_fen(990).unwrap(), money);
        assert_eq!(Money::from_fen(1).unwrap().to_fen(), 1);
        assert_eq!(Money::new(Decimal::MAX).unwrap().to_fen(), i64::MAX);
        assert_eq!(
            Money::from_yuan_str("100000000000000000").unwrap().to_fen(),
            i64::MAX
        );
    }

    #[test]
    fn formats_two_decimals() {
        let money = Money::from_yuan_str("9.9").unwrap();
        assert_eq!(money.to_epay_string(), "9.90");
        assert_eq!(
            Money::from_yuan_str("10.000").unwrap(),
            Money::from_fen(1000).unwrap()
        );
    }

    #[test]
    fn rejects_nonpositive_and_excess_precision() {
        for value in ["0", "-1", "0.004", "0.005", "9.999"] {
            assert!(Money::from_yuan_str(value).is_err(), "{value}");
        }
        assert!(Money::from_fen(0).is_err());
    }

    #[test]
    fn explicit_rounding_is_available() {
        assert_eq!(
            Money::from_yuan_str_rounded("0.005")
                .unwrap()
                .to_epay_string(),
            "0.01"
        );
        assert_eq!(
            Money::from_yuan_str_rounded("9.999")
                .unwrap()
                .to_epay_string(),
            "10.00"
        );
    }

    #[test]
    fn exact_f64_conversion_is_conservative() {
        assert_eq!(Money::try_from_f64(10.0).unwrap().to_epay_string(), "10.00");
        assert!(Money::try_from_f64(9.999).is_err());
        assert_eq!(
            Money::try_from_f64_rounded(9.999).unwrap().to_epay_string(),
            "10.00"
        );
    }

    #[test]
    fn serde_round_trips_as_wire_string() {
        #[derive(Serialize, Deserialize)]
        struct Order {
            money: Money,
        }
        let order = Order {
            money: Money::from_yuan_str("9.9").unwrap(),
        };
        let json = serde_json::to_string(&order).unwrap();
        assert_eq!(json, r#"{"money":"9.90"}"#);
        let parsed: Order = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.money, order.money);
        let numeric: Order = serde_json::from_str(r#"{"money":10}"#).unwrap();
        assert_eq!(numeric.money.to_epay_string(), "10.00");
        let big: Order = serde_json::from_str(r#"{"money":"9007199254740992.01"}"#).unwrap();
        assert_eq!(big.money.to_epay_string(), "9007199254740992.01");
        for bad in [
            r#"{"money":"0"}"#,
            r#"{"money":"9.999"}"#,
            r#"{"money":-1}"#,
            r#"{"money":0.1}"#,
            r#"{"money":9.999}"#,
            r#"{"money":1.0000000000000001}"#,
            r#"{"money":null}"#,
        ] {
            assert!(serde_json::from_str::<Order>(bad).is_err(), "{bad}");
        }
    }
}
