//! Sanitized responses captured from real 易支付 deployments, replayed through
//! the client so envelope handling, null tolerance and validation stay
//! compatible with what forks actually send.
#![cfg(feature = "test-util")]

use epay_sdk::test_util::MockTransport;
use epay_sdk::{Client, Money, PayType, PaymentRequest};

fn fixture(name: &str) -> &'static str {
    match name {
        "order_paid_nullable_columns" => include_str!("fixtures/order_paid_nullable_columns.json"),
        "orders_string_numbers" => include_str!("fixtures/orders_string_numbers.json"),
        "orders_empty_null_data" => include_str!("fixtures/orders_empty_null_data.json"),
        "merchant_query_echoes_key" => include_str!("fixtures/merchant_query_echoes_key.json"),
        "settle_null_uid" => include_str!("fixtures/settle_null_uid.json"),
        "refund_success_code_zero" => include_str!("fixtures/refund_success_code_zero.json"),
        "mapi_qrcode_with_nulls" => include_str!("fixtures/mapi_qrcode_with_nulls.json"),
        other => panic!("unknown fixture {other}"),
    }
}

fn client(transport: MockTransport) -> Client {
    Client::builder(1001, "testkey123", "https://pay.example.com")
        .notify_url("https://shop.example.com/notify")
        .transport(transport)
        .build()
        .unwrap()
}

#[tokio::test]
async fn order_with_nullable_columns() {
    let order = client(MockTransport::new().push_json(fixture("order_paid_nullable_columns")))
        .query_order_by_out_trade_no("LIVEFORM0000000000000000000001")
        .await
        .unwrap();
    assert!(order.is_paid());
    assert_eq!(order.amount().unwrap().to_epay_string(), "0.01");
    assert_eq!(order.pay_type, "wxpay");
    assert_eq!(order.buyer, "");
    assert!(order.extra["bill_trade_no"].is_null());
    assert!(!order.extra.contains_key("code"));
}

#[tokio::test]
async fn orders_with_string_numbers_and_null_data() {
    let transport = MockTransport::new()
        .push_json(fixture("orders_string_numbers"))
        .push_json(fixture("orders_empty_null_data"));
    let client = client(transport);
    let page = client.query_orders(50, 1).await.unwrap();
    assert_eq!(page.count, 2);
    assert_eq!(page.orders.len(), 2);
    assert!(page.orders[0].is_paid());
    assert!(page.orders[1].is_unpaid());
    assert_eq!(page.orders[1].name, "");
    assert!(!page.has_more(50));

    let empty = client.query_orders(50, 2).await.unwrap();
    assert!(empty.orders.is_empty());
}

#[tokio::test]
async fn merchant_query_strips_echoed_key() {
    let info = client(MockTransport::new().push_json(fixture("merchant_query_echoes_key")))
        .query_merchant()
        .await
        .unwrap();
    assert_eq!(info.pid, 1001);
    assert!(info.is_active());
    assert_eq!(info.balance().unwrap().to_string(), "123.45");
    assert!(!info.extra.contains_key("key"));
    assert_eq!(info.extra["orders"], "42");
    assert!(info.extra["order_lastday"].is_null());
    let debug = format!("{info:?}");
    assert!(!debug.contains("REDACTED-MERCHANT-KEY") && !debug.contains("pay@example.com"));
}

#[tokio::test]
async fn settlements_with_null_uid_are_accepted() {
    let list = client(MockTransport::new().push_json(fixture("settle_null_uid")))
        .query_settlements()
        .await
        .unwrap();
    assert_eq!(list.settlements.len(), 1);
    assert_eq!(list.settlements[0].uid, 0);
    assert_eq!(list.settlements[0].money, "100.00");
    assert!(!format!("{list:?}").contains("6222000011112222"));
}

#[tokio::test]
async fn refund_success_code_zero() {
    let response = client(MockTransport::new().push_json(fixture("refund_success_code_zero")))
        .refund_by_out_trade_no("O1", Money::from_yuan_str("0.01").unwrap())
        .await
        .unwrap();
    assert_eq!(response.msg, "退款成功");
}

#[tokio::test]
async fn mapi_qrcode_with_null_destinations() {
    let response = client(MockTransport::new().push_json(fixture("mapi_qrcode_with_nulls")))
        .create_payment(
            &PaymentRequest::new(
                PayType::Wxpay,
                "O1",
                "P",
                Money::from_yuan_str("0.01").unwrap(),
            )
            .client_ip("203.0.113.9"),
        )
        .await
        .unwrap();
    assert_eq!(response.trade_no, "2026082800000000003");
    assert_eq!(response.pay_url, "");
    let destination = response.destination().unwrap();
    assert_eq!(destination.parse_url().unwrap().scheme(), "weixin");
    assert_eq!(response.extra["payType"], "jump");
}
