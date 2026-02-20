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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_item_total_amount_without_tax() {
        let item = LineItem {
            name: "Test Item".to_string(),
            description: None,
            unit_amount: 1000,
            quantity: 2,
            currency: "USD".to_string(),
            images: None,
            metadata: None,
            tax_amount: None,
            tax_name: None,
        };
        assert_eq!(item.total_amount(), 2000);
    }

    #[test]
    fn test_line_item_total_amount_with_tax() {
        let item = LineItem {
            name: "Test Item".to_string(),
            description: Some("A test item".to_string()),
            unit_amount: 1000,
            quantity: 2,
            currency: "USD".to_string(),
            images: Some(vec!["https://example.com/image.jpg".to_string()]),
            metadata: None,
            tax_amount: Some(200),
            tax_name: Some("VAT".to_string()),
        };
        assert_eq!(item.total_amount(), 2200); // 2000 + 200 tax
    }

    #[test]
    fn test_line_item_subtotal_amount() {
        let item = LineItem {
            name: "Test Item".to_string(),
            description: None,
            unit_amount: 1000,
            quantity: 3,
            currency: "USD".to_string(),
            images: None,
            metadata: None,
            tax_amount: Some(300),
            tax_name: Some("Sales Tax".to_string()),
        };
        assert_eq!(item.subtotal_amount(), 3000);
        assert_eq!(item.total_amount(), 3300);
    }

    #[test]
    fn test_line_item_clone() {
        let item = LineItem {
            name: "Test Item".to_string(),
            description: Some("Description".to_string()),
            unit_amount: 500,
            quantity: 1,
            currency: "EUR".to_string(),
            images: None,
            metadata: Some(serde_json::json!({"key": "value"})),
            tax_amount: None,
            tax_name: None,
        };
        let cloned = item.clone();
        assert_eq!(cloned.name, item.name);
        assert_eq!(cloned.unit_amount, item.unit_amount);
    }

    #[test]
    fn test_fiat_payment_info_debug() {
        let info = FiatPaymentInfo {
            external_id: "ext_123".to_string(),
            raw_data: r#"{"id": "123"}"#.to_string(),
        };
        let debug_str = format!("{:?}", info);
        assert!(debug_str.contains("ext_123"));
    }
}
