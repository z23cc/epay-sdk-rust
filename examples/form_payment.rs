//! Offline demo: build a signed jump URL and auto-submit HTML form.
//!
//! ```sh
//! cargo run --example form_payment
//! ```

use epay_sdk::{FormPaymentRequest, Money, PayType, ProtocolContext};

fn main() -> epay_sdk::Result<()> {
    let _ = env_logger::try_init();

    let epay = ProtocolContext::builder(1001, "testkey123", "https://pay.example.com")
        .notify_url("https://shop.example.com/notify")
        .return_url("https://shop.example.com/return")
        .debug(true)
        .build()?;

    let money = Money::from_yuan_str("9.90")?;
    let req = FormPaymentRequest::new("ORDER001", "VIP会员", money).pay_type(PayType::Alipay);

    let url = epay.build_form_payment_url(&req)?;
    println!("跳转 URL:\n{url}\n");

    let html = epay.build_form_payment(&req)?;
    println!("自动提交表单:\n{html}");
    Ok(())
}
