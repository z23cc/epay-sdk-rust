//! Strict parsing and verification of `notify_url` / `return_url` callbacks.
//!
//! # Replay protection
//!
//! 彩虹易支付 V1 signs notifications with a shared-key MD5 and carries **no
//! timestamp or nonce**, so a captured notification stays valid forever and
//! the SDK cannot detect replays. Applications must make the handler
//! idempotent: key the state transition on `out_trade_no` (and record
//! `trade_no`), compare [`NotifyData::money`] against the local order, and
//! ignore notifications for orders that are already paid.

use std::collections::BTreeMap;

use percent_encoding::percent_decode_str;

use crate::error::{Error, Result};
use crate::money::Money;
use crate::params::Params;
use crate::signer::{SIGN_TYPE_MD5, Signer};
use crate::types::{PayType, TradeStatus};
use crate::validation::{
    validate_name, validate_out_trade_no, validate_pay_type, validate_trade_no,
};

/// Size limits applied to the encoded query plus form body of a notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotifyLimits {
    /// Maximum combined byte length of the raw query string and form body.
    pub max_encoded_bytes: usize,
    /// Maximum number of key/value pairs across both sources.
    pub max_fields: usize,
}

impl Default for NotifyLimits {
    fn default() -> Self {
        Self {
            max_encoded_bytes: 16 * 1024,
            max_fields: 128,
        }
    }
}

/// Where a notification field was read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotifySource {
    /// URL query string.
    Query,
    /// `application/x-www-form-urlencoded` body.
    Form,
}

/// One decoded key/value pair with its origin.
///
/// `Debug` masks the values of `key` and `sign`.
#[derive(Clone, PartialEq, Eq)]
pub struct RawNotifyEntry {
    /// Decoded field name.
    pub key: String,
    /// Decoded field value.
    pub value: String,
    /// Origin of the pair.
    pub source: NotifySource,
}

impl std::fmt::Debug for RawNotifyEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value: &dyn std::fmt::Debug = if matches!(self.key.as_str(), "key" | "sign") {
            &"***"
        } else {
            &self.value
        };
        f.debug_struct("RawNotifyEntry")
            .field("key", &self.key)
            .field("value", value)
            .field("source", &self.source)
            .finish()
    }
}

/// Source-preserving, duplicate-aware notify parameters.
///
/// `Debug` masks `key` / `sign` values so a logged notification cannot be
/// replayed from the log.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct RawNotifyParams {
    entries: Vec<RawNotifyEntry>,
}

impl std::fmt::Debug for RawNotifyParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(&self.entries).finish()
    }
}

impl RawNotifyParams {
    /// Entries in wire order.
    pub fn entries(&self) -> &[RawNotifyEntry] {
        &self.entries
    }

    /// Reject duplicates and query/form conflicts, then drop empty values for
    /// the V1 signing representation.
    pub fn canonicalize(&self) -> Result<Params> {
        let mut query = BTreeMap::new();
        let mut form = BTreeMap::new();
        for entry in &self.entries {
            let target = match entry.source {
                NotifySource::Query => &mut query,
                NotifySource::Form => &mut form,
            };
            if target
                .insert(entry.key.clone(), entry.value.clone())
                .is_some()
            {
                return Err(Error::notify(format!(
                    "duplicate {} field",
                    source_name(entry.source)
                )));
            }
        }
        for (key, value) in &form {
            if let Some(query_value) = query.get(key) {
                if query_value != value {
                    return Err(Error::notify("conflicting query/form field"));
                }
            }
        }
        let mut params = Params::new();
        for (key, value) in query.into_iter().chain(form) {
            if params.contains(&key) {
                continue;
            }
            params.insert(key, value);
        }
        Ok(params)
    }
}

/// Authenticated and structurally valid asynchronous/return notification.
///
/// Authenticity does not imply freshness: see the module docs on replay
/// protection. `Debug` shows `param` (opaque merchant data, possibly
/// personal) as a length only.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct NotifyData {
    /// Merchant id; already checked against the configured PID.
    pub pid: u64,
    /// EPay order number.
    pub trade_no: String,
    /// Merchant order number.
    pub out_trade_no: String,
    /// Payment channel.
    pub pay_type: PayType,
    /// Product name.
    pub name: String,
    /// Paid amount.
    pub money: Money,
    /// Trade status; only `TRADE_SUCCESS` means paid.
    pub trade_status: TradeStatus,
    /// Opaque merchant data from the original request (may be empty).
    pub param: String,
}

impl std::fmt::Debug for NotifyData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotifyData")
            .field("pid", &self.pid)
            .field("trade_no", &self.trade_no)
            .field("out_trade_no", &self.out_trade_no)
            .field("pay_type", &self.pay_type)
            .field("name", &self.name)
            .field("money", &self.money)
            .field("trade_status", &self.trade_status)
            .field("param", &format_args!("<{} bytes>", self.param.len()))
            .finish()
    }
}

impl NotifyData {
    /// Whether `trade_status` is exactly `TRADE_SUCCESS`.
    pub fn is_success(&self) -> bool {
        self.trade_status.is_success()
    }

    /// Numeric comparison of the paid amount with the local order amount.
    pub fn money_matches(&self, expected: Money) -> bool {
        self.money == expected
    }

    /// Convert into [`SuccessfulNotify`] if the status is `TRADE_SUCCESS`.
    pub fn try_into_success(self) -> Result<SuccessfulNotify> {
        if self.is_success() {
            Ok(SuccessfulNotify(self))
        } else {
            Err(Error::notify("trade_status is not TRADE_SUCCESS"))
        }
    }
}

/// Structurally valid notification whose status is `TRADE_SUCCESS`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SuccessfulNotify(NotifyData);

impl SuccessfulNotify {
    /// Borrow the verified notification.
    pub fn data(&self) -> &NotifyData {
        &self.0
    }

    /// Unwrap the verified notification.
    pub fn into_inner(self) -> NotifyData {
        self.0
    }
}

/// Strictly parse query and/or `application/x-www-form-urlencoded` input.
pub fn parse_raw_notify_params(
    query: Option<&str>,
    form_body: Option<&str>,
) -> Result<RawNotifyParams> {
    parse_raw_notify_params_with_limits(query, form_body, NotifyLimits::default())
}

/// [`parse_raw_notify_params`] with explicit limits.
pub fn parse_raw_notify_params_with_limits(
    query: Option<&str>,
    form_body: Option<&str>,
    limits: NotifyLimits,
) -> Result<RawNotifyParams> {
    let encoded_bytes = query.map_or(0, str::len) + form_body.map_or(0, str::len);
    if encoded_bytes > limits.max_encoded_bytes {
        return Err(Error::notify(format!(
            "encoded notification exceeds {} bytes",
            limits.max_encoded_bytes
        )));
    }
    let mut entries = Vec::new();
    parse_source(query, NotifySource::Query, limits.max_fields, &mut entries)?;
    parse_source(
        form_body,
        NotifySource::Form,
        limits.max_fields,
        &mut entries,
    )?;
    Ok(RawNotifyParams { entries })
}

/// Parse and canonicalize strict notification parameters.
///
/// Callers must bound the raw HTTP input before calling this; see
/// [`NotifyLimits`] for the defaults applied to the decoded representation.
pub fn parse_notify_params(query: Option<&str>, form_body: Option<&str>) -> Result<Params> {
    parse_raw_notify_params(query, form_body)?.canonicalize()
}

/// Verify signature, PID and all required notification fields.
pub fn verify_notify(signer: &Signer, expected_pid: u64, params: &Params) -> Result<NotifyData> {
    let sign_type = required(params, "sign_type")?;
    if !sign_type.eq_ignore_ascii_case(SIGN_TYPE_MD5) {
        return Err(Error::notify("unsupported sign_type (only MD5)"));
    }
    let sign = required(params, "sign")?;
    if !signer.verify(params, sign) {
        return Err(Error::VerifyFailed);
    }

    let pid = required(params, "pid")?
        .parse::<u64>()
        .map_err(|_| Error::notify("pid is not a positive integer"))?;
    if pid == 0 || pid != expected_pid {
        return Err(Error::notify("pid does not match the configured pid"));
    }
    let trade_no = required(params, "trade_no")?;
    validate_trade_no(trade_no).map_err(|_| Error::notify("trade_no is invalid"))?;
    let out_trade_no = required(params, "out_trade_no")?;
    validate_out_trade_no(out_trade_no).map_err(|_| Error::notify("out_trade_no is invalid"))?;
    let pay_type = PayType::from(required(params, "type")?);
    validate_pay_type(&pay_type).map_err(|_| Error::notify("type is blank"))?;
    let name = required(params, "name")?;
    validate_name(name).map_err(|_| Error::notify("name is blank"))?;
    let money = Money::from_yuan_str(required(params, "money")?)
        .map_err(|_| Error::notify("money is invalid"))?;
    let trade_status = required(params, "trade_status")?;
    if trade_status.trim().is_empty() {
        return Err(Error::notify("trade_status is blank"));
    }

    Ok(NotifyData {
        pid,
        trade_no: trade_no.to_owned(),
        out_trade_no: out_trade_no.to_owned(),
        pay_type,
        name: name.to_owned(),
        money,
        trade_status: TradeStatus(trade_status.to_owned()),
        param: params.get("param").unwrap_or_default().to_owned(),
    })
}

fn required<'a>(params: &'a Params, key: &str) -> Result<&'a str> {
    params
        .get(key)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::notify(format!("missing required field {key:?}")))
}

fn parse_source(
    input: Option<&str>,
    source: NotifySource,
    max_fields: usize,
    entries: &mut Vec<RawNotifyEntry>,
) -> Result<()> {
    let Some(input) = input.filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    for pair in input.split('&') {
        if pair.is_empty() {
            continue;
        }
        if entries.len() >= max_fields {
            return Err(Error::notify(format!(
                "notification exceeds {max_fields} fields"
            )));
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = decode_component(key)?;
        if key.is_empty() {
            return Err(Error::notify("notification field name is empty"));
        }
        entries.push(RawNotifyEntry {
            key,
            value: decode_component(value)?,
            source,
        });
    }
    Ok(())
}

fn decode_component(input: &str) -> Result<String> {
    validate_percent_encoding(input)?;
    let input = input.replace('+', " ");
    percent_decode_str(&input)
        .decode_utf8()
        .map(|value| value.into_owned())
        .map_err(|_| Error::notify("form field is not valid UTF-8"))
}

fn validate_percent_encoding(input: &str) -> Result<()> {
    let bytes = input.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return Err(Error::notify("malformed percent encoding"));
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    Ok(())
}

fn source_name(source: NotifySource) -> &'static str {
    match source {
        NotifySource::Query => "query",
        NotifySource::Form => "form",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed_notify(signer: &Signer) -> Params {
        let mut params = Params::new();
        params.insert("pid", "1001");
        params.insert("trade_no", "T20231123001");
        params.insert("out_trade_no", "ORDER001");
        params.insert("type", "alipay");
        params.insert("name", "Test Product");
        params.insert("money", "10.00");
        params.insert("trade_status", "TRADE_SUCCESS");
        signer.sign_with_params(params)
    }

    #[test]
    fn raw_params_debug_masks_signature_and_key() {
        let raw = parse_raw_notify_params(
            Some("pid=1001&sign=deadbeef&key=super-secret"),
            Some("out_trade_no=ORDER001"),
        )
        .unwrap();
        let text = format!("{raw:?}");
        assert!(!text.contains("deadbeef"), "{text}");
        assert!(!text.contains("super-secret"), "{text}");
        assert!(text.contains("ORDER001"), "{text}");
        assert!(text.contains("***"), "{text}");
        assert_eq!(raw.entries()[1].value, "deadbeef", "values stay readable");
    }

    #[test]
    fn strict_parser_rejects_duplicates_conflicts_and_bad_encoding() {
        assert!(parse_notify_params(Some("pid=1&pid=1"), None).is_err());
        assert!(parse_notify_params(Some("pid=1"), Some("pid=2")).is_err());
        assert!(parse_notify_params(Some("name=%ZZ"), None).is_err());
        let params = parse_notify_params(Some("pid=1"), Some("pid=1&name=A+B")).unwrap();
        assert_eq!(params.get("name"), Some("A B"));
    }

    #[test]
    fn verifies_complete_notify() {
        let signer = Signer::new("testkey123").unwrap();
        let mut params = signed_notify(&signer);
        params.remove("sign");
        params.remove("sign_type");
        params.insert("param", "phone=13800000000");
        let params = signer.sign_with_params(params);
        let data = verify_notify(&signer, 1001, &params).unwrap();
        assert!(data.is_success());
        assert_eq!(data.param, "phone=13800000000");
        let debug = format!("{data:?}");
        assert!(!debug.contains("13800000000"), "{debug}");
        assert!(debug.contains("ORDER001"), "{debug}");
        assert!(data.money_matches(Money::from_yuan_str("10.00").unwrap()));
        assert!(data.try_into_success().is_ok());
    }

    #[test]
    fn successful_status_is_case_sensitive() {
        let signer = Signer::new("testkey123").unwrap();
        let mut params = signed_notify(&signer);
        params.remove("sign");
        params.remove("sign_type");
        params.insert("trade_status", "trade_success");
        let params = signer.sign_with_params(params);
        let data = verify_notify(&signer, 1001, &params).unwrap();
        assert!(!data.is_success());
        assert!(data.try_into_success().is_err());
    }

    #[test]
    fn rejects_missing_metadata_or_required_fields() {
        let signer = Signer::new("testkey123").unwrap();
        let mut params = signed_notify(&signer);
        params.remove("sign_type");
        assert!(matches!(
            verify_notify(&signer, 1001, &params),
            Err(Error::Notify { .. })
        ));
        params.insert("sign_type", "RSA");
        assert!(matches!(
            verify_notify(&signer, 1001, &params),
            Err(Error::Notify { .. })
        ));

        let mut incomplete = Params::new();
        incomplete.insert("pid", "1001");
        incomplete.insert("out_trade_no", "ORDER001");
        let incomplete = signer.sign_with_params(incomplete);
        assert!(verify_notify(&signer, 1001, &incomplete).is_err());
    }

    #[test]
    fn pid_mismatch_and_bad_signature_are_distinct_errors() {
        let signer = Signer::new("testkey123").unwrap();
        let params = signed_notify(&signer);
        assert!(matches!(
            verify_notify(&signer, 2002, &params),
            Err(Error::Notify { .. })
        ));
        let mut tampered = params.clone();
        tampered.insert("money", "0.01");
        assert!(matches!(
            verify_notify(&signer, 1001, &tampered),
            Err(Error::VerifyFailed)
        ));
    }

    #[test]
    fn enforces_limits() {
        let exact = format!("a={}", "x".repeat(16 * 1024 - 2));
        assert_eq!(exact.len(), 16 * 1024);
        assert!(parse_raw_notify_params(Some(&exact), None).is_ok());
        let too_large = format!("{exact}x");
        assert!(parse_raw_notify_params(Some(&too_large), None).is_err());

        let limits = NotifyLimits {
            max_encoded_bytes: 5,
            max_fields: 1,
        };
        assert!(parse_raw_notify_params_with_limits(Some("abcdef"), None, limits).is_err());
        assert!(
            parse_raw_notify_params_with_limits(
                Some("a=1&b=2"),
                None,
                NotifyLimits {
                    max_encoded_bytes: 100,
                    max_fields: 1,
                }
            )
            .is_err()
        );
    }
}
