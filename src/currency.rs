//! Currency types and amounts for payment processing.
//!
//! This module provides types for representing currencies and monetary amounts
//! in a type-safe manner.

use anyhow::{ensure, Result};
use std::fmt::{Display, Formatter};
use std::ops::Sub;
use std::str::FromStr;

/// Supported currency types for payment processing.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Currency {
    /// Euro
    EUR,
    /// Bitcoin (stored internally as milli-satoshis)
    BTC,
    /// US Dollar
    USD,
    /// British Pound Sterling
    GBP,
    /// Canadian Dollar
    CAD,
    /// Swiss Franc
    CHF,
    /// Australian Dollar
    AUD,
    /// Japanese Yen
    JPY,
}

impl Display for Currency {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Currency::EUR => write!(f, "EUR"),
            Currency::BTC => write!(f, "BTC"),
            Currency::USD => write!(f, "USD"),
            Currency::GBP => write!(f, "GBP"),
            Currency::CAD => write!(f, "CAD"),
            Currency::CHF => write!(f, "CHF"),
            Currency::AUD => write!(f, "AUD"),
            Currency::JPY => write!(f, "JPY"),
        }
    }
}

/// Error returned when parsing an invalid currency string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseCurrencyError(String);

impl Display for ParseCurrencyError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown currency: {}", self.0)
    }
}

impl std::error::Error for ParseCurrencyError {}

impl FromStr for Currency {
    type Err = ParseCurrencyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "eur" => Ok(Currency::EUR),
            "usd" => Ok(Currency::USD),
            "btc" => Ok(Currency::BTC),
            "gbp" => Ok(Currency::GBP),
            "cad" => Ok(Currency::CAD),
            "chf" => Ok(Currency::CHF),
            "aud" => Ok(Currency::AUD),
            "jpy" => Ok(Currency::JPY),
            _ => Err(ParseCurrencyError(s.to_string())),
        }
    }
}

/// A monetary amount with an associated currency.
///
/// For fiat currencies, amounts are stored in the smallest unit (e.g., cents for USD).
/// For Bitcoin, amounts are stored in milli-satoshis.
///
/// # Example
///
/// ```
/// use payments_rs::currency::{Currency, CurrencyAmount};
///
/// // Create $20.00 USD
/// let usd = CurrencyAmount::from_f32(Currency::USD, 20.00);
/// assert_eq!(usd.value(), 2000); // 2000 cents
///
/// // Create 1000 milli-satoshis
/// let btc = CurrencyAmount::millisats(1000);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CurrencyAmount(Currency, u64);

impl CurrencyAmount {
    const MILLI_SATS: f64 = 1.0e11;

    /// Create a Bitcoin amount from milli-satoshis.
    pub fn millisats(amount: u64) -> Self {
        CurrencyAmount(Currency::BTC, amount)
    }

    /// Create a currency amount from the smallest unit (cents for fiat, milli-sats for BTC).
    pub fn from_u64(currency: Currency, amount: u64) -> Self {
        CurrencyAmount(currency, amount)
    }

    /// Create a currency amount from a floating-point value.
    ///
    /// For fiat currencies, this expects the standard unit (e.g., 20.00 for $20).
    /// For Bitcoin, this expects the BTC amount (e.g., 0.001 for 0.001 BTC).
    pub fn from_f32(currency: Currency, amount: f32) -> Self {
        CurrencyAmount(
            currency,
            match currency {
                Currency::BTC => (amount as f64 * Self::MILLI_SATS) as u64, // milli-sats
                _ => (amount * 100.0) as u64,                               // cents
            },
        )
    }

    /// Get the raw value in the smallest unit.
    pub fn value(&self) -> u64 {
        self.1
    }

    /// Get the value as a floating-point number in the standard unit.
    pub fn value_f32(&self) -> f32 {
        match self.0 {
            Currency::BTC => (self.1 as f64 / Self::MILLI_SATS) as f32,
            _ => self.1 as f32 / 100.0,
        }
    }

    /// Get the currency type.
    pub fn currency(&self) -> Currency {
        self.0
    }
}

impl Sub for CurrencyAmount {
    type Output = Result<CurrencyAmount>;

    fn sub(self, rhs: Self) -> Self::Output {
        ensure!(self.0 == rhs.0, "Currency doesnt match");
        let value = self
            .1
            .checked_sub(rhs.1)
            .ok_or_else(|| anyhow::anyhow!("Subtraction would underflow: {} - {}", self, rhs))?;
        Ok(CurrencyAmount::from_u64(self.0, value))
    }
}

impl Display for CurrencyAmount {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Currency::BTC => write!(f, "BTC {:.8}", self.value_f32()),
            _ => write!(f, "{} {:.2}", self.0, self.value_f32()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_currency_display() {
        assert_eq!(Currency::EUR.to_string(), "EUR");
        assert_eq!(Currency::BTC.to_string(), "BTC");
        assert_eq!(Currency::USD.to_string(), "USD");
        assert_eq!(Currency::GBP.to_string(), "GBP");
        assert_eq!(Currency::CAD.to_string(), "CAD");
        assert_eq!(Currency::CHF.to_string(), "CHF");
        assert_eq!(Currency::AUD.to_string(), "AUD");
        assert_eq!(Currency::JPY.to_string(), "JPY");
    }

    #[test]
    fn test_currency_from_str() {
        assert_eq!("eur".parse::<Currency>(), Ok(Currency::EUR));
        assert_eq!("EUR".parse::<Currency>(), Ok(Currency::EUR));
        assert_eq!("usd".parse::<Currency>(), Ok(Currency::USD));
        assert_eq!("btc".parse::<Currency>(), Ok(Currency::BTC));
        assert_eq!("gbp".parse::<Currency>(), Ok(Currency::GBP));
        assert_eq!("cad".parse::<Currency>(), Ok(Currency::CAD));
        assert_eq!("chf".parse::<Currency>(), Ok(Currency::CHF));
        assert_eq!("aud".parse::<Currency>(), Ok(Currency::AUD));
        assert_eq!("jpy".parse::<Currency>(), Ok(Currency::JPY));
        assert_eq!(
            "invalid".parse::<Currency>(),
            Err(ParseCurrencyError("invalid".to_string()))
        );
    }

    #[test]
    fn test_currency_amount_millisats() {
        let amount = CurrencyAmount::millisats(1000);
        assert_eq!(amount.currency(), Currency::BTC);
        assert_eq!(amount.value(), 1000);
    }

    #[test]
    fn test_currency_amount_from_u64() {
        let amount = CurrencyAmount::from_u64(Currency::USD, 2000);
        assert_eq!(amount.currency(), Currency::USD);
        assert_eq!(amount.value(), 2000);
    }

    #[test]
    fn test_currency_amount_from_f32_fiat() {
        let amount = CurrencyAmount::from_f32(Currency::USD, 20.00);
        assert_eq!(amount.currency(), Currency::USD);
        assert_eq!(amount.value(), 2000); // 2000 cents
    }

    #[test]
    fn test_currency_amount_from_f32_btc() {
        let amount = CurrencyAmount::from_f32(Currency::BTC, 1.0);
        assert_eq!(amount.currency(), Currency::BTC);
        assert_eq!(amount.value(), 100_000_000_000); // 1 BTC in milli-sats
    }

    #[test]
    fn test_currency_amount_value_f32_fiat() {
        let amount = CurrencyAmount::from_u64(Currency::USD, 2000);
        assert!((amount.value_f32() - 20.0).abs() < 0.01);
    }

    #[test]
    fn test_currency_amount_value_f32_btc() {
        let amount = CurrencyAmount::from_u64(Currency::BTC, 100_000_000_000);
        assert!((amount.value_f32() - 1.0).abs() < 0.0001);
    }

    #[test]
    fn test_currency_amount_sub_same_currency() {
        let a = CurrencyAmount::from_u64(Currency::USD, 2000);
        let b = CurrencyAmount::from_u64(Currency::USD, 500);
        let result = (a - b).unwrap();
        assert_eq!(result.value(), 1500);
        assert_eq!(result.currency(), Currency::USD);
    }

    #[test]
    fn test_currency_amount_sub_different_currency() {
        let a = CurrencyAmount::from_u64(Currency::USD, 2000);
        let b = CurrencyAmount::from_u64(Currency::EUR, 500);
        let result = a - b;
        assert!(result.is_err());
    }

    #[test]
    fn test_currency_amount_sub_underflow_returns_error() {
        let a = CurrencyAmount::from_u64(Currency::USD, 500);
        let b = CurrencyAmount::from_u64(Currency::USD, 2000);
        let result = a - b;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Subtraction would underflow"));
    }

    #[test]
    fn test_currency_amount_sub_equal_values() {
        let a = CurrencyAmount::from_u64(Currency::USD, 1000);
        let b = CurrencyAmount::from_u64(Currency::USD, 1000);
        let result = (a - b).unwrap();
        assert_eq!(result.value(), 0);
        assert_eq!(result.currency(), Currency::USD);
    }

    #[test]
    fn test_currency_amount_display_fiat() {
        let amount = CurrencyAmount::from_u64(Currency::USD, 2000);
        assert_eq!(amount.to_string(), "USD 20.00");
    }

    #[test]
    fn test_currency_amount_display_btc() {
        let amount = CurrencyAmount::from_u64(Currency::BTC, 100_000_000_000);
        assert_eq!(amount.to_string(), "BTC 1.00000000");
    }
}
