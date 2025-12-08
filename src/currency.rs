use anyhow::{Result, ensure};
use std::fmt::{Display, Formatter};
use std::ops::Sub;
use std::str::FromStr;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Currency {
    EUR,
    BTC,
    USD,
    GBP,
    CAD,
    CHF,
    AUD,
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

impl FromStr for Currency {
    type Err = ();

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
            _ => Err(()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CurrencyAmount(Currency, u64);

impl CurrencyAmount {
    const MILLI_SATS: f64 = 1.0e11;

    pub fn millisats(amount: u64) -> Self {
        CurrencyAmount(Currency::BTC, amount)
    }

    pub fn from_u64(currency: Currency, amount: u64) -> Self {
        CurrencyAmount(currency, amount)
    }

    pub fn from_f32(currency: Currency, amount: f32) -> Self {
        CurrencyAmount(
            currency,
            match currency {
                Currency::BTC => (amount as f64 * Self::MILLI_SATS) as u64, // milli-sats
                _ => (amount * 100.0) as u64,                               // cents
            },
        )
    }

    pub fn value(&self) -> u64 {
        self.1
    }

    pub fn value_f32(&self) -> f32 {
        match self.0 {
            Currency::BTC => (self.1 as f64 / Self::MILLI_SATS) as f32,
            _ => self.1 as f32 / 100.0,
        }
    }

    pub fn currency(&self) -> Currency {
        self.0
    }
}

impl Sub for CurrencyAmount {
    type Output = Result<CurrencyAmount>;

    fn sub(self, rhs: Self) -> Self::Output {
        ensure!(self.0 == rhs.0, "Currency doesnt match");
        Ok(CurrencyAmount::from_u64(self.0, self.1 - rhs.1))
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
