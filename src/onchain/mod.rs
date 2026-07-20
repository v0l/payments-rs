//! On-chain Bitcoin payment integrations.
//!
//! This module provides integrations with on-chain Bitcoin backends for
//! **receiving** payments. Callers derive a fresh receive address per order
//! and are notified when funds arrive, including the number of confirmations
//! and the transaction id (`txid`).
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
/// Implement this trait to add support for additional on-chain providers.
/// The consumer only needs to **receive** payments.
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
