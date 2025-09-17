use anyhow::Result;
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

/// Generic lightning node for creating payments
#[async_trait]
pub trait LightningNode: Send + Sync {
    async fn add_invoice(&self, req: AddInvoiceRequest) -> Result<AddInvoiceResponse>;
    async fn cancel_invoice(&self, id: &Vec<u8>) -> Result<()>;
    async fn subscribe_invoices(
        &self,
        from_payment_hash: Option<Vec<u8>>,
    ) -> Result<Pin<Box<dyn Stream<Item = InvoiceUpdate> + Send>>>;
}

#[derive(Debug, Clone)]
pub struct AddInvoiceRequest {
    pub amount: u64,
    pub memo: Option<String>,
    pub expire: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct AddInvoiceResponse {
    pub external_id: Option<String>,
    pub parsed_invoice: Bolt11Invoice,
}

impl AddInvoiceResponse {
    pub fn pr(&self) -> String {
        self.parsed_invoice.to_string()
    }

    pub fn description_hash(&self) -> String {
        self.parsed_invoice.payment_hash().encode_hex()
    }
}

#[derive(Debug, Clone)]
pub enum InvoiceUpdate {
    /// Internal impl created an update which we don't support or care about
    Unknown {
        payment_hash: String,
    },
    Error(String),
    Created {
        payment_hash: String,
        payment_request: String,
    },
    Canceled {
        payment_hash: String,
    },
    Settled {
        payment_hash: String,
        preimage: Option<String>,
        external_id: Option<String>,
    },
}
