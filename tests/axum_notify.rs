//! Integration tests for strict Axum callback extraction.
#![cfg(feature = "axum")]

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Path, State};
use axum::http::{Request, StatusCode, header};
use axum::response::Html;
use epay_sdk::axum::{
    NotifyAck, NotifyParams, NotifyRejection, ParsedNotify, VerifiedNotify, VerifiedReturn,
    callback_route,
};
use epay_sdk::{Money, Params, ProtocolContext, Signer};
use tower::ServiceExt;

const KEY: &str = "testkey123";

fn protocol() -> ProtocolContext {
    ProtocolContext::new(1001, KEY, "https://pay.example.com").unwrap()
}

fn signed_query(pid: &str, status: &str, money: &str) -> String {
    let mut params = Params::new();
    params.insert("pid", pid);
    params.insert("trade_no", "2024112312345678");
    params.insert("out_trade_no", "ORDER001");
    params.insert("type", "alipay");
    params.insert("name", "VIP会员");
    params.insert("money", money);
    params.insert("trade_status", status);
    Signer::new(KEY).unwrap().sign_with_params(params).encode()
}

fn signed_incomplete_query() -> String {
    let mut params = Params::new();
    params.insert("pid", "1001");
    params.insert("trade_no", "2024112312345678");
    params.insert("out_trade_no", "ORDER001");
    params.insert("type", "alipay");
    params.insert("name", "VIP会员");
    params.insert("trade_status", "TRADE_SUCCESS");
    Signer::new(KEY).unwrap().sign_with_params(params).encode()
}

async fn notify(VerifiedNotify(data): VerifiedNotify) -> NotifyAck {
    let expected = Money::from_yuan_str("9.90").unwrap();
    if data.money_matches(expected) && data.try_into_success().is_ok() {
        NotifyAck::ok()
    } else {
        NotifyAck::fail()
    }
}

/// Applications that want to count rejections extract the `Result`.
async fn observed(result: Result<VerifiedNotify, NotifyRejection>) -> String {
    match result {
        Ok(VerifiedNotify(data)) => format!("success:{}", data.out_trade_no),
        Err(NotifyRejection(error)) => format!("fail:{error}"),
    }
}

/// Multi-merchant routing: the account comes from the path, never from the
/// unverified `pid` in the payload.
async fn multi(
    State(protocol): State<ProtocolContext>,
    Path(account): Path<String>,
    ParsedNotify(params): ParsedNotify,
) -> NotifyAck {
    if account != "acct-1" {
        return NotifyAck::fail();
    }
    protocol.verify_notify(&params).map(|_| ()).into()
}

async fn raw(NotifyParams(params): NotifyParams) -> String {
    params.get("out_trade_no").unwrap_or("-").to_owned()
}

async fn return_page(VerifiedReturn(result): VerifiedReturn) -> Html<&'static str> {
    match result.and_then(|data| data.try_into_success()) {
        Ok(_) => Html("verified"),
        Err(_) => Html("pending"),
    }
}

fn app() -> Router {
    Router::new()
        .route("/notify", callback_route(notify))
        .route("/raw", callback_route(raw))
        .route("/observed", callback_route(observed))
        .route("/multi/{account}/notify", callback_route(multi))
        .route("/return", callback_route(return_page))
        .with_state(protocol())
}

async fn send(request: Request<Body>) -> (StatusCode, String) {
    let response = app().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

fn get_request(path: &str, query: &str) -> Request<Body> {
    Request::get(format!("{path}?{query}"))
        .body(Body::empty())
        .unwrap()
}

fn post(path: &str, body: String) -> Request<Body> {
    Request::post(path)
        .header(
            header::CONTENT_TYPE,
            "application/x-www-form-urlencoded; charset=UTF-8",
        )
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn valid_get_and_post_return_success() {
    let query = signed_query("1001", "TRADE_SUCCESS", "9.90");
    for request in [get_request("/notify", &query), post("/notify", query)] {
        let (status, body) = send(request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "success");
    }
}

#[tokio::test]
async fn authenticated_wrong_amount_is_handler_failure() {
    let query = signed_query("1001", "TRADE_SUCCESS", "0.01");
    let (status, body) = send(get_request("/notify", &query)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "fail");
}

#[tokio::test]
async fn tampering_foreign_pid_and_missing_fields_are_rejected() {
    let tampered =
        signed_query("1001", "TRADE_SUCCESS", "9.90").replace("money=9.90", "money=0.01");
    for query in [
        tampered,
        signed_query("2002", "TRADE_SUCCESS", "9.90"),
        signed_incomplete_query(),
        "pid=1001&out_trade_no=ORDER001".to_owned(),
    ] {
        let (status, body) = send(get_request("/notify", &query)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "fail");
    }
}

#[tokio::test]
async fn duplicate_and_conflicting_fields_are_rejected() {
    let query = signed_query("1001", "TRADE_SUCCESS", "9.90");
    let duplicate = format!("{query}&pid=1001");
    assert_eq!(send(get_request("/notify", &duplicate)).await.1, "fail");

    let request = Request::post("/notify?pid=2002")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(query))
        .unwrap();
    assert_eq!(send(request).await.1, "fail");
}

#[tokio::test]
async fn request_limits_and_content_type_are_enforced() {
    let oversized = "a".repeat(17 * 1024);
    assert_eq!(send(get_request("/notify", &oversized)).await.1, "fail");
    assert_eq!(send(post("/notify", oversized)).await.1, "fail");

    let combined = Request::post(format!("/notify?{}", "a".repeat(9 * 1024)))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("b".repeat(9 * 1024)))
        .unwrap();
    assert_eq!(send(combined).await.1, "fail");

    let request = Request::post("/notify")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(signed_query("1001", "TRADE_SUCCESS", "9.90")))
        .unwrap();
    assert_eq!(send(request).await.1, "fail");
}

#[tokio::test]
async fn rejection_can_be_observed_by_the_handler() {
    let good = signed_query("1001", "TRADE_SUCCESS", "9.90");
    let (status, body) = send(get_request("/observed", &good)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "success:ORDER001");

    let (status, body) = send(get_request("/observed", "pid=1001&sign=bad")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with("fail:"), "{body}");
}

#[tokio::test]
async fn parsed_notify_supports_multi_merchant_routing() {
    let good = signed_query("1001", "TRADE_SUCCESS", "9.90");
    assert_eq!(
        send(get_request("/multi/acct-1/notify", &good)).await.1,
        "success"
    );
    assert_eq!(
        send(get_request("/multi/acct-2/notify", &good)).await.1,
        "fail"
    );

    let (status, body) = send(get_request("/multi/acct-1/notify", "name=%ZZ")).await;
    assert_eq!(status, StatusCode::OK, "malformed input still answers 200");
    assert_eq!(body, "fail");
}

#[tokio::test]
async fn raw_extractor_is_fallible() {
    let request = post("/raw", "name=%ZZ".to_owned());
    let (status, _) = send(request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn verified_return_leaves_rendering_to_handler() {
    let good = signed_query("1001", "TRADE_SUCCESS", "9.90");
    assert_eq!(send(get_request("/return", &good)).await.1, "verified");
    assert_eq!(send(get_request("/return", "sign=bad")).await.1, "pending");
}
