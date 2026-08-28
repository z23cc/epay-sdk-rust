# epay-sdk 进阶指南

README 只讲 Axum 的最短路径；这里是其余细节。所有代码片段都会作为 doctest 编译（`cargo test --all-features`）。

## 目录

- [架构](#架构)
- [Features](#features)
- [无 HTTP 的协议上下文](#无-http-的协议上下文)
- [分支特有字段](#分支特有字段)
- [重试与单次调用选项](#重试与单次调用选项)
- [分页](#分页)
- [错误处理](#错误处理)
- [阻塞调用](#阻塞调用blocking-feature)
- [可观测性](#可观测性)
- [旧 HTTP 网关](#旧-http-网关)
- [其它 Web 框架](#其它-web-框架)
- [金额](#金额)
- [环境变量](#环境变量)
- [自定义 HTTP](#自定义-http)
- [测试回调](#测试回调)
- [0.1 → 0.2 迁移](#01--02-迁移)

## 架构

```text
ProtocolContext（无 HTTP / 无运行时依赖）
├── Config：商户号、密钥、网关地址、回调默认值
├── MD5 签名和验签
├── 表单支付 URL / HTML
└── 严格通知验证

Client
├── ProtocolContext
├── HttpConfig：超时、响应大小上限、RetryPolicy
├── reqwest 或自定义 Transport
├── mapi.php / api.php 异步接口（按 Endpoint 枚举分发）
└── CallOptions：单次调用的超时 / 重试覆盖

axum feature（axum 关闭默认特性，不引入 tokio / hyper）
├── VerifiedNotify / VerifiedReturn / NotifyParams 抽取器
├── NotifyAck
├── callback_route：GET + POST 一次注册
└── payer_ip：反代后的付款者 IP

blocking feature  → blocking::Client
tracing feature   → 每次网关调用一个 span
test-util feature → MockTransport、NotifyFixture
```

- 网关响应先校验 `code` / `msg` 包络，再解析成功结构；成功结构不携带 `code`（退款回执保留 `msg`）。
- `api.php` 只使用官方 `pid + key` 鉴权。
- 通知 query + form 合计最多 16 KiB，重复字段和冲突字段直接拒绝。

## Features

| Feature | 默认 | 用途 |
|---|---:|---|
| `reqwest-rustls` | 是 | reqwest + rustls |
| `reqwest-native-tls` | 否 | reqwest + native-tls |
| `axum` | 否 | Axum 0.8 抽取器与路由助手 |
| `blocking` | 否 | `blocking::Client`，面向脚本 / CLI / 对账任务 |
| `tracing` | 否 | 每次网关调用一个 `tracing` span，日志改走 `tracing`（不再发 `log`） |
| `test-util` | 否 | `test_util`：`MockTransport`、`NotifyFixture` |

## 无 HTTP 的协议上下文

只建表单、只收回调的服务不需要 reqwest 和 Tokio：

```rust
use epay_sdk::{FormPaymentRequest, Money, PayType, ProtocolContext};

let epay = ProtocolContext::builder(1001, "merchant-key", "https://pay.example.com")
    .notify_url("https://shop.example.com/notify")
    .return_url("https://shop.example.com/return")
    .build()?;

let request = FormPaymentRequest::new("ORDER001", "VIP会员", Money::from_yuan_str("9.90")?)
    .pay_type(PayType::Alipay);

let html = epay.build_form_payment(&request)?;
let url = epay.build_form_payment_url(&request)?;
# let _ = (html, url);
# Ok::<(), epay_sdk::Error>(())
```

`Client` 上的同名方法只是转发到 `client.protocol_context()`。

## 分支特有字段

每个响应结构体都有 `extra: BTreeMap<String, serde_json::Value>`，保留网关返回但 SDK 未建模的字段；包络字段 `code` 已被消费，回显的 `key` / `sign` 被剔除，都不会出现在其中。

```rust
# async fn demo(client: &epay_sdk::Client) -> epay_sdk::Result<()> {
let order = client.query_order_by_out_trade_no("ORDER001").await?;
if let Some(fee) = order.extra.get("pay_fee").and_then(|v| v.as_str()) {
    println!("手续费 {fee}");
}
# Ok(())
# }
```

## 重试与单次调用选项

默认不重试。`RetryPolicy` 只对 **GET** 端点（订单 / 商户 / 结算查询）且只对 `Error::is_retryable()` 为真的失败（超时、连接失败、502/503/504）生效；`mapi.php` 下单和退款永远不自动重试。

```rust
use std::time::Duration;
use epay_sdk::{CallOptions, Client, RetryPolicy};

# async fn demo() -> epay_sdk::Result<()> {
let client = Client::builder(1001, "merchant-key", "https://pay.example.com")
    .retry(RetryPolicy::reads()) // 2 次，200 ms → 2 s 指数退避
    .build()?;

// 单次调用覆盖超时和重试
let options = CallOptions::new()
    .timeout(Duration::from_secs(60))
    .retry(RetryPolicy::NONE);
let merchant = client.query_merchant_with(&options).await?;
println!("{}", merchant.money);
# Ok(())
# }
```

自定义 `Transport` 必须实现 `sleep`（用运行时的定时器），退避才能生效。

## 分页

`query_orders(limit, page)` 返回一页；`OrderListResponse::next_page(page, limit)` 在页满时给出下一页（不依赖各分支含义不一的 `count`）。`query_all_orders(limit, max_pages)` 有界地收集多页：

```rust
# async fn demo(client: &epay_sdk::Client) -> epay_sdk::Result<()> {
let recent = client.query_all_orders(50, 4).await?; // 最多 200 笔
# let _ = recent;
# Ok(())
# }
```

## 错误处理

```rust
use epay_sdk::{Endpoint, Error, TransportErrorKind};

fn classify(error: &Error) -> &'static str {
    match error {
        Error::Api { endpoint: Endpoint::Refund, code, .. } if *code < 0 => "refund rejected",
        Error::Api { .. } => "gateway error",
        Error::Transport { kind: TransportErrorKind::Timeout, .. } => "retry later",
        Error::VerifyFailed => "bad signature",
        _ => "other",
    }
}
```

`Error` 是 `#[non_exhaustive]`；`Error::code()` 提供与 Go SDK 一致的数字码，`Error::transport_kind()` / `is_timeout()` / `is_retryable()` 提供传输层分类。所有错误文本都会对 `key` / `sign` 脱敏；内置 reqwest transport 的错误只包含固定分类消息，不含请求 URL。

SDK 有意不为 `Error` 实现 `IntoResponse`：网关错误文案不应该原样送到浏览器。在 Axum 里用一个 newtype 统一映射：

```rust
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use epay_sdk::Error;

struct AppError(Error);

impl From<Error> for AppError {
    fn from(error: Error) -> Self {
        Self(error)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            Error::Param(_) | Error::Notify { .. } => StatusCode::BAD_REQUEST,
            Error::Api { .. } => StatusCode::BAD_GATEWAY,
            Error::Transport { .. } | Error::Http { .. } => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        log::warn!("epay call failed: {}", self.0);
        (status, "payment service error").into_response()
    }
}

// handler 返回 Result<_, AppError> 后就能直接 `?`
```

## 阻塞调用（`blocking` feature）

```rust,ignore
let client = epay_sdk::blocking::Client::new(
    epay_sdk::Client::new(1001, "merchant-key", "https://pay.example.com")?,
)?;
let merchant = client.query_merchant()?;
```

不要在 Tokio 运行时内部使用；那里直接用 async `Client`。

## 可观测性

`debug(true)` 时输出脱敏后的请求 / 响应。未开启 `tracing` feature 时走 `log`；开启后**只**走 `tracing`（不会同时发 `log`，因此安装了 `LogTracer` 的应用不会看到重复事件），每次网关调用都在 `epay.request` span（字段 `endpoint`、`method`）内执行，重试和回调拒绝以 `warn` 事件记录。

## 旧 HTTP 网关

不安全 HTTP 不会被静默接受：

```rust,no_run
let client = epay_sdk::Client::builder(1001, "key", "http://legacy.example.com")
    .allow_insecure_http(true)
    .build()?;
# Ok::<(), epay_sdk::Error>(())
```

生产环境应始终使用 HTTPS。

## 其它 Web 框架

```rust
# fn demo(protocol: &epay_sdk::ProtocolContext, query: Option<&str>, form_body: Option<&str>) -> epay_sdk::Result<()> {
let params = epay_sdk::parse_notify_params(query, form_body)?;
let data = protocol.verify_notify(&params)?;
# let _ = data;
# Ok(())
# }
```

调用方仍需先限制原始 HTTP 输入大小；也可以使用 `parse_raw_notify_params_with_limits`。回调必须同时接受 GET 和 POST。

## 金额

```rust
assert!(epay_sdk::Money::from_yuan_str("9.999").is_err());
let exact = epay_sdk::Money::from_yuan_str("9.90")?;
let rounded = epay_sdk::Money::from_yuan_str_rounded("9.999")?;
assert_eq!(rounded.to_string(), "10.00");
assert_eq!(exact.try_to_fen()?, 990);
# Ok::<(), epay_sdk::Error>(())
```

只有显式带 `_rounded` 的 API 才会四舍五入。`try_to_fen()` 在超出 `i64` 范围时返回错误而不是饱和。核对回调金额请用 `NotifyData::money_matches(order.money)`，不要先把回调金额转成分再入账。`Money` 序列化为 `"9.90"` 字符串；反序列化只接受字符串和整数 JSON 数字，带小数的 JSON 数字会被拒绝（它到达时已经是 f64，无法证明精确）。

## 环境变量

```text
EPAY_PID=1001
EPAY_KEY=merchant-key
EPAY_API_URL=https://pay.example.com
EPAY_NOTIFY_URL=https://shop.example.com/notify
EPAY_RETURN_URL=https://shop.example.com/return
EPAY_DEBUG=false
EPAY_ALLOW_INSECURE_HTTP=false
EPAY_TIMEOUT_SECS=30             # 仅 Client
EPAY_MAX_RESPONSE_BYTES=1048576  # 仅 Client
EPAY_MAX_RETRIES=0               # 仅 Client，GET 端点重试次数
```

`ProtocolContext::from_env` 读取前七项，`Client::from_env` 额外读取三项 HTTP 设置。非法数字或布尔值会在启动时失败，不会静默回退。

## 自定义 HTTP

```rust
use epay_sdk::{transport::BoxFuture, Error, Result, Transport, TransportErrorKind, TransportOptions, Url};

struct CustomTransport;

impl Transport for CustomTransport {
    fn get<'a>(
        &'a self,
        url: &'a Url,
        options: TransportOptions,
    ) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            let _ = (url, options.max_response_bytes, options.timeout);
            Err(Error::transport(TransportErrorKind::Other, "not implemented"))
        })
    }

    fn post_form<'a>(
        &'a self,
        url: &'a Url,
        form: &'a [(&'a str, &'a str)],
        options: TransportOptions,
    ) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            let _ = (url, form, options);
            Err(Error::transport(TransportErrorKind::Other, "not implemented"))
        })
    }

    fn sleep(&self, duration: std::time::Duration) -> BoxFuture<'_, ()> {
        Box::pin(tokio::time::sleep(duration))
    }
}
```

自定义 transport 必须边读边执行 `options.max_response_bytes`，尽量尊重 `options.timeout`（`CallOptions` 未设置时为 `HttpConfig` 的超时），用 `Error::transport` 构造传输错误、用 `Error::http(status, body)` 报告非 2xx 响应（会自动截断并脱敏 body）。`Error::transport` 只会遮盖 `key=` / `sign=` 片段，不会删除 URL，因此消息里不要放请求 URL（GET 请求的 URL 含商户密钥和订单号）；`Client` 会在返回后再次检查长度并再次脱敏。

复用自定义 `reqwest::Client` 时，其 TLS 和重定向策略由调用方负责（SDK 不会替它关掉重定向）；SDK 仍会逐请求应用超时、执行响应上限，并把 reqwest 错误分类成不含 URL 的固定消息。

## 测试回调

开启 `test-util` 后，`NotifyFixture` 一行生成签名正确的通知，配合 `tower::ServiceExt::oneshot` 就能测完整的回调链路：

```rust
use epay_sdk::test_util::NotifyFixture;
use epay_sdk::{Money, ProtocolContext};

# fn demo() -> epay_sdk::Result<()> {
let epay = ProtocolContext::new(1001, "testkey123", "https://pay.example.com")?;
let query = NotifyFixture::new(1001, "testkey123", "ORDER001", Money::from_yuan_str("9.90")?)
    .trade_no("T42")
    .query();
// GET /notify?{query}  → 期望 body "success"
// 篡改场景：直接改 query 验证拒绝路径，或用 unsigned_params() 改完再自己签
# let _ = (epay, query);
# Ok(())
# }
```

网关接口用 `MockTransport` 脚本化响应：`Client::builder(..).transport(MockTransport::new().push_json(r#"{"code":1,...}"#))`；`recorded()` 里的 `query` / `form` 是 `Params`，`Debug` 输出会脱敏 `key` / `sign`，用 `get()` 断言。

## 0.1 → 0.2 迁移

- 表单/验签服务：`Client` → `ProtocolContext`。
- Axum `FromRef<AppState>`：返回 `ProtocolContext`，不再要求完整 HTTP Client。
- 超时和响应上限移入 `HttpConfig`，只在 `ClientBuilder` 上设置。
- 响应结构体不再包含 `code` / `is_success()`（`RefundResponse::msg` 作为退款回执保留）；错误通过 `Error::Api { endpoint, code, .. }` 报告。
- `PayType` / `Device` / `OrderStatus` / `Endpoint` / `Error` 为 `#[non_exhaustive]`，`match` 需要通配分支。
- `Error::Transport` 变为 `{ kind, message }`；通知类错误统一为 `Error::Notify`。
- `PaymentRequest::validate` / `FormPaymentRequest::validate` 不再接收默认 `notify_url`。
- `Signer::new`、`parse_notify_params`、`generate_out_trade_no` 现在返回 `Result`。
- `NotifyData::money` 现在是 `Money`；签名字段不再保留在 domain object。
- `Money` 默认不再隐式舍入。
- `MerchantApiAuth` 和 `EPAY_API_AUTH` 已删除。
- `Config` 字段改为只读访问器，API URL 使用 `Url`。
- `Transport` 方法接收 `&Url` 和 `TransportOptions`，`post_form` 接收 `&[(&str, &str)]`，`sleep` 必须实现。
- 响应结构体新增 `extra` 字段。
- `MockTransport` 需要 `test-util` feature。

完整变更见 [CHANGELOG.md](../CHANGELOG.md)。
