//! Currency types and amounts for payment processing.
//!
//! This module provides types for representing currencies and monetary amounts
//! in a type-safe manner.

use anyhow::{Result, ensure};
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
    /// Monero
    XMR,
    /// Litecoin
    LTC,
    /// Ethereum
    ETH,
    /// Tether (USDT) on the Tron network
    USDTTRC,
}

impl Currency {
    /// Every supported currency.
    pub const ALL: [Currency; 12] = [
        Currency::EUR,
        Currency::BTC,
        Currency::USD,
        Currency::GBP,
        Currency::CAD,
        Currency::CHF,
        Currency::AUD,
        Currency::JPY,
        Currency::XMR,
        Currency::LTC,
        Currency::ETH,
        Currency::USDTTRC,
    ];

    /// True for cryptocurrencies, which are held in milli-units (1e-11 of a coin)
    /// and can never be charged through a fiat payment service.
    pub fn is_crypto(&self) -> bool {
        matches!(
            self,
            Currency::BTC | Currency::XMR | Currency::LTC | Currency::ETH | Currency::USDTTRC
        )
    }

    /// Smallest units per standard unit: 1e11 for crypto, 100 for fiat.
    ///
    /// Crypto shares one scale so every coin fits a `u64` (ETH has 18 decimals,
    /// which would overflow at about 18 ETH in native units).
    pub fn scale(&self) -> f64 {
        if self.is_crypto() { 1.0e11 } else { 100.0 }
    }
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
            Currency::XMR => write!(f, "XMR"),
            Currency::LTC => write!(f, "LTC"),
            Currency::ETH => write!(f, "ETH"),
            Currency::USDTTRC => write!(f, "USDTTRC"),
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
            "xmr" => Ok(Currency::XMR),
            "ltc" => Ok(Currency::LTC),
            "eth" => Ok(Currency::ETH),
            "usdttrc" => Ok(Currency::USDTTRC),
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
    /// For cryptocurrencies, this expects the coin amount (e.g., 0.001 for 0.001 BTC).
    pub fn from_f32(currency: Currency, amount: f32) -> Self {
        if currency.is_crypto() {
            CurrencyAmount(currency, (amount as f64 * currency.scale()) as u64)
        } else {
            CurrencyAmount(currency, (amount * 100.0) as u64)
        }
    }

    /// Get the raw value in the smallest unit.
    pub fn value(&self) -> u64 {
        self.1
    }

    /// Get the value as a floating-point number in the standard unit.
    pub fn value_f32(&self) -> f32 {
        if self.0.is_crypto() {
            (self.1 as f64 / self.0.scale()) as f32
        } else {
            self.1 as f32 / 100.0
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
        if self.0.is_crypto() {
            write!(f, "{} {:.8}", self.0, self.value_f32())
        } else {
            write!(f, "{} {:.2}", self.0, self.value_f32())
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
        assert_eq!(Currency::XMR.to_string(), "XMR");
        assert_eq!(Currency::LTC.to_string(), "LTC");
        assert_eq!(Currency::ETH.to_string(), "ETH");
        assert_eq!(Currency::USDTTRC.to_string(), "USDTTRC");
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
        assert_eq!("xmr".parse::<Currency>(), Ok(Currency::XMR));
        assert_eq!("XMR".parse::<Currency>(), Ok(Currency::XMR));
        assert_eq!("ltc".parse::<Currency>(), Ok(Currency::LTC));
        assert_eq!("eth".parse::<Currency>(), Ok(Currency::ETH));
        assert_eq!("USDTTRC".parse::<Currency>(), Ok(Currency::USDTTRC));
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
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Subtraction would underflow")
        );
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
    fn test_currency_all_round_trips_display_and_parse() {
        for c in Currency::ALL {
            assert_eq!(c.to_string().parse::<Currency>(), Ok(c));
        }
        let unique: std::collections::HashSet<_> = Currency::ALL.iter().collect();
        assert_eq!(unique.len(), Currency::ALL.len());
    }

    #[test]
    fn test_currency_is_crypto() {
        for c in [
            Currency::BTC,
            Currency::XMR,
            Currency::LTC,
            Currency::ETH,
            Currency::USDTTRC,
        ] {
            assert!(c.is_crypto(), "{c}");
            assert_eq!(c.scale(), 1.0e11, "{c}");
        }
        for c in [
            Currency::EUR,
            Currency::USD,
            Currency::GBP,
            Currency::CAD,
            Currency::CHF,
            Currency::AUD,
            Currency::JPY,
        ] {
            assert!(!c.is_crypto(), "{c}");
            assert_eq!(c.scale(), 100.0, "{c}");
        }
    }

    #[test]
    fn test_currency_amount_crypto_scale() {
        for c in [
            Currency::XMR,
            Currency::LTC,
            Currency::ETH,
            Currency::USDTTRC,
        ] {
            let amount = CurrencyAmount::from_f32(c, 1.0);
            assert_eq!(amount.value(), 100_000_000_000, "{c}");
            assert!((amount.value_f32() - 1.0).abs() < 0.0001, "{c}");
            assert_eq!(amount.to_string(), format!("{c} 1.00000000"));
        }
    }

    #[test]
    fn test_currency_amount_sub_crypto_mismatch() {
        let a = CurrencyAmount::from_u64(Currency::XMR, 2000);
        let b = CurrencyAmount::from_u64(Currency::LTC, 500);
        assert!((a - b).is_err());
    }

    #[test]
    fn test_currency_amount_display_btc() {
        let amount = CurrencyAmount::from_u64(Currency::BTC, 100_000_000_000);
        assert_eq!(amount.to_string(), "BTC 1.00000000");
    }
}
