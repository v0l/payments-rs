//! LND (Lightning Network Daemon) on-chain payment integration.
//!
//! This backend derives fresh receive addresses from the LND wallet and watches
//! for incoming on-chain deposits via the `SubscribeTransactions` streaming RPC.
//!
//! The async methods that require a running LND node are excluded from coverage;
//! the pure transaction-parsing helpers are unit tested.

use crate::currency::CurrencyAmount;
use crate::onchain::{
    ChainPaymentUpdate, NewAddressRequest, NewAddressResponse, OnChainProvider, PaymentCursor,
    SendCoinsRequest, SendCoinsResponse, msat_to_sats, sats_to_msat,
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use fedimint_tonic_lnd::lnrpc::{
    GetTransactionsRequest, NewAddressRequest as LndNewAddressRequest, SendManyRequest, Transaction,
};
use fedimint_tonic_lnd::{Client, connect};
use futures::{Stream, StreamExt};
use std::collections::HashMap;
use std::path::Path;
use std::pin::Pin;

/// The type of on-chain address to derive from the LND wallet.
///
/// Mirrors LND's `AddressType` enum for the address families relevant to
/// receiving payments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LndAddressType {
    /// Pay-to-witness-public-key-hash (`p2wkh`, bech32).
    WitnessPubkeyHash,
    /// Nested pay-to-witness-public-key-hash (`np2wkh`, base58).
    NestedPubkeyHash,
    /// Pay-to-taproot (`p2tr`, bech32m).
    TaprootPubkey,
}

impl LndAddressType {
    /// Map to the LND `AddressType` protobuf enum value.
    pub fn as_lnd_type(&self) -> i32 {
        match self {
            // WITNESS_PUBKEY_HASH = 0
            LndAddressType::WitnessPubkeyHash => 0,
            // NESTED_PUBKEY_HASH = 1
            LndAddressType::NestedPubkeyHash => 1,
            // TAPROOT_PUBKEY = 4
            LndAddressType::TaprootPubkey => 4,
        }
    }
}

/// Configuration for the LND on-chain provider.
#[derive(Debug, Clone)]
pub struct LndOnChainConfig {
    /// The type of receive address to derive.
    pub address_type: LndAddressType,
    /// Optional wallet account name (empty/`None` uses the default account).
    pub account: Option<String>,
    /// Number of confirmations required before an update is reported as
    /// [`ChainPaymentUpdate::Confirmed`].
    pub min_confirmations: u32,
}

/// LND on-chain payment provider.
///
/// Derives receive addresses and streams every on-chain deposit event for the
/// LND wallet. `SubscribeTransactions` only ever reports the wallet's own
/// transactions, so no address bookkeeping is required here: the caller
/// correlates incoming deposits back to orders by address.
#[derive(Clone)]
pub struct LndOnChainProvider {
    client: Client,
    config: LndOnChainConfig,
}

impl LndOnChainProvider {
    /// Create a new LND on-chain provider.
    ///
    /// # Arguments
    ///
    /// * `url` - The gRPC URL of the LND node (e.g., "https://localhost:10009")
    /// * `cert` - Path to the TLS certificate file (tls.cert)
    /// * `macaroon` - Path to the macaroon file (admin.macaroon)
    /// * `config` - On-chain provider configuration
    ///
    /// # Note
    ///
    /// You must call [`setup_crypto_provider`](crate::lightning::setup_crypto_provider)
    /// before creating connections.
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub async fn new(
        url: &str,
        cert: &Path,
        macaroon: &Path,
        config: LndOnChainConfig,
    ) -> Result<Self> {
        let cert = cert
            .to_str()
            .ok_or_else(|| anyhow!("cert path is not valid UTF-8"))?;
        let macaroon = macaroon
            .to_str()
            .ok_or_else(|| anyhow!("macaroon path is not valid UTF-8"))?;
        let client = connect(url.to_string(), cert, macaroon)
            .await
            .map_err(|e| anyhow!("Failed to connect to LND: {}", e))?;

        Ok(Self { client, config })
    }
}

/// Build the `GetTransactionsRequest` used to (re)start a subscription.
///
/// `end_height` is set to `-1` so unconfirmed transactions up to the chain tip
/// are also streamed. The cursor's block height becomes the inclusive
/// `start_height`, giving resumable, at-least-once delivery.
fn subscribe_request(
    from: Option<&PaymentCursor>,
    account: Option<&str>,
) -> GetTransactionsRequest {
    GetTransactionsRequest {
        start_height: from.map(|c| c.block_height as i32).unwrap_or(0),
        end_height: -1,
        account: account.unwrap_or_default().to_string(),
        index_offset: 0,
        max_transactions: 0,
    }
}

/// Build the LND `SendManyRequest` for a [`SendCoinsRequest`].
///
/// Amounts are converted from milli-satoshis to whole satoshis (LND works in
/// sats); an output that rounds to 0 sats is rejected. Outputs are collapsed
/// into an address→sats map, summing duplicate addresses. Returns the request
/// and the exact total in milli-satoshis actually sent (post-truncation).
fn build_send_many_request(req: &SendCoinsRequest) -> Result<(SendManyRequest, u64)> {
    if req.outputs.is_empty() {
        return Err(anyhow!("send_coins requires at least one output"));
    }
    let mut addr_to_amount: HashMap<String, i64> = HashMap::new();
    let mut total_msat: u64 = 0;
    for o in &req.outputs {
        let sats = msat_to_sats(o.amount.value());
        if sats == 0 {
            return Err(anyhow!(
                "output to {} is below the 1 sat minimum",
                o.address
            ));
        }
        let entry = addr_to_amount.entry(o.address.clone()).or_insert(0);
        *entry = entry.saturating_add(sats as i64);
        total_msat = total_msat.saturating_add(sats_to_msat(sats));
    }
    let request = SendManyRequest {
        addr_to_amount,
        target_conf: req.target_conf.map(|c| c as i32).unwrap_or(0),
        sat_per_vbyte: req.sat_per_vbyte.unwrap_or(0),
        label: req.label.clone().unwrap_or_default(),
        ..Default::default()
    };
    Ok((request, total_msat))
}

/// Convert an LND [`Transaction`] into zero or more [`ChainPaymentUpdate`]s.
///
/// Only outputs controlled by our wallet with a positive value are reported.
/// Whether an update is [`ChainPaymentUpdate::Detected`] or
/// [`ChainPaymentUpdate::Confirmed`] depends on `min_confirmations`. The
/// `label` is always `None`: the caller correlates the reported `address`
/// back to an order.
fn transaction_to_updates(tx: &Transaction, min_confirmations: u32) -> Vec<ChainPaymentUpdate> {
    let confirmations = tx.num_confirmations.max(0) as u32;
    tx.output_details
        .iter()
        .filter(|o| o.is_our_address && o.amount > 0)
        .map(|o| {
            let amount_msat = sats_to_msat(o.amount as u64);
            if confirmations >= min_confirmations {
                ChainPaymentUpdate::Confirmed {
                    address: o.address.clone(),
                    txid: tx.tx_hash.clone(),
                    vout: o.output_index.max(0) as u32,
                    amount_msat,
                    confirmations,
                    label: None,
                }
            } else {
                ChainPaymentUpdate::Detected {
                    address: o.address.clone(),
                    txid: tx.tx_hash.clone(),
                    vout: o.output_index.max(0) as u32,
                    amount_msat,
                    confirmations,
                    label: None,
                }
            }
        })
        .collect()
}

#[async_trait]
impl OnChainProvider for LndOnChainProvider {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn new_address(&self, req: NewAddressRequest) -> Result<NewAddressResponse> {
        let mut client = self.client.clone();
        let res = client
            .lightning()
            .new_address(LndNewAddressRequest {
                r#type: self.config.address_type.as_lnd_type(),
                account: self.config.account.clone().unwrap_or_default(),
            })
            .await?;

        let address = res.into_inner().address;
        Ok(NewAddressResponse {
            address,
            label: req.label,
        })
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn subscribe_payments(
        &self,
        from: Option<PaymentCursor>,
    ) -> Result<Pin<Box<dyn Stream<Item = ChainPaymentUpdate> + Send>>> {
        let min_conf = self.config.min_confirmations;
        let account = self.config.account.clone();
        let req = subscribe_request(from.as_ref(), account.as_deref());

        // 1. Historical/confirmed transactions from the cursor via the unary
        //    GetTransactions RPC. `SubscribeTransactions` is live-only and does
        //    not replay history, so this is required for resumability.
        let mut hist_client = self.client.clone();
        let history = hist_client
            .lightning()
            .get_transactions(req.clone())
            .await?
            .into_inner();
        let mut historical = Vec::new();
        for tx in &history.transactions {
            historical.extend(transaction_to_updates(tx, min_conf));
        }

        // 2. Live transactions via SubscribeTransactions. LND defers the gRPC
        //    response headers until it has something to stream, so we establish
        //    the stream lazily (inside `stream::once`) to avoid blocking here.
        let mut live_client = self.client.clone();
        let live = futures::stream::once(async move {
            match live_client.lightning().subscribe_transactions(req).await {
                Ok(resp) => resp
                    .into_inner()
                    .flat_map(move |item| match item {
                        Ok(tx) => futures::stream::iter(transaction_to_updates(&tx, min_conf)),
                        Err(e) => {
                            futures::stream::iter(vec![ChainPaymentUpdate::Error(e.to_string())])
                        }
                    })
                    .boxed(),
                Err(e) => {
                    futures::stream::iter(vec![ChainPaymentUpdate::Error(e.to_string())]).boxed()
                }
            }
        })
        .flatten();

        Ok(Box::pin(futures::stream::iter(historical).chain(live)))
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn send_coins(&self, req: SendCoinsRequest) -> Result<SendCoinsResponse> {
        let (request, total_msat) = build_send_many_request(&req)?;
        let mut client = self.client.clone();
        let resp = client
            .lightning()
            .send_many(request)
            .await
            .map_err(|e| anyhow!("LND SendMany failed: {}", e))?
            .into_inner();
        // SendMany only returns the txid, not the per-output vouts. Look the tx
        // up in the wallet to return its raw bytes so callers can decode which
        // output pays each address. Best-effort: `None` if it can't be found.
        let raw_tx = self.lookup_raw_tx(&resp.txid).await.ok().flatten();
        Ok(SendCoinsResponse {
            txid: resp.txid,
            total_amount: CurrencyAmount::millisats(total_msat),
            // lnrpc SendMany does not report the fee. Fee is paid on top of the
            // outputs.
            fee: None,
            raw_tx,
        })
    }
}

impl LndOnChainProvider {
    /// Fetch the hex-encoded raw transaction for `txid` from the wallet's
    /// transaction history. Returns `None` when the transaction isn't found or
    /// carries no raw bytes.
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn lookup_raw_tx(&self, txid: &str) -> Result<Option<String>> {
        let mut client = self.client.clone();
        let txns = client
            .lightning()
            .get_transactions(GetTransactionsRequest {
                start_height: 0,
                end_height: -1,
                account: self.config.account.clone().unwrap_or_default(),
                index_offset: 0,
                max_transactions: 0,
            })
            .await?
            .into_inner();
        Ok(txns
            .transactions
            .into_iter()
            .find(|t| t.tx_hash == txid)
            .map(|t| t.raw_tx_hex)
            .filter(|s| !s.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::onchain::SendOutput;
    use fedimint_tonic_lnd::lnrpc::OutputDetail;

    fn output(address: &str, amount: i64, is_ours: bool) -> OutputDetail {
        OutputDetail {
            output_type: 0,
            address: address.to_string(),
            pk_script: String::new(),
            output_index: 0,
            amount,
            is_our_address: is_ours,
        }
    }

    #[allow(deprecated)]
    fn tx(hash: &str, confs: i32, outputs: Vec<OutputDetail>) -> Transaction {
        Transaction {
            tx_hash: hash.to_string(),
            amount: outputs.iter().map(|o| o.amount).sum(),
            num_confirmations: confs,
            block_hash: String::new(),
            block_height: 0,
            time_stamp: 0,
            total_fees: 0,
            dest_addresses: vec![],
            output_details: outputs,
            raw_tx_hex: String::new(),
            label: String::new(),
            previous_outpoints: vec![],
        }
    }

    #[test]
    fn test_address_type_as_lnd_type() {
        assert_eq!(LndAddressType::WitnessPubkeyHash.as_lnd_type(), 0);
        assert_eq!(LndAddressType::NestedPubkeyHash.as_lnd_type(), 1);
        assert_eq!(LndAddressType::TaprootPubkey.as_lnd_type(), 4);
    }

    #[test]
    fn test_address_type_clone_debug_eq() {
        let a = LndAddressType::TaprootPubkey;
        assert_eq!(a, a);
        assert_ne!(a, LndAddressType::WitnessPubkeyHash);
        assert!(format!("{:?}", a).contains("Taproot"));
    }

    #[test]
    fn test_config_clone_debug() {
        let cfg = LndOnChainConfig {
            address_type: LndAddressType::WitnessPubkeyHash,
            account: Some("orders".to_string()),
            min_confirmations: 2,
        };
        let cloned = cfg.clone();
        assert_eq!(cloned.min_confirmations, 2);
        assert_eq!(cloned.account, Some("orders".to_string()));
        assert!(format!("{:?}", cfg).contains("orders"));
    }

    #[test]
    fn test_subscribe_request_no_cursor() {
        let req = subscribe_request(None, None);
        assert_eq!(req.start_height, 0);
        assert_eq!(req.end_height, -1);
        assert_eq!(req.account, "");
    }

    #[test]
    fn test_subscribe_request_with_cursor_and_account() {
        let cursor = PaymentCursor::from_height(800_000);
        let req = subscribe_request(Some(&cursor), Some("orders"));
        assert_eq!(req.start_height, 800_000);
        assert_eq!(req.end_height, -1);
        assert_eq!(req.account, "orders");
    }

    #[test]
    fn test_transaction_to_updates_detected() {
        let t = tx("txid1", 0, vec![output("bc1qaddr", 100_000, true)]);
        let updates = transaction_to_updates(&t, 1);
        assert_eq!(updates.len(), 1);
        assert_eq!(
            updates[0],
            ChainPaymentUpdate::Detected {
                address: "bc1qaddr".to_string(),
                txid: "txid1".to_string(),
                vout: 0,
                amount_msat: 100_000_000,
                confirmations: 0,
                label: None,
            }
        );
    }

    #[test]
    fn test_transaction_to_updates_vout_per_output() {
        // One tx paying two of our addresses: each update carries the
        // originating output index so callers can key deposits by txid:vout.
        let mut o1 = output("bc1qaddr1", 100_000, true);
        o1.output_index = 0;
        let mut o2 = output("bc1qaddr2", 200_000, true);
        o2.output_index = 2;
        let t = tx("txid3", 3, vec![o1, o2]);
        let updates = transaction_to_updates(&t, 1);
        assert_eq!(updates.len(), 2);
        let vouts: Vec<u32> = updates
            .iter()
            .map(|u| match u {
                ChainPaymentUpdate::Confirmed { vout, .. } => *vout,
                other => panic!("expected Confirmed, got {:?}", other),
            })
            .collect();
        assert_eq!(vouts, vec![0, 2]);
    }

    #[test]
    fn test_transaction_to_updates_confirmed() {
        let t = tx("txid2", 3, vec![output("bc1qaddr", 50_000, true)]);
        let updates = transaction_to_updates(&t, 1);
        assert_eq!(
            updates[0],
            ChainPaymentUpdate::Confirmed {
                address: "bc1qaddr".to_string(),
                txid: "txid2".to_string(),
                vout: 0,
                amount_msat: 50_000_000,
                confirmations: 3,
                label: None,
            }
        );
    }

    #[test]
    fn test_transaction_to_updates_filters_non_ours_and_zero() {
        let t = tx(
            "txid3",
            2,
            vec![
                output("bc1qtheirs", 100_000, false),
                output("bc1qchange", 0, true),
                output("bc1qmine", 25_000, true),
            ],
        );
        let updates = transaction_to_updates(&t, 1);
        assert_eq!(updates.len(), 1);
        if let ChainPaymentUpdate::Confirmed { address, .. } = &updates[0] {
            assert_eq!(address, "bc1qmine");
        } else {
            panic!("Expected Confirmed variant");
        }
    }

    #[test]
    fn test_transaction_to_updates_negative_confirmations() {
        let t = tx("txid4", -1, vec![output("bc1qaddr", 10_000, true)]);
        let updates = transaction_to_updates(&t, 1);
        // -1 confirmations clamps to 0 -> Detected
        assert!(matches!(updates[0], ChainPaymentUpdate::Detected { .. }));
    }

    fn send_output(address: &str, msat: u64) -> SendOutput {
        SendOutput {
            address: address.to_string(),
            amount: CurrencyAmount::millisats(msat),
        }
    }

    #[test]
    fn test_build_send_many_request_basic() {
        let req = SendCoinsRequest {
            outputs: vec![
                send_output("bc1qone", 100_000),   // 100 sats
                send_output("bc1qtwo", 2_500_000), // 2500 sats
            ],
            sat_per_vbyte: Some(7),
            target_conf: Some(3),
            label: Some("referral payout".to_string()),
        };
        let (lnd, total_msat) = build_send_many_request(&req).unwrap();
        assert_eq!(total_msat, 2_600_000);
        assert_eq!(lnd.addr_to_amount.get("bc1qone"), Some(&100));
        assert_eq!(lnd.addr_to_amount.get("bc1qtwo"), Some(&2500));
        // sat_per_vbyte set -> target_conf still passed through
        assert_eq!(lnd.sat_per_vbyte, 7);
        assert_eq!(lnd.target_conf, 3);
        assert_eq!(lnd.label, "referral payout");
    }

    #[test]
    fn test_build_send_many_request_sums_duplicate_addresses() {
        let req = SendCoinsRequest {
            outputs: vec![
                send_output("bc1qdup", 1_000_000), // 1000 sats
                send_output("bc1qdup", 500_000),   // 500 sats
            ],
            sat_per_vbyte: None,
            target_conf: None,
            label: None,
        };
        let (lnd, total_msat) = build_send_many_request(&req).unwrap();
        assert_eq!(lnd.addr_to_amount.len(), 1);
        assert_eq!(lnd.addr_to_amount.get("bc1qdup"), Some(&1500));
        assert_eq!(total_msat, 1_500_000);
        // Defaults when unset.
        assert_eq!(lnd.sat_per_vbyte, 0);
        assert_eq!(lnd.target_conf, 0);
        assert_eq!(lnd.label, "");
    }

    #[test]
    fn test_build_send_many_request_rejects_empty() {
        let req = SendCoinsRequest {
            outputs: vec![],
            sat_per_vbyte: None,
            target_conf: None,
            label: None,
        };
        assert!(build_send_many_request(&req).is_err());
    }

    #[test]
    fn test_build_send_many_request_rejects_sub_sat_output() {
        // 500 msat rounds down to 0 sats -> rejected.
        let req = SendCoinsRequest {
            outputs: vec![send_output("bc1qtiny", 500)],
            sat_per_vbyte: None,
            target_conf: None,
            label: None,
        };
        let err = build_send_many_request(&req).unwrap_err().to_string();
        assert!(err.contains("bc1qtiny"), "unexpected error: {err}");
    }
}
