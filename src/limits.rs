//! Size limits applied to outbound requests.
//!
//! 彩虹易支付 V1 publishes no formal limits; these are conservative caps that
//! stay well within what the PHP implementation and its database columns
//! accept while keeping signed URLs and forms bounded.

/// Maximum UTF-8 length of `out_trade_no`.
pub const MAX_OUT_TRADE_NO_BYTES: usize = 64;
/// Maximum UTF-8 length of the product `name`.
pub const MAX_NAME_BYTES: usize = 255;
/// Maximum UTF-8 length of the opaque `param`.
pub const MAX_PARAM_BYTES: usize = 255;
/// Maximum UTF-8 length of `sitename`.
pub const MAX_SITENAME_BYTES: usize = 64;
/// Maximum length of `notify_url` / `return_url`.
pub const MAX_CALLBACK_URL_BYTES: usize = 1024;
/// Maximum length of a custom `type` / `device` identifier.
pub const MAX_IDENTIFIER_BYTES: usize = 32;
/// Maximum `application/x-www-form-urlencoded` size of a signed request.
pub const MAX_REQUEST_BYTES: usize = 4096;
