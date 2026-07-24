//! On-chain Bitcoin payment integrations.
//!
//! This module provides integrations with on-chain Bitcoin backends for
//! **receiving** and **sending** payments. Callers derive a fresh receive
//! address per order and are notified when funds arrive, including the number of
//! confirmations and the transaction id (`txid`), and can send payments to one
//! or more outputs in a single transaction via
//! [`OnChainProvider::send_coins`].
//!
//! # Supported Providers
//!
//! - **LND** (`method-lnd-onchain` feature) - Derives receive addresses and
//!   watches for on-chain deposits via the Lightning Network Daemon wallet.
//! - **Mock** (`mock` feature) - Scripted provider for downstream integration
//!   tests that need no real node.
//!
//! # Amounts
//!
//! On-chain amounts are surfaced in **milli-satoshis** so they round-trip cleanly
//! against [`CurrencyAmount::millisats`](crate::currency::CurrencyAmount::millisats).
//! Convert on-chain satoshis to milli-satoshis at the boundary using
//! [`sats_to_msat`].
//!
//! # Delivery guarantees
//!
//! [`OnChainProvider::subscribe_payments`] is **resumable** via a
//! [`PaymentCursor`]. The stream provides **at-least-once** delivery: a restarted
//! consumer resuming from a persisted cursor will not miss deposits, but may
//! observe the same `(txid, address)` more than once (for example a `Detected`
//! update followed by a later `Confirmed` update, or a replay after a restart).
//! Consumers must therefore de-duplicate on the `txid` (which is unique per
//! update) to achieve exactly-once accounting.
//!
//! # Example
//!
//! ```rust,ignore
//! use payments_rs::onchain::{LndOnChainConfig, LndAddressType, LndOnChainProvider, OnChainProvider, NewAddressRequest};
//! use payments_rs::currency::{Currency, CurrencyAmount};
//! use payments_rs::lightning::setup_crypto_provider;
//! use futures::StreamExt;
//! use std::path::Path;
//!
//! setup_crypto_provider();
//! let provider = LndOnChainProvider::new(
//!     "https://localhost:10009",
//!     Path::new("/path/to/tls.cert"),
//!     Path::new("/path/to/admin.macaroon"),
//!     LndOnChainConfig {
//!         address_type: LndAddressType::WitnessPubkeyHash,
//!         account: None,
//!         min_confirmations: 1,
//!     },
//! ).await?;
//!
//! let addr = provider.new_address(NewAddressRequest {
//!     amount: CurrencyAmount::millisats(100_000_000), // 100k sats
//!     memo: Some("Order #123".to_string()),
//!     label: Some("order-123".to_string()),
//! }).await?;
//! println!("Send payment to {}", addr.address);
//!
//! let mut updates = provider.subscribe_payments(None).await?;
//! while let Some(update) = updates.next().await {
//!     println!("chain update: {:?}", update);
//! }
//! ```

use crate::currency::CurrencyAmount;
use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;

#[cfg(feature = "method-lnd-onchain")]
mod lnd;
#[cfg(feature = "mock")]
mod mock;

#[cfg(feature = "method-lnd-onchain")]
pub use lnd::*;
#[cfg(feature = "mock")]
pub use mock::*;

/// Number of milli-satoshis in a single satoshi.
pub const MSAT_PER_SAT: u64 = 1000;

/// Convert on-chain satoshis to milli-satoshis.
///
/// Saturates at [`u64::MAX`] rather than overflowing.
pub fn sats_to_msat(sats: u64) -> u64 {
    sats.saturating_mul(MSAT_PER_SAT)
}

/// Convert milli-satoshis to whole satoshis, truncating any sub-satoshi remainder.
pub fn msat_to_sats(msat: u64) -> u64 {
    msat / MSAT_PER_SAT
}

/// Trait for on-chain Bitcoin payment backends.
///
/// Implement this trait to add support for additional on-chain providers. The
/// consumer can **receive** payments (derive addresses + stream deposits) and
/// **send** payments ([`send_coins`](OnChainProvider::send_coins)).
#[async_trait]
pub trait OnChainProvider: Send + Sync {
    /// Derive/allocate a fresh receive address for a new order.
    ///
    /// The `label` on the request is the caller's order reference and is
    /// echoed back on the [`NewAddressResponse`]. Callers should persist the
    /// returned `address` against their order: chain updates are keyed by
    /// `address` (and `txid`), and most backends (including LND) cannot attach
    /// an arbitrary order id to an on-chain output, so `label` is left
    /// `None` on [`ChainPaymentUpdate`]s.
    async fn new_address(&self, req: NewAddressRequest) -> Result<NewAddressResponse>;

    /// Stream chain events (payment detected / confirmed) for watched addresses.
    ///
    /// The stream is resumable from `from`, a [`PaymentCursor`] describing the
    /// last processed position (block height + hash). Pass `None` to start from
    /// the backend default (typically the current chain tip).
    async fn subscribe_payments(
        &self,
        from: Option<PaymentCursor>,
    ) -> Result<Pin<Box<dyn Stream<Item = ChainPaymentUpdate> + Send>>>;

    /// Send an on-chain payment to one or more destination outputs in a single
    /// transaction ("send-many").
    ///
    /// Each output receives its exact requested amount; the network fee is paid
    /// on top from the wallet (i.e. **absorbed by the sender**, not deducted
    /// from the outputs). The returned [`SendCoinsResponse`] carries the
    /// broadcast `txid` and, when the backend reports it, the fee paid.
    ///
    /// Amounts are milli-satoshis but Bitcoin has satoshi granularity, so any
    /// sub-satoshi remainder is rejected (an output that rounds to 0 sats is an
    /// error). Sending is **not** idempotent — a retry after an unknown outcome
    /// may broadcast a second transaction, so callers must reserve/de-duplicate
    /// their own payout records before calling.
    async fn send_coins(&self, req: SendCoinsRequest) -> Result<SendCoinsResponse>;
}

/// A single destination output of an on-chain [`OnChainProvider::send_coins`].
#[derive(Debug, Clone)]
pub struct SendOutput {
    /// Destination on-chain address.
    pub address: String,
    /// Amount to send to `address`, in milli-satoshis. Must be at least 1 whole
    /// satoshi (1000 msat); sub-satoshi remainders are not representable
    /// on-chain and are rejected.
    pub amount: CurrencyAmount,
}

/// Request to send an on-chain payment to one or more outputs in a single
/// transaction.
#[derive(Debug, Clone)]
pub struct SendCoinsRequest {
    /// Destination outputs. Must be non-empty. Multiple outputs to the same
    /// address are summed by the backend into a single output.
    pub outputs: Vec<SendOutput>,
    /// Manual fee rate in sat/vByte. `None` lets the backend pick a fee for the
    /// `target_conf` confirmation target.
    pub sat_per_vbyte: Option<u64>,
    /// Confirmation target in blocks, used when `sat_per_vbyte` is `None`.
    /// `None` uses the backend default.
    pub target_conf: Option<u32>,
    /// Optional transaction label/memo (backends that support it).
    pub label: Option<String>,
}

impl SendCoinsRequest {
    /// Total amount across all outputs (excludes the network fee), in
    /// milli-satoshis.
    pub fn total_msat(&self) -> u64 {
        self.outputs
            .iter()
            .fold(0u64, |acc, o| acc.saturating_add(o.amount.value()))
    }
}

/// Result of an on-chain [`OnChainProvider::send_coins`].
#[derive(Debug, Clone)]
pub struct SendCoinsResponse {
    /// Transaction id (hex) of the broadcast transaction.
    pub txid: String,
    /// Total amount sent across all outputs (excludes fees), in milli-satoshis.
    pub total_amount: CurrencyAmount,
    /// Total network fee paid, if reported by the backend. The fee is paid by
    /// the sender on top of the outputs.
    pub fee: Option<CurrencyAmount>,
    /// Raw broadcast transaction, hex-encoded, when the backend can supply it.
    ///
    /// Send-many transactions pay several outputs, and the backend does not
    /// report which output index (`vout`) pays each address. Callers that need
    /// per-output outpoints (`txid:vout`) can decode this raw transaction and
    /// match each destination's script. `None` when the backend cannot return
    /// the raw transaction.
    pub raw_tx: Option<String>,
}

/// Request to derive a new on-chain receive address.
#[derive(Debug, Clone)]
pub struct NewAddressRequest {
    /// Expected amount for the order (used for display/labelling only; the
    /// library never filters incoming transactions by exact amount).
    pub amount: CurrencyAmount,
    /// Optional human-readable description for the address (display only).
    pub memo: Option<String>,
    /// Caller's order reference, echoed back on the response. Backends that
    /// cannot label on-chain outputs (e.g. LND) leave it `None` on updates.
    pub label: Option<String>,
}

/// Response from deriving a new on-chain receive address.
#[derive(Debug, Clone)]
pub struct NewAddressResponse {
    /// The freshly derived receive address.
    pub address: String,
    /// Caller's order reference, echoed back from the request.
    pub label: Option<String>,
}

/// Cursor describing a position on the chain for resumable subscriptions.
///
/// Persist the most recently observed cursor and pass it back to
/// [`OnChainProvider::subscribe_payments`] after a restart to avoid missing or
/// double-counting deposits. De-duplicate observed updates by `txid`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentCursor {
    /// Block height of the last fully processed block.
    pub block_height: u64,
    /// Block hash of the last fully processed block, if known.
    ///
    /// Backends that support hash-based resume (e.g. Bitcoin Core's
    /// `listsinceblock`) use this to detect reorgs.
    pub block_hash: Option<String>,
}

impl PaymentCursor {
    /// Create a cursor from a block height.
    pub fn from_height(block_height: u64) -> Self {
        Self {
            block_height,
            block_hash: None,
        }
    }

    /// Create a cursor from a block height and hash.
    pub fn new(block_height: u64, block_hash: Option<String>) -> Self {
        Self {
            block_height,
            block_hash,
        }
    }
}

/// Updates for on-chain payment status changes.
///
/// Every variant that represents a payment carries the real `txid` and the
/// **actual** `amount_msat` received. Partial, late and over-payments are all
/// reported as-is; the library never filters by exact amount and leaves any
/// pro-rating to the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainPaymentUpdate {
    /// A transaction paying a watched address was seen but has fewer than the
    /// configured minimum confirmations.
    Detected {
        /// The receive address that was paid.
        address: String,
        /// Transaction id (hex).
        txid: String,
        /// Index of the output paying `address` within the transaction.
        vout: u32,
        /// Actual amount received in milli-satoshis.
        amount_msat: u64,
        /// Number of confirmations observed so far.
        confirmations: u32,
        /// Caller's order reference (`label`), if the backend can attach one;
        /// `None` otherwise (e.g. LND). Correlate by `address` + `txid` instead.
        label: Option<String>,
    },
    /// A transaction paying a watched address reached the configured minimum
    /// number of confirmations.
    Confirmed {
        /// The receive address that was paid.
        address: String,
        /// Transaction id (hex).
        txid: String,
        /// Index of the output paying `address` within the transaction.
        vout: u32,
        /// Actual amount received in milli-satoshis.
        amount_msat: u64,
        /// Number of confirmations observed.
        confirmations: u32,
        /// Caller's order reference (`label`), if the backend can attach one;
        /// `None` otherwise (e.g. LND). Correlate by `address` + `txid` instead.
        label: Option<String>,
    },
    /// An error occurred while watching the chain.
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::currency::Currency;

    #[test]
    fn test_sats_to_msat() {
        assert_eq!(sats_to_msat(0), 0);
        assert_eq!(sats_to_msat(1), 1000);
        assert_eq!(sats_to_msat(100_000_000), 100_000_000_000);
    }

    #[test]
    fn test_sats_to_msat_saturates() {
        assert_eq!(sats_to_msat(u64::MAX), u64::MAX);
    }

    #[test]
    fn test_msat_to_sats() {
        assert_eq!(msat_to_sats(0), 0);
        assert_eq!(msat_to_sats(999), 0);
        assert_eq!(msat_to_sats(1000), 1);
        assert_eq!(msat_to_sats(1500), 1);
        assert_eq!(msat_to_sats(100_000_000_000), 100_000_000);
    }

    #[test]
    fn test_sats_msat_round_trip() {
        let sats = 12_345u64;
        assert_eq!(msat_to_sats(sats_to_msat(sats)), sats);
    }

    #[test]
    fn test_new_address_request_clone_debug() {
        let req = NewAddressRequest {
            amount: CurrencyAmount::millisats(100_000_000),
            memo: Some("Order #1".to_string()),
            label: Some("order-1".to_string()),
        };
        let cloned = req.clone();
        assert_eq!(cloned.amount.currency(), Currency::BTC);
        assert_eq!(cloned.amount.value(), 100_000_000);
        assert_eq!(cloned.memo, Some("Order #1".to_string()));
        assert_eq!(cloned.label, Some("order-1".to_string()));
        assert!(format!("{:?}", req).contains("order-1"));
    }

    #[test]
    fn test_new_address_response_clone_debug() {
        let rsp = NewAddressResponse {
            address: "bc1qexampleaddr".to_string(),
            label: Some("order-1".to_string()),
        };
        let cloned = rsp.clone();
        assert_eq!(cloned.address, "bc1qexampleaddr");
        assert_eq!(cloned.label, Some("order-1".to_string()));
        assert!(format!("{:?}", rsp).contains("bc1qexampleaddr"));
    }

    #[test]
    fn test_payment_cursor_from_height() {
        let cursor = PaymentCursor::from_height(800_000);
        assert_eq!(cursor.block_height, 800_000);
        assert_eq!(cursor.block_hash, None);
    }

    #[test]
    fn test_payment_cursor_new() {
        let cursor = PaymentCursor::new(800_001, Some("0000abcd".to_string()));
        assert_eq!(cursor.block_height, 800_001);
        assert_eq!(cursor.block_hash, Some("0000abcd".to_string()));
    }

    #[test]
    fn test_payment_cursor_clone_eq() {
        let cursor = PaymentCursor::new(1, Some("hash".to_string()));
        let cloned = cursor.clone();
        assert_eq!(cursor, cloned);
        assert_ne!(cursor, PaymentCursor::from_height(1));
    }

    #[test]
    fn test_chain_payment_update_detected() {
        let update = ChainPaymentUpdate::Detected {
            address: "bc1qaddr".to_string(),
            txid: "deadbeef".to_string(),
            vout: 0,
            amount_msat: 100_000_000,
            confirmations: 0,
            label: Some("order-1".to_string()),
        };
        let cloned = update.clone();
        assert_eq!(update, cloned);
        if let ChainPaymentUpdate::Detected {
            address,
            txid,
            vout,
            amount_msat,
            confirmations,
            label,
        } = cloned
        {
            assert_eq!(address, "bc1qaddr");
            assert_eq!(txid, "deadbeef");
            assert_eq!(vout, 0);
            assert_eq!(amount_msat, 100_000_000);
            assert_eq!(confirmations, 0);
            assert_eq!(label, Some("order-1".to_string()));
        } else {
            panic!("Expected Detected variant");
        }
    }

    #[test]
    fn test_chain_payment_update_confirmed() {
        let update = ChainPaymentUpdate::Confirmed {
            address: "bc1qaddr".to_string(),
            txid: "cafebabe".to_string(),
            vout: 1,
            amount_msat: 50_000_000,
            confirmations: 3,
            label: None,
        };
        assert!(format!("{:?}", update).contains("cafebabe"));
        if let ChainPaymentUpdate::Confirmed {
            txid,
            amount_msat,
            confirmations,
            label,
            ..
        } = update
        {
            assert_eq!(txid, "cafebabe");
            assert_eq!(amount_msat, 50_000_000);
            assert_eq!(confirmations, 3);
            assert_eq!(label, None);
        } else {
            panic!("Expected Confirmed variant");
        }
    }

    #[test]
    fn test_send_coins_request_total_msat() {
        let req = SendCoinsRequest {
            outputs: vec![
                SendOutput {
                    address: "bc1qone".to_string(),
                    amount: CurrencyAmount::millisats(100_000),
                },
                SendOutput {
                    address: "bc1qtwo".to_string(),
                    amount: CurrencyAmount::millisats(250_000),
                },
            ],
            sat_per_vbyte: Some(5),
            target_conf: None,
            label: Some("payout batch".to_string()),
        };
        assert_eq!(req.total_msat(), 350_000);
        // Clone/Debug coverage
        let cloned = req.clone();
        assert_eq!(cloned.outputs.len(), 2);
        assert!(format!("{:?}", req).contains("payout batch"));
    }

    #[test]
    fn test_send_coins_response_clone_debug() {
        let rsp = SendCoinsResponse {
            txid: "abc123".to_string(),
            total_amount: CurrencyAmount::millisats(350_000),
            fee: Some(CurrencyAmount::millisats(1_000)),
            raw_tx: Some("0200000000".to_string()),
        };
        let cloned = rsp.clone();
        assert_eq!(cloned.txid, "abc123");
        assert_eq!(cloned.total_amount.value(), 350_000);
        assert_eq!(cloned.fee.map(|f| f.value()), Some(1_000));
        assert_eq!(cloned.raw_tx.as_deref(), Some("0200000000"));
        assert!(format!("{:?}", rsp).contains("abc123"));
    }

    #[test]
    fn test_chain_payment_update_error() {
        let update = ChainPaymentUpdate::Error("rpc down".to_string());
        let cloned = update.clone();
        assert_eq!(update, cloned);
        if let ChainPaymentUpdate::Error(msg) = cloned {
            assert_eq!(msg, "rpc down");
        } else {
            panic!("Expected Error variant");
        }
    }
}
