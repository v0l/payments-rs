use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;
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
    async fn add_invoice(&self, req: AddInvoiceRequest) -> Result<AddInvoiceResult>;
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
pub struct AddInvoiceResult {
    pub pr: String,
    pub payment_hash: String,
    pub external_id: Option<String>,
}

#[derive(Debug, Clone)]
pub enum InvoiceUpdate {
    /// Internal impl created an update which we don't support or care about
    Unknown,
    Error(String),
    Settled {
        payment_hash: Option<String>,
        external_id: Option<String>,
    },
}
