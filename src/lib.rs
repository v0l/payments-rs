//! # payments-rs
//!
//! A Rust library for integrating with multiple payment providers, supporting both
//! fiat (traditional currency) and Lightning Network (Bitcoin) payments.
//!
//! ## Features
//!
//! This library uses feature flags to enable only the payment methods you need:
//!
//! - `method-lnd` - LND (Lightning Network Daemon) integration
//! - `method-bitvora` - Bitvora Lightning payment provider
//! - `method-revolut` - Revolut merchant API integration
//! - `method-stripe` - Stripe payment processing
//!
//! ## Example
//!
//! ```rust,ignore
//! use payments_rs::fiat::{StripeApi, StripeConfig, FiatPaymentService};
//! use payments_rs::currency::{Currency, CurrencyAmount};
//!
//! let config = StripeConfig {
//!     url: None, // Uses default Stripe API URL
//!     api_key: "sk_test_...".to_string(),
//!     webhook_secret: Some("whsec_...".to_string()),
//! };
//!
//! let stripe = StripeApi::new(config)?;
//! let amount = CurrencyAmount::from_f32(Currency::USD, 20.00);
//! let payment = stripe.create_order("Order #123", amount, None).await?;
//! ```

/// User-Agent string used for all HTTP requests.
pub(crate) const USER_AGENT: &str = concat!("payments-rs/", env!("CARGO_PKG_VERSION"));

#[cfg(feature = "fiat")]
pub mod currency;

#[cfg(feature = "lightning")]
pub mod lightning;

#[cfg(feature = "json-api")]
pub mod json_api;

#[cfg(feature = "webhook")]
pub mod webhook;

#[cfg(feature = "fiat")]
pub mod fiat;
