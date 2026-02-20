//! Lightning Network payment integrations.
//!
//! This module provides integrations with Lightning Network payment providers
//! for Bitcoin payments.
//!
//! # Supported Providers
//!
//! - **LND** (`method-lnd` feature) - Direct connection to Lightning Network Daemon
//! - **Bitvora** (`method-bitvora` feature) - Custodial Lightning payment API
//!
//! # Example
//!
//! ```rust,ignore
//! use payments_rs::lightning::{LndNode, LightningNode, AddInvoiceRequest};
//!
//! let lnd = LndNode::new("https://localhost:10009", cert_path, macaroon_path).await?;
//!
//! let invoice = lnd.add_invoice(AddInvoiceRequest {
//!     amount: 1000, // 1000 milli-satoshis
//!     memo: Some("Coffee".to_string()),
//!     expire: Some(3600),
//! }).await?;
//!
//! println!("Payment request: {}", invoice.pr());
//! ```

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use futures::Stream;
use hex::ToHex;
use lightning_invoice::Bolt11Invoice;
use std::pin::Pin;

#[cfg(feature = "method-bitvora")]
mod bitvora;
#[cfg(feature = "method-lnd")]
mod lnd;

#[cfg(feature = "method-bitvora")]
pub use bitvora::*;
#[cfg(feature = "method-lnd")]
pub use lnd::*;

/// Trait for Lightning Network node implementations.
///
/// Implement this trait to add support for additional Lightning providers.
/// Both LND and Bitvora implement this trait.
#[async_trait]
pub trait LightningNode: Send + Sync {
    /// Create a new invoice for receiving payments.
    async fn add_invoice(&self, req: AddInvoiceRequest) -> Result<AddInvoiceResponse>;

    /// Cancel an existing invoice by payment hash.
    async fn cancel_invoice(&self, id: &[u8]) -> Result<()>;

    /// Pay a Lightning invoice.
    async fn pay_invoice(&self, req: PayInvoiceRequest) -> Result<PayInvoiceResponse>;

    /// Subscribe to invoice updates (created, settled, canceled).
    ///
    /// # Arguments
    ///
    /// * `from_payment_hash` - Optional payment hash to resume from
    async fn subscribe_invoices(
        &self,
        from_payment_hash: Option<Vec<u8>>,
    ) -> Result<Pin<Box<dyn Stream<Item = InvoiceUpdate> + Send>>>;
}

/// Request to create a new Lightning invoice.
#[derive(Debug, Clone)]
pub struct AddInvoiceRequest {
    /// Amount in milli-satoshis
    pub amount: u64,
    /// Optional memo/description for the invoice
    pub memo: Option<String>,
    /// Expiration time in seconds (default: 3600)
    pub expire: Option<u32>,
}

/// Response from creating a Lightning invoice.
#[derive(Debug, Clone)]
pub struct AddInvoiceResponse {
    /// External ID from the provider (if applicable)
    pub external_id: Option<String>,
    /// The parsed BOLT11 invoice
    pub parsed_invoice: Bolt11Invoice,
}

impl AddInvoiceResponse {
    /// Get the payment request string (BOLT11 invoice).
    pub fn pr(&self) -> String {
        self.parsed_invoice.to_string()
    }

    /// Get the payment hash as a hex string.
    pub fn payment_hash(&self) -> String {
        self.parsed_invoice.payment_hash().encode_hex()
    }

    /// Create an AddInvoiceResponse from a payment request string.
    pub fn from_invoice(pr: &str, external_id: Option<String>) -> Result<AddInvoiceResponse> {
        let parsed = pr
            .parse()
            .map_err(|e| anyhow!("Failed to parse invoice {}", e))?;
        Ok(Self {
            parsed_invoice: parsed,
            external_id,
        })
    }
}

/// Request to pay a Lightning invoice.
#[derive(Debug, Clone)]
pub struct PayInvoiceRequest {
    /// The BOLT11 invoice string to pay
    pub invoice: String,
    /// Timeout in seconds for the payment attempt
    pub timeout_seconds: Option<u32>,
}

/// Response from paying a Lightning invoice.
#[derive(Debug, Clone)]
pub struct PayInvoiceResponse {
    /// Payment hash as hex string
    pub payment_hash: String,
    /// Payment preimage as hex string (proof of payment)
    pub payment_preimage: Option<String>,
    /// Amount paid in milli-satoshis
    pub amount_msat: u64,
    /// Routing fee paid in milli-satoshis
    pub fee_msat: u64,
}

/// Updates for invoice status changes.
#[derive(Debug, Clone)]
pub enum InvoiceUpdate {
    /// Unknown or unsupported update type
    Unknown {
        /// Payment hash as hex string
        payment_hash: String,
    },
    /// An error occurred
    Error(String),
    /// Invoice was created
    Created {
        /// Payment hash as hex string
        payment_hash: String,
        /// BOLT11 payment request
        payment_request: String,
    },
    /// Invoice was canceled
    Canceled {
        /// Payment hash as hex string
        payment_hash: String,
    },
    /// Invoice was paid/settled
    Settled {
        /// Payment hash as hex string
        payment_hash: String,
        /// Payment preimage (proof of payment)
        preimage: Option<String>,
        /// External ID from the provider
        external_id: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_invoice_request_clone() {
        let req = AddInvoiceRequest {
            amount: 1000,
            memo: Some("Test payment".to_string()),
            expire: Some(3600),
        };
        let cloned = req.clone();
        assert_eq!(cloned.amount, 1000);
        assert_eq!(cloned.memo, Some("Test payment".to_string()));
        assert_eq!(cloned.expire, Some(3600));
    }

    #[test]
    fn test_add_invoice_request_debug() {
        let req = AddInvoiceRequest {
            amount: 1000,
            memo: None,
            expire: None,
        };
        let debug_str = format!("{:?}", req);
        assert!(debug_str.contains("1000"));
    }

    #[test]
    fn test_add_invoice_response_from_invoice_invalid() {
        let result = AddInvoiceResponse::from_invoice("invalid_invoice", None);
        assert!(result.is_err());
    }

    #[test]
    fn test_pay_invoice_request_clone() {
        let req = PayInvoiceRequest {
            invoice: "lnbc...".to_string(),
            timeout_seconds: Some(60),
        };
        let cloned = req.clone();
        assert_eq!(cloned.invoice, "lnbc...");
        assert_eq!(cloned.timeout_seconds, Some(60));
    }

    #[test]
    fn test_pay_invoice_response_clone() {
        let resp = PayInvoiceResponse {
            payment_hash: "abc123".to_string(),
            payment_preimage: Some("def456".to_string()),
            amount_msat: 1000,
            fee_msat: 10,
        };
        let cloned = resp.clone();
        assert_eq!(cloned.payment_hash, "abc123");
        assert_eq!(cloned.payment_preimage, Some("def456".to_string()));
        assert_eq!(cloned.amount_msat, 1000);
        assert_eq!(cloned.fee_msat, 10);
    }

    #[test]
    fn test_invoice_update_unknown() {
        let update = InvoiceUpdate::Unknown {
            payment_hash: "abc123".to_string(),
        };
        let cloned = update.clone();
        if let InvoiceUpdate::Unknown { payment_hash } = cloned {
            assert_eq!(payment_hash, "abc123");
        } else {
            panic!("Expected Unknown variant");
        }
    }

    #[test]
    fn test_invoice_update_error() {
        let update = InvoiceUpdate::Error("Connection failed".to_string());
        let cloned = update.clone();
        if let InvoiceUpdate::Error(msg) = cloned {
            assert_eq!(msg, "Connection failed");
        } else {
            panic!("Expected Error variant");
        }
    }

    #[test]
    fn test_invoice_update_created() {
        let update = InvoiceUpdate::Created {
            payment_hash: "abc123".to_string(),
            payment_request: "lnbc...".to_string(),
        };
        let debug_str = format!("{:?}", update);
        assert!(debug_str.contains("abc123"));
    }

    #[test]
    fn test_invoice_update_canceled() {
        let update = InvoiceUpdate::Canceled {
            payment_hash: "abc123".to_string(),
        };
        if let InvoiceUpdate::Canceled { payment_hash } = update {
            assert_eq!(payment_hash, "abc123");
        } else {
            panic!("Expected Canceled variant");
        }
    }

    #[test]
    fn test_invoice_update_settled() {
        let update = InvoiceUpdate::Settled {
            payment_hash: "abc123".to_string(),
            preimage: Some("preimage456".to_string()),
            external_id: Some("ext789".to_string()),
        };
        if let InvoiceUpdate::Settled {
            payment_hash,
            preimage,
            external_id,
        } = update
        {
            assert_eq!(payment_hash, "abc123");
            assert_eq!(preimage, Some("preimage456".to_string()));
            assert_eq!(external_id, Some("ext789".to_string()));
        } else {
            panic!("Expected Settled variant");
        }
    }
}
