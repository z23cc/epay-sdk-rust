# epay-sdk

彩虹易支付（EPay）V1 的 Rust SDK，为 Axum 服务设计：一个 `Client` 放进 State，回调用抽取器验签，下单 / 查询 / 退款用 async 方法。

MSRV 1.85。API 文档见 [docs.rs/epay-sdk](https://docs.rs/epay-sdk)，进阶用法见 [docs/GUIDE.md](docs/GUIDE.md)，变更见 [CHANGELOG.md](CHANGELOG.md)，安全说明见 [SECURITY.md](SECURITY.md)。

## 安装

```toml
[dependencies]
epay-sdk = { version = "1", features = ["axum"] }
axum = "0.8"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

只收回调、不调网关接口的服务可以去掉 reqwest：`default-features = false, features = ["axum"]`。

## 在 Axum 中使用

```rust
use std::net::SocketAddr;

use axum::extract::{ConnectInfo, FromRef, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use epay_sdk::axum::{NotifyAck, VerifiedNotify, VerifiedReturn, callback_route, payer_ip};
use epay_sdk::{Client, Money, PayType, PaymentRequest, ProtocolContext};

#[derive(Clone)]
struct AppState {
    epay: Client,
    // db: sqlx::PgPool, ...
}

// 回调抽取器只需要 ProtocolContext；从 Client 里取即可。
impl FromRef<AppState> for ProtocolContext {
    fn from_ref(state: &AppState) -> Self {
        state.epay.protocol_context().clone()
    }
}

// 1. 下单：返回支付跳转地址
async fn create_payment(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<String>, StatusCode> {
    let money = Money::from_yuan_str("9.90").map_err(|_| StatusCode::BAD_REQUEST)?;
    let request = PaymentRequest::new(PayType::Alipay, "ORDER001", "VIP会员", money)
        // mapi.php 要求付款者 IP；反向代理后把第三个参数设为 true（只信任你自己的代理）
        .client_ip_addr(payer_ip(&headers, peer, false));
    // 错误文本已脱敏（不含密钥 / URL），可以直接记入日志；用 error.kind() 做状态码映射见 GUIDE
    let response = state
        .epay
        .create_payment(&request)
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    // 微信 / 扫码渠道可能只返回二维码内容：用 response.destination() 按渠道处理（见 GUIDE）
    Ok(Json(response.pay_url))
}

// 2. 异步通知：进入 handler 时签名、PID、金额格式都已验证
async fn notify(State(state): State<AppState>, VerifiedNotify(data): VerifiedNotify) -> NotifyAck {
    let Ok(paid) = data.try_into_success() else {
        return NotifyAck::fail();
    };
    let paid = paid.data();
    // 按 paid.out_trade_no 加载本地订单
    // 核对 paid.money_matches(order.money)
    // 幂等地写入已支付状态（同一订单的重复通知直接返回 ok）
    // 只有持久化成功后才返回 ok，否则 fail 让网关稍后重试
    let _ = (&state, paid);
    NotifyAck::ok()
}

// 3. 同步跳转页：渲染权在你手里，未验证通过就显示"待确认"
async fn return_page(VerifiedReturn(result): VerifiedReturn) -> &'static str {
    match result.and_then(|data| data.try_into_success()) {
        Ok(_) => "支付完成",
        Err(_) => "支付结果待确认",
    }
}

fn app(epay: Client) -> Router {
    Router::new()
        .route("/pay", post(create_payment))
        .route("/notify", callback_route(notify)) // 网关用 GET 或 POST，一次注册两种
        .route("/return", callback_route(return_page))
        .with_state(AppState { epay })
    // 用 ConnectInfo 时：axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
}
```

`Client` 从环境变量或 builder 构造：

```rust,no_run
# fn demo() -> epay_sdk::Result<()> {
// EPAY_PID / EPAY_KEY / EPAY_API_URL / EPAY_NOTIFY_URL / EPAY_RETURN_URL ...
let client = epay_sdk::Client::from_env()?;

let client = epay_sdk::Client::builder(1001, "merchant-key", "https://pay.example.com")
    .notify_url("https://shop.example.com/notify")
    .return_url("https://shop.example.com/return")
    .build()?;
# let _ = client;
# Ok(())
# }
```

其它常用调用：

```rust
# async fn demo(client: &epay_sdk::Client) -> epay_sdk::Result<()> {
use epay_sdk::{FormPaymentRequest, Money, PayType};

// 页面跳转支付（submit.php）：直接返回给浏览器的自动提交表单
// （含内联 script；严格 CSP 下改用 prepare_form_payment() 自行渲染）
let html = client.build_form_payment(
    &FormPaymentRequest::new("ORDER002", "商品", Money::from_yuan_str("1.00")?)
        .pay_type(PayType::Wxpay),
)?;

// 查询与退款
let order = client.query_order_by_out_trade_no("ORDER001").await?;
if order.is_paid() {
    client.refund_by_out_trade_no("ORDER001", order.amount()?).await?;
}
# let _ = html;
# Ok(())
# }
```

## 你可以依赖的行为

- **通知验证是严格的**：`VerifiedNotify` 要求 `sign_type=MD5`、签名正确、PID 与配置一致、订单号 / 金额 / 状态结构有效，重复或冲突的字段直接拒绝；拒绝时自动回 `200 fail` 让网关重试。它不做订单查找、金额核对和幂等——那是你的 handler 的事（协议没有防重放，必须按 `out_trade_no` 幂等）。
- **金额不会悄悄变**：`Money` 只接受最多两位小数，`"9.999"` 报错而不是四舍五入；有 serde，可直接放进订单结构体。
- **网关兼容**：`mapi.php` 走 POST 并强制 `clientip`；退款走 `POST api.php?act=refund` 且接受官方成功码 `0`；PHP 返回的 `null` 映射为空值；各分支多出来的字段保留在响应的 `extra` 里。
- **安全默认值**：API 地址和回调地址都必须 HTTPS（旧网关 / 本地联调各有独立的显式放行开关）；SDK 自建的 reqwest 不跟随重定向、响应最多 1 MiB；出站请求字段和总大小有上限；商户密钥只保存一份并在释放时清零；密钥、签名、回调 query、支付链接和可能含隐私的字段（`param`、`buyer`、结算账号）在 `Debug` / 日志 / 错误文本中一律脱敏。
- **下单和退款永不自动重试**；查询接口可按需开启 `RetryPolicy`（超时是 per-attempt 的）。
- **错误可稳定分类**：`Error::kind()` 给出低基数的 `ErrorKind`，`Error::endpoint()` 给出涉及的网关端点，不必匹配 `Error` 本身。
- 公共类型都是 `#[non_exhaustive]`，`match` 请带通配分支。

## 进阶

[docs/GUIDE.md](docs/GUIDE.md) 覆盖：只用 `ProtocolContext` 的无 HTTP 服务、CSP 友好的 `PreparedForm`、支付目标地址、重试与单次调用选项（per-attempt 超时、连接超时）、分页、`extra` 字段、`ErrorKind` 与 HTTP 响应映射、旧 HTTP 网关与本地回调、多商户回调（`ParsedNotify`）、观察被拒绝的回调、出站字段上限、`blocking` / `tracing` feature、自定义 `Transport`、环境变量、用 `NotifyFixture` 和真实分支 fixture 测试。

## 验证

```sh
cargo test --all-features
cargo clippy --all-features --all-targets -- -D warnings
```

## License

MIT
