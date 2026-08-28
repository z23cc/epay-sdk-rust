# Security Policy

## Supported versions

| Version | Supported |
|---|---|
| 0.2.x | yes |
| < 0.2 | no |

## Reporting a vulnerability

Please do **not** open a public issue for security problems. Use GitHub's
*Private vulnerability reporting* on the repository named in `Cargo.toml`
(`repository`), and include:

- affected version and feature flags;
- a description and, if possible, a minimal reproduction;
- whether the issue depends on a specific 易支付 fork.

You will get an acknowledgement within 7 days. Fixes are released as patch
versions and recorded in `CHANGELOG.md` under *Security*.

## Protocol limitations you must design around

彩虹易支付 V1 is a legacy protocol; the SDK enforces everything it can, but
some weaknesses are inherent and must be handled by the application:

- **Shared-key MD5 signatures.** Anyone holding the merchant key can forge
  requests and notifications. Keep the key out of source control, logs and
  browser-facing code; rotate it if a leak is suspected.
- **No replay protection.** Notifications carry no timestamp or nonce, so a
  captured notification stays valid. Make the `notify_url` handler
  idempotent on `out_trade_no`, compare the amount with the local order and
  ignore repeats for already-paid orders.
- **Merchant key in GET URLs.** `api.php` queries authenticate with
  `key=` in the query string. The SDK never logs it, but gateway, proxy and
  CDN access logs on the path to the gateway will contain it — use HTTPS end
  to end and trust the operators of that path.
- **Plain HTTP gateways** are refused unless `allow_insecure_http(true)` is
  set explicitly, and **plain HTTP callback URLs** are refused unless
  `allow_insecure_callback_http(true)` is set; do not enable either in
  production. A callback over HTTP exposes the signed payment result to
  on-path readers, who can replay it.

## What the SDK guarantees

- Merchant key, signatures and callback query strings are masked in `Debug`
  output, logs and error messages produced by the SDK.
- The merchant key is stored once (shared by `Config` and `Signer`) and
  zeroized when the last handle is dropped. It is still copied transiently
  into request parameter maps and the outgoing HTTP request; those buffers
  are freed after the call but not wiped.
- The reqwest client the SDK creates itself follows no redirects, bounds
  response bodies and reports errors without the request URL. A
  `reqwest::Client` supplied by the caller keeps its own redirect policy;
  response limits and error redaction still apply.
- Credentials echoed by the gateway (`key`, `sign`) are stripped from
  successful responses before they reach application code.
- `VerifiedNotify` / `verify_notify` accept a notification only if the
  signature, PID and all required fields validate.
