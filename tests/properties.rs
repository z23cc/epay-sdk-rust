//! Property tests: parsers and encoders never panic and keep their invariants.

use epay_sdk::{Money, Params, ProtocolContext, Signer, parse_raw_notify_params};
use proptest::prelude::*;

fn form_key() -> impl Strategy<Value = String> {
    "[a-z_]{1,12}"
}

fn form_value() -> impl Strategy<Value = String> {
    // Printable ASCII plus a few CJK characters, then percent-encoded by Params.
    "[ -~\u{4e2d}\u{6587}]{0,24}"
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Arbitrary bytes never panic the strict notify parser, and accepted
    /// input canonicalizes deterministically.
    #[test]
    fn notify_parser_never_panics(query in ".{0,256}", body in ".{0,256}") {
        let first = parse_raw_notify_params(Some(&query), Some(&body));
        let second = parse_raw_notify_params(Some(&query), Some(&body));
        match (first, second) {
            (Ok(a), Ok(b)) => {
                prop_assert_eq!(a.entries().len(), b.entries().len());
                let _ = a.canonicalize();
            }
            (Err(_), Err(_)) => {}
            _ => prop_assert!(false, "parser is not deterministic"),
        }
    }

    /// Duplicate keys are always rejected at canonicalization.
    #[test]
    fn duplicate_fields_are_rejected(key in form_key(), a in form_value(), b in form_value()) {
        let mut p = Params::new();
        p.insert(key.clone(), a);
        let mut q = Params::new();
        q.insert(key, b);
        let query = format!("{}&{}", p.encode(), q.encode());
        if p.len() == 1 && q.len() == 1 {
            let raw = parse_raw_notify_params(Some(&query), None).unwrap();
            prop_assert!(raw.canonicalize().is_err());
        }
    }

    /// Encoding round-trips through the strict parser and the signature is
    /// independent of insertion order.
    #[test]
    fn params_encode_round_trips_and_sign_is_order_independent(
        pairs in prop::collection::btree_map(form_key(), form_value(), 0..8)
    ) {
        let forward: Params = pairs.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let backward: Params = pairs.iter().rev().map(|(k, v)| (k.clone(), v.clone())).collect();
        prop_assert_eq!(forward.encode(), backward.encode());

        let signer = Signer::new("testkey123").unwrap();
        prop_assert_eq!(signer.sign(&forward), signer.sign(&backward));
        prop_assert!(signer.verify(&forward, &signer.sign(&backward)));

        let encoded = forward.encode();
        let parsed = parse_raw_notify_params(Some(&encoded), None)
            .unwrap()
            .canonicalize()
            .unwrap();
        prop_assert_eq!(parsed, forward);
    }

    /// Any string either fails to parse or yields a positive, two-decimal
    /// amount whose wire form parses back to the same value.
    #[test]
    fn money_strings_never_panic_and_round_trip(s in ".{0,32}") {
        if let Ok(money) = Money::from_yuan_str(&s) {
            let wire = money.to_epay_string();
            prop_assert!(wire.contains('.'));
            prop_assert_eq!(wire.rsplit('.').next().unwrap().len(), 2);
            prop_assert_eq!(Money::from_yuan_str(&wire).unwrap(), money);
            prop_assert!(money.decimal() > rust_decimal::Decimal::ZERO);
        }
        let _ = Money::from_yuan_str_rounded(&s);
    }

    /// Form HTML never contains an unescaped attribute delimiter from user
    /// input, whatever the product name is.
    #[test]
    fn auto_submit_form_escapes_everything(name in "[^\u{0}-\u{1f}\u{7f}]{1,40}") {
        let epay = ProtocolContext::builder(1001, "k", "https://pay.example.com")
            .notify_url("https://shop.example.com/notify")
            .build()
            .unwrap();
        let request = epay_sdk::FormPaymentRequest::new(
            "ORDER001",
            name.clone(),
            Money::from_yuan_str("1.00").unwrap(),
        );
        if let Ok(form) = epay.prepare_form_payment(&request) {
            let inputs = form.hidden_inputs();
            for attr in inputs.split("value=\"").skip(1) {
                let value = attr.split('"').next().unwrap();
                prop_assert!(!value.contains('<') && !value.contains('>') && !value.contains('"'));
                prop_assert!(!value.contains("&") || value.contains("&amp;") || value.contains("&lt;")
                    || value.contains("&gt;") || value.contains("&quot;") || value.contains("&#39;"));
            }
            prop_assert!(form.auto_submit_html().matches("<script").count() == 1);
        }
    }
}
