//! Mock on-chain provider for downstream integration tests.
//!
//! [`MockOnChainProvider`] emits a scripted sequence of [`ChainPaymentUpdate`]s
//! so that consumers (such as `lnvps-api`) can integration-test their monitoring
//! loops without a real node. Enabled by the `mock` feature.

use crate::currency::CurrencyAmount;
use crate::onchain::{
    ChainPaymentUpdate, NewAddressRequest, NewAddressResponse, OnChainProvider, PaymentCursor,
    SendCoinsRequest, SendCoinsResponse,
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

/// A scripted, in-memory [`OnChainProvider`] for tests.
///
/// Addresses are handed out from a predefined pool (falling back to a generated
/// placeholder), and [`subscribe_payments`](OnChainProvider::subscribe_payments)
/// replays a fixed list of updates, honouring the resume cursor by skipping any
/// update whose block height is at or below the cursor.
#[derive(Clone, Default)]
pub struct MockOnChainProvider {
    addresses: Arc<Mutex<Vec<String>>>,
    /// Scripted updates paired with the block height at which they occur.
    updates: Arc<Vec<(u64, ChainPaymentUpdate)>>,
    /// Every [`send_coins`](OnChainProvider::send_coins) request, recorded in
    /// order so tests can assert what was sent.
    sends: Arc<Mutex<Vec<SendCoinsRequest>>>,
}

impl MockOnChainProvider {
    /// Create a mock provider with a pool of addresses to hand out and a
    /// script of `(block_height, update)` pairs to replay.
    pub fn new(addresses: Vec<String>, updates: Vec<(u64, ChainPaymentUpdate)>) -> Self {
        Self {
            addresses: Arc::new(Mutex::new(addresses)),
            updates: Arc::new(updates),
            sends: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// All [`send_coins`](OnChainProvider::send_coins) requests received so far,
    /// in call order.
    pub fn sends(&self) -> Vec<SendCoinsRequest> {
        self.sends.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Select the updates that occur strictly after the given cursor height.
    ///
    /// Exposed for direct testing of the resume semantics.
    pub fn updates_after(&self, from: Option<&PaymentCursor>) -> Vec<ChainPaymentUpdate> {
        let min_height = from.map(|c| c.block_height).unwrap_or(0);
        self.updates
            .iter()
            .filter(|(height, _)| *height > min_height)
            .map(|(_, u)| u.clone())
            .collect()
    }
}

#[async_trait]
impl OnChainProvider for MockOnChainProvider {
    async fn new_address(&self, req: NewAddressRequest) -> Result<NewAddressResponse> {
        let address = self
            .addresses
            .lock()
            .ok()
            .and_then(|mut pool| {
                if pool.is_empty() {
                    None
                } else {
                    Some(pool.remove(0))
                }
            })
            .unwrap_or_else(|| format!("mock-address-{}", req.amount.value()));
        Ok(NewAddressResponse {
            address,
            label: req.label,
        })
    }

    async fn subscribe_payments(
        &self,
        from: Option<PaymentCursor>,
    ) -> Result<Pin<Box<dyn Stream<Item = ChainPaymentUpdate> + Send>>> {
        let updates = self.updates_after(from.as_ref());
        Ok(Box::pin(futures::stream::iter(updates)))
    }

    async fn send_coins(&self, req: SendCoinsRequest) -> Result<SendCoinsResponse> {
        if req.outputs.is_empty() {
            return Err(anyhow!("send_coins requires at least one output"));
        }
        // Reject sub-satoshi outputs, matching real backends.
        for o in &req.outputs {
            if o.amount.value() < 1000 {
                return Err(anyhow!(
                    "output to {} is below the 1 sat minimum",
                    o.address
                ));
            }
        }
        let total_msat = req.total_msat();
        let mut sends = self
            .sends
            .lock()
            .map_err(|_| anyhow!("sends mutex poisoned"))?;
        // Deterministic fake txid derived from call order.
        let txid = format!("mock-txid-{}", sends.len());
        sends.push(req);
        Ok(SendCoinsResponse {
            txid,
            total_amount: CurrencyAmount::millisats(total_msat),
            fee: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::currency::CurrencyAmount;
    use futures::StreamExt;

    fn sample_update(txid: &str) -> ChainPaymentUpdate {
        ChainPaymentUpdate::Confirmed {
            address: "bc1qaddr".to_string(),
            txid: txid.to_string(),
            vout: 0,
            amount_msat: 1000,
            confirmations: 1,
            label: None,
        }
    }

    fn send_output(address: &str, msat: u64) -> crate::onchain::SendOutput {
        crate::onchain::SendOutput {
            address: address.to_string(),
            amount: CurrencyAmount::millisats(msat),
        }
    }

    #[tokio::test]
    async fn test_send_coins_records_and_returns_txid() {
        let provider = MockOnChainProvider::default();
        let rsp = provider
            .send_coins(SendCoinsRequest {
                outputs: vec![send_output("bc1qpay", 2_000_000)],
                sat_per_vbyte: Some(3),
                target_conf: None,
                label: Some("payout".to_string()),
            })
            .await
            .unwrap();
        assert_eq!(rsp.txid, "mock-txid-0");
        assert_eq!(rsp.total_amount.value(), 2_000_000);
        // Second send gets a distinct txid and is recorded in order.
        let rsp2 = provider
            .send_coins(SendCoinsRequest {
                outputs: vec![send_output("bc1qpay2", 1_000_000)],
                sat_per_vbyte: None,
                target_conf: Some(6),
                label: None,
            })
            .await
            .unwrap();
        assert_eq!(rsp2.txid, "mock-txid-1");
        let sends = provider.sends();
        assert_eq!(sends.len(), 2);
        assert_eq!(sends[0].outputs[0].address, "bc1qpay");
        assert_eq!(sends[1].outputs[0].address, "bc1qpay2");
    }

    #[tokio::test]
    async fn test_send_coins_rejects_empty_and_sub_sat() {
        let provider = MockOnChainProvider::default();
        assert!(
            provider
                .send_coins(SendCoinsRequest {
                    outputs: vec![],
                    sat_per_vbyte: None,
                    target_conf: None,
                    label: None,
                })
                .await
                .is_err()
        );
        assert!(
            provider
                .send_coins(SendCoinsRequest {
                    outputs: vec![send_output("bc1qtiny", 999)],
                    sat_per_vbyte: None,
                    target_conf: None,
                    label: None,
                })
                .await
                .is_err()
        );
        assert!(provider.sends().is_empty());
    }

    #[test]
    fn test_default() {
        let provider = MockOnChainProvider::default();
        assert!(provider.updates_after(None).is_empty());
    }

    #[test]
    fn test_updates_after_no_cursor() {
        let provider = MockOnChainProvider::new(
            vec![],
            vec![(1, sample_update("a")), (2, sample_update("b"))],
        );
        assert_eq!(provider.updates_after(None).len(), 2);
    }

    #[test]
    fn test_updates_after_with_cursor_resumes() {
        let provider = MockOnChainProvider::new(
            vec![],
            vec![
                (1, sample_update("a")),
                (2, sample_update("b")),
                (3, sample_update("c")),
            ],
        );
        let cursor = PaymentCursor::from_height(2);
        let remaining = provider.updates_after(Some(&cursor));
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0], sample_update("c"));
    }

    #[tokio::test]
    async fn test_new_address_from_pool() {
        let provider = MockOnChainProvider::new(
            vec!["bc1qpool1".to_string(), "bc1qpool2".to_string()],
            vec![],
        );
        let rsp = provider
            .new_address(NewAddressRequest {
                amount: CurrencyAmount::millisats(1000),
                memo: None,
                label: Some("order-1".to_string()),
            })
            .await
            .unwrap();
        assert_eq!(rsp.address, "bc1qpool1");
        assert_eq!(rsp.label, Some("order-1".to_string()));
    }

    #[tokio::test]
    async fn test_new_address_fallback_generates() {
        let provider = MockOnChainProvider::new(vec![], vec![]);
        let rsp = provider
            .new_address(NewAddressRequest {
                amount: CurrencyAmount::millisats(4200),
                memo: None,
                label: None,
            })
            .await
            .unwrap();
        assert_eq!(rsp.address, "mock-address-4200");
        assert_eq!(rsp.label, None);
    }

    #[tokio::test]
    async fn test_subscribe_payments_streams_updates() {
        let provider = MockOnChainProvider::new(
            vec![],
            vec![(1, sample_update("a")), (2, sample_update("b"))],
        );
        let stream = provider
            .subscribe_payments(Some(PaymentCursor::from_height(1)))
            .await
            .unwrap();
        let collected: Vec<_> = stream.collect().await;
        assert_eq!(collected, vec![sample_update("b")]);
    }
}
