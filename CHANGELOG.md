# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the crate adheres
to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [1.0.0] - 2026-08-28

First public release. The public API is considered stable from here:
breaking changes require a major version bump; new `ErrorKind` /
`Endpoint` / enum variants may arrive in minor versions behind
`#[non_exhaustive]`.

### Protocol
- 彩虹易支付 V1: `POST mapi.php` (API payment, `clientip` required and
  validated), `submit.php` form / redirect payment, `api.php` order, orders,
  refund (`POST`, `act` in the query string, official success code `0` and
  fork `1`), merchant and settlement queries.
- `Endpoint` enum is the single source of truth for path, `act`, HTTP
  method and success codes; carried by `Error::Api` / `Error::InvalidResponse`.
- Response envelopes (`code` / `msg`) are checked before the success payload
  is decoded; PHP `null` columns and `data: null` map to empty values;
  unmodelled fields are preserved in `extra` (`code`, `key` and `sign` are
  stripped first).
- Strict notification verification: `sign_type=MD5`, signature, PID and all
  required fields; duplicate / conflicting / malformed fields rejected;
  combined 16 KiB budget (`NotifyLimits`); `NotifyData` / `SuccessfulNotify`.

### Core API
- `ProtocolContext` (transport-free signing, forms, callback verification)
  and `Client` (async gateway calls) with `ProtocolContextBuilder` /
  `ClientBuilder`, `from_env`, and a `Config` / `HttpConfig` split.
- `PreparedForm` + `prepare_form_payment()` for CSP-friendly form rendering
  (read-only `action()` / `params()`, `hidden_inputs`, `redirect_url`,
  `auto_submit_html`, `into_parts`).
- `PaymentDestination` + `PaymentResponse::destination()/destinations()`.
- `Money`: exact two-decimal amounts, explicit `*_rounded` constructors,
  serde (wire string or integer JSON number), `try_to_fen()`.
- `RetryPolicy` for GET endpoints only (per-attempt timeout, exponential
  back-off), `CallOptions` with `*_with` variants, `connect_timeout`.
- Pagination helpers `OrderListResponse::has_more` / `next_page` and bounded
  `Client::query_all_orders`.
- `ErrorKind`, `Error::kind()` and `Error::endpoint()` for stable
  low-cardinality matching and metrics; callers keep a fallback arm.
  `TransportErrorKind`, `Error::is_retryable()`, `Error::transport()` and
  `Error::http()` for custom transports.
- `Transport` trait with `TransportOptions` and a required `sleep` hook;
  built-in reqwest transport (rustls or native-tls).
- `limits` module: caps for `name`, `param`, `sitename`, callback URLs,
  custom `type` / `device` identifiers and the encoded request size.
- Public enums and structs are `#[non_exhaustive]`.

### Integrations and features
- `axum` (no default features): `VerifiedNotify`, `VerifiedReturn`,
  `ParsedNotify` (multi-merchant routing), `NotifyParams`, `NotifyAck`,
  `callback_route`, `payer_ip`.
- `blocking`: `blocking::Client` for non-async programs.
- `tracing`: a span per gateway call; replaces `log` output instead of
  duplicating it.
- `test-util`: `MockTransport` / `RecordedRequest` and `NotifyFixture`.

### Security
- API base URL and callback URLs must be HTTPS unless explicitly allowed
  (`allow_insecure_http`, `allow_insecure_callback_http`).
- SDK-created reqwest client follows no redirects, bounds response bodies
  (1 MiB default, streamed) and reports errors without the request URL.
- Merchant key is held once (`Config` and `Signer` share it) and zeroized
  on drop; key, signatures, callback query strings, payment links and
  personal fields (`param`, `buyer`, settlement accounts) are masked in
  `Debug`, logs and error text.
- `VerifiedNotify` rejections are logged at `debug` only; observe them via
  `Result<VerifiedNotify, NotifyRejection>`.
- Every outbound request is size-bounded before it reaches the transport;
  `name` / `sitename` reject control characters.

### Tooling
- Complete API documentation, README and guide compiled as doctests, CI
  (stable + 1.85 MSRV, feature matrix, cargo-deny, package), property tests
  (proptest) and sanitized real-fork response fixtures.

[Unreleased]: https://github.com/z23cc/epay-sdk-rust/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/z23cc/epay-sdk-rust/releases/tag/v1.0.0
