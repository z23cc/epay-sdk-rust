//! Blocking wrapper around [`crate::Client`] (feature `blocking`).
//!
//! Runs each call to completion on a private single-threaded Tokio runtime.
//! Intended for scripts, CLIs and reconciliation jobs that are not already
//! async. **Do not** use it from inside a Tokio runtime — `block_on` panics
//! there; use the async [`crate::Client`] instead.

use std::future::Future;

use crate::client::CallOptions;
use crate::error::{Error, Result};
use crate::merchant::{MerchantInfo, SettlementListResponse};
use crate::money::Money;
use crate::order::{OrderDetail, OrderListResponse, OrderQueryRequest};
use crate::payment::{PaymentRequest, PaymentResponse};
use crate::refund::{RefundRequest, RefundResponse};

/// Blocking facade over an async [`crate::Client`].
pub struct Client {
    inner: crate::Client,
    runtime: tokio::runtime::Runtime,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("blocking::Client")
            .field("inner", &self.inner)
            .finish()
    }
}

impl Client {
    /// Wrap an async client with a new current-thread runtime.
    pub fn new(inner: crate::Client) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| Error::Config("failed to start the blocking runtime"))?;
        Ok(Self { inner, runtime })
    }

    /// The wrapped async client.
    pub fn inner(&self) -> &crate::Client {
        &self.inner
    }

    /// Run any future on the private runtime.
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }

    /// See [`crate::Client::create_payment`].
    pub fn create_payment(&self, request: &PaymentRequest) -> Result<PaymentResponse> {
        self.block_on(self.inner.create_payment(request))
    }

    /// See [`crate::Client::create_payment_with`].
    pub fn create_payment_with(
        &self,
        request: &PaymentRequest,
        options: &CallOptions,
    ) -> Result<PaymentResponse> {
        self.block_on(self.inner.create_payment_with(request, options))
    }

    /// See [`crate::Client::query_order`].
    pub fn query_order(&self, request: &OrderQueryRequest) -> Result<OrderDetail> {
        self.block_on(self.inner.query_order(request))
    }

    /// See [`crate::Client::query_order_by_out_trade_no`].
    pub fn query_order_by_out_trade_no(&self, out_trade_no: &str) -> Result<OrderDetail> {
        self.block_on(self.inner.query_order_by_out_trade_no(out_trade_no))
    }

    /// See [`crate::Client::query_order_by_trade_no`].
    pub fn query_order_by_trade_no(&self, trade_no: &str) -> Result<OrderDetail> {
        self.block_on(self.inner.query_order_by_trade_no(trade_no))
    }

    /// See [`crate::Client::query_orders`].
    pub fn query_orders(&self, limit: u32, page: u32) -> Result<OrderListResponse> {
        self.block_on(self.inner.query_orders(limit, page))
    }

    /// See [`crate::Client::query_all_orders`].
    pub fn query_all_orders(&self, limit: u32, max_pages: u32) -> Result<Vec<OrderDetail>> {
        self.block_on(self.inner.query_all_orders(limit, max_pages))
    }

    /// See [`crate::Client::refund`].
    pub fn refund(&self, request: &RefundRequest) -> Result<RefundResponse> {
        self.block_on(self.inner.refund(request))
    }

    /// See [`crate::Client::refund_by_out_trade_no`].
    pub fn refund_by_out_trade_no(
        &self,
        out_trade_no: &str,
        money: Money,
    ) -> Result<RefundResponse> {
        self.block_on(self.inner.refund_by_out_trade_no(out_trade_no, money))
    }

    /// See [`crate::Client::refund_by_trade_no`].
    pub fn refund_by_trade_no(&self, trade_no: &str, money: Money) -> Result<RefundResponse> {
        self.block_on(self.inner.refund_by_trade_no(trade_no, money))
    }

    /// See [`crate::Client::query_merchant`].
    pub fn query_merchant(&self) -> Result<MerchantInfo> {
        self.block_on(self.inner.query_merchant())
    }

    /// See [`crate::Client::query_settlements`].
    pub fn query_settlements(&self) -> Result<SettlementListResponse> {
        self.block_on(self.inner.query_settlements())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::MockTransport;

    #[test]
    fn blocking_calls_run_without_an_ambient_runtime() {
        let transport = MockTransport::new().push_json(r#"{"code":1,"pid":1001,"money":"1.00"}"#);
        let client = Client::new(
            crate::Client::builder(1001, "k", "https://pay.example.com")
                .transport(transport)
                .build()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(client.query_merchant().unwrap().pid, 1001);
        assert_eq!(client.inner().config().pid(), 1001);
    }
}
