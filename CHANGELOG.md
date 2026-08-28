# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the crate adheres
to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.2.0] - 2026-08-28

First release intended for production use. Everything below is relative to
the unpublished 0.1 line.

### Added
- `ProtocolContext`: transport-free signing, form building and callback
  verification; the recommended Axum state.
- `Endpoint` enum carried by `Error::Api` / `Error::InvalidResponse`; single
  source of truth for path, `act`, HTTP method and success codes (refund
  accepts the official `0` and fork `1`).
- `HttpConfig` (timeout, response limit, retry policy) separated from the
  protocol `Config`.
- `RetryPolicy` and `Error::is_retryable()`: opt-in retries with exponential
  back-off for GET endpoints only; payments and refunds are never retried.
- `CallOptions` and `*_with` client methods for per-call timeout / retry.
- `TransportOptions` passed to `Transport`; `Transport::sleep` hook for
  back-off.
- `TransportErrorKind`, `Error::transport_kind()`, `Error::is_timeout()`;
  `Error::transport()` is public for custom transports.
- `extra: BTreeMap<String, Value>` on every response struct preserving
  fork-specific fields (`bill_trade_no`, `pay_fee`, …); the envelope `code`
  and any echoed `key` / `sign` are stripped first.
- Pagination helpers `OrderListResponse::has_more` / `next_page` and
  `Client::query_all_orders(limit, max_pages)`.
- `Money` implements `Serialize`/`Deserialize` (wire string or integer
  JSON number; fractional JSON numbers are rejected) and `Money::to_fen()`.
- `blocking` feature: `blocking::Client` for non-async programs.
- `tracing` feature: a span per gateway call plus retry/warn events.
- Axum `VerifiedReturn` extractor and `NotifyParams` fallible extractor;
  `callback_route` (GET + POST in one call) and `payer_ip` helper;
  `PaymentRequest::client_ip_addr`.
- `test_util::NotifyFixture` for signed notification payloads in tests.
- Strict notification parser: duplicate / conflicting / malformed fields
  rejected; combined 16 KiB budget; `NotifyLimits`.
- Settlement rows bound to the configured PID.
- Environment configuration is testable (`EPAY_MAX_RETRIES` added).
- Complete API documentation (`#![warn(missing_docs)]`), README doctests,
  CI workflow, `cargo-deny` configuration, `SECURITY.md`.

### Changed
- `mapi.php` is sent as `POST`; `clientip` is required and validated.
- Refund is `POST api.php?act=refund` with `act` in the query string.
- API base URL must be HTTPS unless `allow_insecure_http(true)`.
- SDK-created reqwest client: no redirects, streaming response limit
  (1 MiB default), URL-free error messages, per-request timeouts. A
  caller-supplied `reqwest::Client` keeps its own redirect policy.
- `Money` constructors reject more than two decimals; explicit `*_rounded`
  variants round.
- Response structs no longer expose `code` / `msg` / `is_success()`
  (`RefundResponse::msg` remains).
- `PaymentRequest::validate` / `FormPaymentRequest::validate` no longer take
  a default `notify_url`.
- `Transport::post_form` takes `&[(&str, &str)]` and `TransportOptions`.
- `Error` and public enums/structs are `#[non_exhaustive]`; notify errors
  are reported as `Error::Notify` (signature mismatch stays `VerifyFailed`).
- `Signer::new`, `parse_notify_params`, `generate_out_trade_no` return
  `Result`.
- `axum` dependency is pulled in without default features.

### Removed
- `MerchantApiAuth` / `EPAY_API_AUTH` (the official gateway has no merchant
  `sign` mode for `api.php`).
- `Params::with_capacity`.
- Actix Web integration (use `parse_notify_params` + `verify_notify`).

### Security
- Merchant key, signatures and callback query strings are masked in `Debug`,
  logs and error text; `MerchantInfo` masks the echoed key in `Debug`.
- Notifications must carry `sign_type=MD5`, match the configured PID and
  have structurally valid order number, amount and status before they are
  handed to application code.

[Unreleased]: https://github.com/z23cc/epay-sdk-rust/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/z23cc/epay-sdk-rust/releases/tag/v0.2.0
