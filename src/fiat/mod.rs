/// Fiat payment integrations
use crate::currency::CurrencyAmount;
use anyhow::Result;
use std::future::Future;
use std::pin::Pin;

#[cfg(feature = "method-revolut")]
mod revolut;
#[cfg(feature = "method-revolut")]
pub use revolut::*;

#[cfg(feature = "method-stripe")]
mod stripe;
#[cfg(feature = "method-stripe")]
pub use stripe::*;

/// A single line item in a payment order
#[derive(Debug, Clone)]
pub struct LineItem {
    /// Name/title of the item
    pub name: String,
    /// Description of the item (optional)
    pub description: Option<String>,
    /// Unit price in smallest currency unit (e.g., cents for USD)
    pub unit_amount: u64,
    /// Quantity of this item
    pub quantity: u64,
    /// Currency code (e.g., "USD", "EUR")
    pub currency: String,
    /// Optional image URLs for the item
    pub images: Option<Vec<String>>,
    /// Optional metadata for the item
    pub metadata: Option<serde_json::Value>,
    /// Tax amount in smallest currency unit (optional)
    pub tax_amount: Option<u64>,
    /// Tax name/description (e.g., "VAT", "Sales Tax") (optional)
    pub tax_name: Option<String>,
}

impl LineItem {
    /// Calculate total amount for this line item (including tax)
    pub fn total_amount(&self) -> u64 {
        let subtotal = self.unit_amount * self.quantity;
        subtotal + self.tax_amount.unwrap_or(0)
    }
    
    /// Calculate subtotal amount (before tax)
    pub fn subtotal_amount(&self) -> u64 {
        self.unit_amount * self.quantity
    }
}

pub trait FiatPaymentService: Send + Sync {
    /// Create an order with line items
    fn create_order(
        &self,
        description: &str,
        amount: CurrencyAmount,
        line_items: Option<Vec<LineItem>>,
    ) -> Pin<Box<dyn Future<Output = Result<FiatPaymentInfo>> + Send>>;

    fn cancel_order(&self, id: &str) -> Pin<Box<dyn Future<Output = Result<()>> + Send>>;
}

#[derive(Debug)]
pub struct FiatPaymentInfo {
    /// External Payment ID
    pub external_id: String,
    /// Raw JSON object
    pub raw_data: String,
}
