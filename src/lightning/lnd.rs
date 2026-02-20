//! LND (Lightning Network Daemon) integration.
//!
//! This module requires a running LND node and cannot be unit tested without one.
//! Coverage exclusions are applied to async methods that require network access.

use crate::lightning::{
    AddInvoiceRequest, AddInvoiceResponse, InvoiceUpdate, LightningNode, PayInvoiceRequest,
    PayInvoiceResponse,
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use fedimint_tonic_lnd::invoicesrpc::lookup_invoice_msg::InvoiceRef;
use fedimint_tonic_lnd::invoicesrpc::{CancelInvoiceMsg, LookupInvoiceMsg};
use fedimint_tonic_lnd::lnrpc::invoice::InvoiceState;
use fedimint_tonic_lnd::lnrpc::{Invoice, InvoiceSubscription};
use fedimint_tonic_lnd::routerrpc::SendPaymentRequest;
use fedimint_tonic_lnd::{Client, connect};
use futures::{Stream, StreamExt};
use std::path::Path;
use std::pin::Pin;
use std::sync::Once;

static INIT_CRYPTO: Once = Once::new();

/// Initialize the rustls crypto provider.
///
/// This must be called before creating any [`LndNode`] connections.
/// It is safe to call multiple times; only the first call has any effect.
///
/// # Example
///
/// ```rust,ignore
/// use payments_rs::lightning::setup_crypto_provider;
///
/// fn main() {
///     setup_crypto_provider();
///     // Now you can create LndNode connections
/// }
/// ```
pub fn setup_crypto_provider() {
    INIT_CRYPTO.call_once(|| {
        // Only install if no provider is already set
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            #[cfg(feature = "tls-ring")]
            let provider = rustls::crypto::ring::default_provider();
            #[cfg(all(feature = "tls-aws", not(feature = "tls-ring")))]
            let provider = rustls::crypto::aws_lc_rs::default_provider();
            
            let _ = provider.install_default();
        }
    });
}

/// LND (Lightning Network Daemon) client.
///
/// Provides direct connection to an LND node for creating invoices,
/// paying invoices, and subscribing to invoice updates.
///
/// # Example
///
/// ```rust,ignore
/// use payments_rs::lightning::{LndNode, LightningNode, AddInvoiceRequest, setup_crypto_provider};
/// use std::path::Path;
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     setup_crypto_provider();
///     
///     let lnd = LndNode::new(
///         "https://localhost:10009",
///         Path::new("/path/to/tls.cert"),
///         Path::new("/path/to/admin.macaroon"),
///     ).await?;
///     
///     let invoice = lnd.add_invoice(AddInvoiceRequest {
///         amount: 1000,
///         memo: Some("Test payment".to_string()),
///         expire: None,
///     }).await?;
///     
///     println!("Pay this invoice: {}", invoice.pr());
///     Ok(())
/// }
/// ```
#[derive(Clone)]
pub struct LndNode {
    client: Client,
}

impl LndNode {
    /// Create a new LND client connection.
    ///
    /// # Arguments
    ///
    /// * `url` - The gRPC URL of the LND node (e.g., "https://localhost:10009")
    /// * `cert` - Path to the TLS certificate file (tls.cert)
    /// * `macaroon` - Path to the macaroon file (admin.macaroon or invoice.macaroon)
    ///
    /// # Note
    ///
    /// You must call [`setup_crypto_provider`] before creating connections.
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub async fn new(url: &str, cert: &Path, macaroon: &Path) -> Result<Self> {
        let lnd = connect(
            url.to_string(),
            cert.to_str().unwrap(),
            macaroon.to_str().unwrap(),
        )
        .await
        .map_err(|e| anyhow!("Failed to connect to LND: {}", e))?;

        Ok(Self { client: lnd })
    }

    /// Get a clone of the underlying LND client for advanced operations.
    pub fn client(&self) -> Client {
        self.client.clone()
    }
}

#[async_trait]
#[cfg_attr(coverage_nightly, coverage(off))]
impl LightningNode for LndNode {
    async fn add_invoice(&self, req: AddInvoiceRequest) -> Result<AddInvoiceResponse> {
        let mut client = self.client.clone();
        let ln = client.lightning();
        let res = ln
            .add_invoice(Invoice {
                memo: req.memo.unwrap_or_default(),
                value_msat: req.amount as i64,
                expiry: req.expire.unwrap_or(3600) as i64,
                ..Default::default()
            })
            .await?;

        let inner = res.into_inner();
        Ok(AddInvoiceResponse::from_invoice(
            &inner.payment_request,
            None,
        )?)
    }

    async fn cancel_invoice(&self, id: &[u8]) -> Result<()> {
        let mut client = self.client.clone();
        let ln = client.invoices();
        ln.cancel_invoice(CancelInvoiceMsg {
            payment_hash: id.to_vec(),
        })
        .await?;
        Ok(())
    }

    async fn pay_invoice(&self, req: PayInvoiceRequest) -> Result<PayInvoiceResponse> {
        let mut client = self.client.clone();
        let router = client.router();
        let mut stream = router
            .send_payment_v2(SendPaymentRequest {
                payment_request: req.invoice.clone(),
                timeout_seconds: req.timeout_seconds.unwrap_or(60) as i32,
                ..Default::default()
            })
            .await?
            .into_inner();

        // Wait for the final payment result
        let mut final_result = None;
        while let Some(update) = stream.message().await? {
            // LND sends multiple updates, we want the final one
            final_result = Some(update);
        }

        let payment = final_result.ok_or_else(|| anyhow!("No payment result received"))?;
        
        if payment.status != 2 {
            // 2 = SUCCEEDED
            let failure_reason = if !payment.failure_reason().as_str_name().is_empty() {
                payment.failure_reason().as_str_name()
            } else {
                "Unknown failure"
            };
            return Err(anyhow!("Payment failed: {}", failure_reason));
        }

        Ok(PayInvoiceResponse {
            payment_hash: hex::encode(&payment.payment_hash),
            payment_preimage: Some(hex::encode(&payment.payment_preimage)),
            amount_msat: payment.value_msat as u64,
            fee_msat: payment.fee_msat as u64,
        })
    }

    async fn subscribe_invoices(
        &self,
        from_payment_hash: Option<Vec<u8>>,
    ) -> Result<Pin<Box<dyn Stream<Item = InvoiceUpdate> + Send>>> {
        let mut client = self.client.clone();
        let from_settle_index = if let Some(ph) = from_payment_hash {
            if let Ok(inv) = client
                .invoices()
                .lookup_invoice_v2(LookupInvoiceMsg {
                    lookup_modifier: 0,
                    invoice_ref: Some(InvoiceRef::PaymentHash(ph)),
                })
                .await
            {
                inv.into_inner().settle_index
            } else {
                0
            }
        } else {
            0
        };

        let stream = client
            .lightning()
            .subscribe_invoices(InvoiceSubscription {
                add_index: 0,
                settle_index: from_settle_index,
            })
            .await?;

        let stream = stream.into_inner();
        Ok(Box::pin(stream.map(|i| match i {
            Ok(m) => {
                const SETTLED: i32 = InvoiceState::Settled as i32;
                const CREATED: i32 = InvoiceState::Open as i32;
                const CANCELED: i32 = InvoiceState::Canceled as i32;
                let payment_hash = hex::encode(m.r_hash);
                match m.state {
                    SETTLED => InvoiceUpdate::Settled {
                        payment_hash,
                        preimage: Some(hex::encode(m.r_preimage)),
                        external_id: None,
                    },
                    CREATED => InvoiceUpdate::Created {
                        payment_hash,
                        payment_request: m.payment_request,
                    },
                    CANCELED => InvoiceUpdate::Canceled { payment_hash },
                    _ => InvoiceUpdate::Unknown { payment_hash },
                }
            }
            Err(e) => InvoiceUpdate::Error(e.to_string()),
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_setup_crypto_provider() {
        // Should not panic when called
        setup_crypto_provider();
    }

    #[test]
    fn test_setup_crypto_provider_idempotent() {
        // Should be safe to call multiple times
        setup_crypto_provider();
        setup_crypto_provider();
        setup_crypto_provider();
    }
}
