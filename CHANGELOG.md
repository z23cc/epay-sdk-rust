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
  JSON number; fractional JSON numbers are rejected) and
  `Money::try_to_fen()` (errors instead of saturating).
- `PreparedForm` + `prepare_form_payment()` for CSP-friendly form rendering
  (read-only `action()` / `params()`, `hidden_inputs`, `redirect_url`,
  `auto_submit_html`, `into_parts`).
- `PaymentDestination` + `PaymentResponse::destination()/destinations()`.
- `ErrorKind`, `Error::kind()` and `Error::endpoint()` for exhaustive
  matching and metrics without touching the non-exhaustive `Error`.
- `ClientBuilder::connect_timeout` / `EPAY_CONNECT_TIMEOUT_SECS` (zero or
  combining with a caller-supplied reqwest client fails at build); request
  timeouts are documented as per-attempt.
- Axum `ParsedNotify` extractor (parse only, `200 fail` on rejection) for
  multi-merchant callback routing.
- Property tests (proptest) and sanitized real-fork response fixtures.
- `blocking` feature: `blocking::Client` for non-async programs.
- `tracing` feature: a span per gateway call plus retry/warn events;
  replaces `log` output instead of duplicating it.
- Axum `VerifiedReturn` extractor and `NotifyParams` fallible extractor;
  `callback_route` (GET + POST in one call) and `payer_ip` helper;
  `PaymentRequest::client_ip_addr`.
- `test_util::NotifyFixture` for signed notification payloads in tests.
- Strict notification parser: duplicate / conflicting / malformed fields
  rejected; combined 16 KiB budget; `NotifyLimits`.
- `limits` module: outbound caps for `name`, `param`, `sitename`, callback
  URLs, custom `type` / `device` identifiers and the encoded request size.
- Settlement rows bound to the configured PID.
- Environment configuration is testable (`EPAY_MAX_RETRIES` added).
- Complete API documentation (`#![warn(missing_docs)]`), README doctests,
  CI workflow, `cargo-deny` configuration, `SECURITY.md`.

### Changed
- `mapi.php` is sent as `POST`; `clientip` is required and validated.
- Refund is `POST api.php?act=refund` with `act` in the query string.
- API base URL must be HTTPS unless `allow_insecure_http(true)`; callback
  URLs (configured defaults and request overrides) must be HTTPS unless
  `allow_insecure_callback_http(true)` / `EPAY_ALLOW_INSECURE_CALLBACK_HTTP`.
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
- The merchant key is held once, shared by `Config` and `Signer`, and
  zeroized when the last handle is dropped.
- `RawNotifyParams` / `RawNotifyEntry` / `NotifyFixture` mask `key` and
  `sign` in `Debug`, so a logged notification cannot be replayed from logs.
- `PaymentRequest` / `FormPaymentRequest` / `PaymentResponse` mask callback
  and payment-link query strings in `Debug` (unparseable URLs show their
  length only); `param` and `qr_code` show their length, `extra` its keys.
- `NotifyData::param`, `OrderDetail::buyer`/`param`, `Settlement::account`
  and `MerchantInfo::account` show only their length in `Debug`; payment
  links in `PaymentResponse` / `PaymentDestination` `Debug` are reduced to
  scheme and host.
- Every outbound request is size-checked in the client's GET/POST paths;
  `name` / `sitename` reject control characters; `mapi.php` success
  responses require a well-formed `trade_no`.
- `VerifiedNotify` logs rejections at `debug` instead of `warn` (public
  endpoint); extract `Result<VerifiedNotify, NotifyRejection>` to observe
  them.
- `MockTransport::recorded()` stores query/form as `Params` (redacting
  `Debug`) rather than a URL containing `key=`.
- Notifications must carry `sign_type=MD5`, match the configured PID and
  have structurally valid order number, amount and status before they are
  handed to application code.

[Unreleased]: https://github.com/z23cc/epay-sdk-rust/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/z23cc/epay-sdk-rust/releases/tag/v0.2.0
