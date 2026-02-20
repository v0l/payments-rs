//! Fiat payment provider integrations.
//!
//! This module provides integrations with traditional payment processors
//! including Stripe and Revolut.
//!
//! # Supported Providers
//!
//! - **Stripe** (`method-stripe` feature) - Full checkout session and payment intent support
//! - **Revolut** (`method-revolut` feature) - Merchant API integration with order management
//!
//! # Example
//!
//! ```rust,ignore
//! use payments_rs::fiat::{FiatPaymentService, StripeApi, StripeConfig};
//! use payments_rs::currency::{Currency, CurrencyAmount};
//!
//! let stripe = StripeApi::new(StripeConfig {
//!     url: None,
//!     api_key: "sk_test_...".to_string(),
//!     webhook_secret: None,
//! })?;
//!
//! let amount = CurrencyAmount::from_f32(Currency::USD, 50.00);
//! let payment = stripe.create_order("Order #123", amount, None).await?;
//! ```

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

/// A single line item in a payment order.
///
/// Line items provide detailed breakdown of what is being purchased,
/// which can be displayed to customers and used for reporting.
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

/// Trait for fiat payment service providers.
///
/// Implement this trait to add support for additional payment processors.
/// Both Stripe and Revolut implement this trait, allowing for provider-agnostic
/// payment processing.
pub trait FiatPaymentService: Send + Sync {
    /// Create a payment order.
    ///
    /// # Arguments
    ///
    /// * `description` - A human-readable description of the order
    /// * `amount` - The total amount to charge
    /// * `line_items` - Optional detailed breakdown of items being purchased
    ///
    /// # Returns
    ///
    /// Returns [`FiatPaymentInfo`] containing the external ID and raw response data.
    fn create_order(
        &self,
        description: &str,
        amount: CurrencyAmount,
        line_items: Option<Vec<LineItem>>,
    ) -> Pin<Box<dyn Future<Output = Result<FiatPaymentInfo>> + Send>>;

    /// Cancel an existing order.
    ///
    /// # Arguments
    ///
    /// * `id` - The external ID of the order to cancel
    fn cancel_order(&self, id: &str) -> Pin<Box<dyn Future<Output = Result<()>> + Send>>;
}

/// Information about a created fiat payment.
#[derive(Debug)]
pub struct FiatPaymentInfo {
    /// External payment ID from the provider
    pub external_id: String,
    /// Raw JSON response from the provider
    pub raw_data: String,
}
