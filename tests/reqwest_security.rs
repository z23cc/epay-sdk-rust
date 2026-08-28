#![cfg(any(feature = "reqwest-rustls", feature = "reqwest-native-tls"))]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread::JoinHandle;

use epay_sdk::{Client, Error};

fn serve_once(response: &'static [u8]) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request);
        stream.write_all(response).unwrap();
        stream.flush().unwrap();
    });
    (format!("http://{address}"), handle)
}

#[tokio::test]
async fn sdk_client_does_not_follow_redirects() {
    let (base_url, server) = serve_once(
        b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/should-not-be-requested\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    let client = Client::builder(1001, "super-secret", base_url)
        .allow_insecure_http(true)
        .build()
        .unwrap();
    let error = client.query_merchant().await.unwrap_err();
    assert!(matches!(error, Error::Http { status: 302, .. }), "{error}");
    server.join().unwrap();
}

#[tokio::test]
async fn reqwest_connection_errors_never_include_url_or_key() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let client = Client::builder(1001, "super-secret", format!("http://{address}"))
        .allow_insecure_http(true)
        .timeout(std::time::Duration::from_millis(250))
        .build()
        .unwrap();
    let error = client.query_merchant().await.unwrap_err();
    let display = error.to_string();
    let debug = format!("{error:?}");
    for text in [display, debug] {
        assert!(!text.contains("super-secret"), "{text}");
        assert!(!text.contains("api.php"), "{text}");
        assert!(!text.contains("key="), "{text}");
    }
}

#[tokio::test]
async fn non_success_response_preview_is_redacted() {
    let body = br#"{"code":-1,"key":"super-secret","sign":"deadbeef"}"#;
    let response = format!(
        "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        String::from_utf8_lossy(body)
    );
    let response: &'static [u8] = Box::leak(response.into_bytes().into_boxed_slice());
    let (base_url, server) = serve_once(response);
    let client = Client::builder(1001, "k", base_url)
        .allow_insecure_http(true)
        .build()
        .unwrap();
    let error = client.query_merchant().await.unwrap_err();
    let text = error.to_string();
    assert!(matches!(error, Error::Http { status: 500, .. }));
    assert!(!text.contains("super-secret"), "{text}");
    assert!(!text.contains("deadbeef"), "{text}");
    assert!(text.contains("***"), "{text}");
    server.join().unwrap();
}

#[tokio::test]
async fn content_length_is_rejected_before_buffering() {
    let (base_url, server) = serve_once(
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 20\r\nConnection: close\r\n\r\n12345678901234567890",
    );
    let client = Client::builder(1001, "k", base_url)
        .allow_insecure_http(true)
        .max_response_body_bytes(10)
        .build()
        .unwrap();
    assert!(matches!(
        client.query_merchant().await,
        Err(Error::ResponseTooLarge { limit: 10 })
    ));
    server.join().unwrap();
}

#[tokio::test]
async fn chunked_response_is_bounded_while_streaming() {
    let (base_url, server) = serve_once(
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n6\r\n123456\r\n6\r\n789012\r\n0\r\n\r\n",
    );
    let client = Client::builder(1001, "k", base_url)
        .allow_insecure_http(true)
        .max_response_body_bytes(10)
        .build()
        .unwrap();
    assert!(matches!(
        client.query_merchant().await,
        Err(Error::ResponseTooLarge { limit: 10 })
    ));
    server.join().unwrap();
}
