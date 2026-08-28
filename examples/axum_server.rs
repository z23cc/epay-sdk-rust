//! Axum example: form/API payment, strict callback verification, and return-page verification.
//!
//! ```sh
//! EPAY_PID=1001 EPAY_KEY=your-key EPAY_API_URL=https://pay.example.com \
//! EPAY_NOTIFY_URL=http://localhost:8080/notify \
//! EPAY_RETURN_URL=http://localhost:8080/return \
//! EPAY_ALLOW_INSECURE_CALLBACK_HTTP=1 \
//! cargo run --example axum_server --features axum
//! ```

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, FromRef, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect};
use axum::routing::{get, post};
use axum::{Json, Router};
use epay_sdk::axum::{NotifyAck, VerifiedNotify, VerifiedReturn, callback_route, payer_ip};
use epay_sdk::{
    Client, Device, FormPaymentRequest, Money, PayType, PaymentRequest, ProtocolContext,
    generate_out_trade_no,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

#[derive(Clone)]
struct AppState {
    client: Client,
    orders: Arc<RwLock<HashMap<String, Order>>>,
}

impl FromRef<AppState> for ProtocolContext {
    fn from_ref(state: &AppState) -> Self {
        state.client.protocol_context().clone()
    }
}

/// `Money` serializes as the wire string, so it can live in order records.
#[derive(Clone, Serialize)]
struct Order {
    out_trade_no: String,
    trade_no: String,
    name: String,
    money: Money,
    pay_type: String,
    paid: bool,
}

#[derive(Deserialize)]
struct PayQuery {
    #[serde(rename = "type", default)]
    pay_type: String,
    #[serde(default = "default_name")]
    name: String,
    money: String,
}

#[derive(Deserialize)]
struct CreateBody {
    pay_type: Option<String>,
    name: Option<String>,
    money: String,
}

fn default_name() -> String {
    "商品".into()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = env_logger::try_init();
    // Fall back to demo settings only when EPay is entirely unconfigured.
    const EPAY_VARS: &[&str] = &[
        "EPAY_PID",
        "EPAY_KEY",
        "EPAY_API_URL",
        "EPAY_NOTIFY_URL",
        "EPAY_RETURN_URL",
        "EPAY_DEBUG",
        "EPAY_ALLOW_INSECURE_HTTP",
        "EPAY_ALLOW_INSECURE_CALLBACK_HTTP",
        "EPAY_TIMEOUT_SECS",
        "EPAY_MAX_RESPONSE_BYTES",
    ];
    let configured = EPAY_VARS
        .iter()
        .any(|name| std::env::var_os(name).is_some());
    let client = if configured {
        Client::from_env()?
    } else {
        eprintln!("未设置 EPAY_* 环境变量，使用 HTTPS 演示配置（无法真正下单）");
        Client::builder(1001, "demo-key", "https://pay.example.com")
            .notify_url("http://localhost:8080/notify")
            .return_url("http://localhost:8080/return")
            .allow_insecure_callback_http(true) // demo only
            .timeout(Duration::from_secs(30))
            .debug(true)
            .build()?
    };
    let state = AppState {
        client,
        orders: Arc::new(RwLock::new(HashMap::new())),
    };
    let app = Router::new()
        .route("/", get(index))
        .route("/pay/form", get(form_pay))
        .route("/pay/url", get(url_pay))
        .route("/pay/qrcode", post(qrcode_pay))
        .route("/pay/query", get(query_order))
        .route("/notify", callback_route(notify))
        .route("/return", callback_route(return_page))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
    eprintln!("EPay Axum example: http://127.0.0.1:8080");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

async fn index() -> Html<&'static str> {
    Html(
        r#"<!doctype html><meta charset="utf-8"><h1>EPay Axum</h1>
<a href="/pay/form?type=alipay&name=测试商品&money=0.01">表单支付</a>"#,
    )
}

async fn form_pay(
    State(state): State<AppState>,
    Query(query): Query<PayQuery>,
) -> Result<Html<String>, (StatusCode, String)> {
    let money = parse_money(&query.money)?;
    let order_no = new_order_no("ORDER")?;
    remember(&state, &order_no, &query.name, money, &query.pay_type).await;
    let mut request = FormPaymentRequest::new(&order_no, &query.name, money);
    if !query.pay_type.is_empty() {
        request = request.pay_type(PayType::from(query.pay_type.as_str()));
    }
    state
        .client
        .build_form_payment(&request)
        .map(Html)
        .map_err(internal)
}

async fn url_pay(
    State(state): State<AppState>,
    Query(query): Query<PayQuery>,
) -> Result<Redirect, (StatusCode, String)> {
    let money = parse_money(&query.money)?;
    let order_no = new_order_no("URL")?;
    remember(&state, &order_no, &query.name, money, &query.pay_type).await;
    let mut request = FormPaymentRequest::new(&order_no, &query.name, money);
    if !query.pay_type.is_empty() {
        request = request.pay_type(PayType::from(query.pay_type.as_str()));
    }
    state
        .client
        .build_form_payment_url(&request)
        .map(|url| Redirect::to(&url))
        .map_err(internal)
}

async fn qrcode_pay(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<CreateBody>,
) -> impl IntoResponse {
    let money = match Money::from_yuan_str(&body.money) {
        Ok(money) => money,
        Err(error) => return json_error(StatusCode::BAD_REQUEST, error.to_string()),
    };
    let name = body.name.unwrap_or_else(|| "商品".into());
    let pay_type = body.pay_type.unwrap_or_else(|| "alipay".into());
    let order_no = match generate_out_trade_no("API") {
        Ok(value) => value,
        Err(error) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    remember(&state, &order_no, &name, money, &pay_type).await;
    // Set the flag to `true` only behind a reverse proxy you control.
    let trust_proxy_headers = std::env::var_os("TRUST_PROXY_HEADERS").is_some();
    let request = PaymentRequest::new(PayType::from(pay_type.as_str()), &order_no, &name, money)
        .device(Device::Pc)
        .client_ip_addr(payer_ip(&headers, peer, trust_proxy_headers));
    match state.client.create_payment(&request).await {
        Ok(response) => {
            if let Some(order) = state.orders.write().await.get_mut(&order_no) {
                order.trade_no.clone_from(&response.trade_no);
            }
            (
                StatusCode::OK,
                Json(serde_json::json!({"success": true, "data": {
                    "out_trade_no": order_no,
                    "trade_no": response.trade_no,
                    "pay_url": response.pay_url,
                    "qr_code": response.qr_code,
                    "url_scheme": response.url_scheme,
                }})),
            )
        }
        Err(error) => {
            log::warn!(
                "create_payment failed (transport kind: {:?}): {error}",
                error.transport_kind()
            );
            json_error(StatusCode::BAD_GATEWAY, error.to_string())
        }
    }
}

async fn query_order(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let request = if let Some(value) = query.get("out_trade_no") {
        epay_sdk::OrderQueryRequest::out_trade_no(value)
    } else if let Some(value) = query.get("trade_no") {
        epay_sdk::OrderQueryRequest::trade_no(value)
    } else {
        return json_error(
            StatusCode::BAD_REQUEST,
            "order identifier is required".into(),
        );
    };
    match state.client.query_order(&request).await {
        Ok(order) => (
            StatusCode::OK,
            Json(serde_json::json!({"success": true, "data": {
                "out_trade_no": order.out_trade_no,
                "trade_no": order.trade_no,
                "paid": order.is_paid(),
                "money": order.money,
            }})),
        ),
        Err(error) => json_error(StatusCode::BAD_GATEWAY, error.to_string()),
    }
}

async fn notify(State(state): State<AppState>, VerifiedNotify(data): VerifiedNotify) -> NotifyAck {
    let successful = match data.try_into_success() {
        Ok(successful) => successful,
        Err(_) => return NotifyAck::fail(),
    };
    let data = successful.data();
    let mut orders = state.orders.write().await;
    let Some(order) = orders.get_mut(&data.out_trade_no) else {
        log::warn!("notify for unknown order {}", data.out_trade_no);
        return NotifyAck::fail();
    };
    if !data.money_matches(order.money) {
        log::error!("amount mismatch for {}", data.out_trade_no);
        return NotifyAck::fail();
    }
    // Real applications must commit an idempotent state transition before ack.
    order.paid = true;
    order.trade_no.clone_from(&data.trade_no);
    NotifyAck::ok()
}

async fn return_page(VerifiedReturn(result): VerifiedReturn) -> Html<String> {
    match result.and_then(|data| data.try_into_success()) {
        Ok(successful) => {
            let data = successful.data();
            // `out_trade_no` is validated to `[A-Za-z0-9._|-]`; no HTML escaping needed.
            Html(format!(
                "<h1>支付完成</h1><p>订单号: {}</p><p>金额: {} 元</p>",
                data.out_trade_no, data.money
            ))
        }
        Err(_) => Html("<h1>支付结果待确认</h1>".to_owned()),
    }
}

async fn remember(state: &AppState, no: &str, name: &str, money: Money, pay_type: &str) {
    state.orders.write().await.insert(
        no.to_owned(),
        Order {
            out_trade_no: no.to_owned(),
            trade_no: String::new(),
            name: name.to_owned(),
            money,
            pay_type: pay_type.to_owned(),
            paid: false,
        },
    );
}

fn parse_money(value: &str) -> Result<Money, (StatusCode, String)> {
    Money::from_yuan_str(value).map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))
}

fn new_order_no(prefix: &str) -> Result<String, (StatusCode, String)> {
    generate_out_trade_no(prefix).map_err(internal)
}

fn internal(error: epay_sdk::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

fn json_error(status: StatusCode, message: String) -> (StatusCode, Json<serde_json::Value>) {
    (
        status,
        Json(serde_json::json!({"success": false, "message": message})),
    )
}
