# payments-rs

A Rust library for integrating with multiple payment providers, supporting both fiat and Bitcoin Lightning Network payments through a unified trait-based interface.

## Supported Providers

| Provider | Type | Feature Flag |
|----------|------|-------------|
| [Stripe](https://stripe.com) | Fiat | `method-stripe` |
| [Revolut](https://www.revolut.com/business) | Fiat | `method-revolut` |
| [LND](https://github.com/lightningnetwork/lnd) | Lightning | `method-lnd` |
| [LND](https://github.com/lightningnetwork/lnd) | On-chain (receive) | `method-lnd-onchain` |
| [Bitvora](https://bitvora.com) | Lightning | `method-bitvora` _(deprecated)_ |

## Usage

Add to your `Cargo.toml` with only the providers you need:

```toml
[dependencies]
payments-rs = { version = "0.2", default-features = false, features = ["method-stripe"] }
```

All providers are enabled by default.

### Fiat Payments

Fiat providers implement the `FiatPaymentService` trait:

```rust,ignore
use payments_rs::fiat::{StripeApi, StripeConfig, FiatPaymentService};
use payments_rs::currency::{Currency, CurrencyAmount};

let config = StripeConfig {
    url: None,
    api_key: "sk_test_...".to_string(),
    webhook_secret: Some("whsec_...".to_string()),
};

let stripe = StripeApi::new(config)?;
let amount = CurrencyAmount::from_f32(Currency::USD, 20.00);
let payment = stripe.create_order("Order #123", amount, None).await?;
```

### On-chain Payments

On-chain providers implement the `OnChainProvider` trait for receiving Bitcoin
deposits. Derive a fresh receive address per order and stream chain events
(amounts are reported in milli-satoshis, carrying the real `txid`):

```rust,ignore
use payments_rs::onchain::{LndOnChainProvider, LndOnChainConfig, LndAddressType, OnChainProvider, NewAddressRequest};
use payments_rs::currency::{Currency, CurrencyAmount};
use payments_rs::lightning::setup_crypto_provider;
use std::path::Path;

setup_crypto_provider();
let provider = LndOnChainProvider::new(
    "https://localhost:10009",
    Path::new("/path/to/tls.cert"),
    Path::new("/path/to/admin.macaroon"),
    LndOnChainConfig {
        address_type: LndAddressType::WitnessPubkeyHash,
        account: None,
        min_confirmations: 1,
    },
).await?;

let address = provider.new_address(NewAddressRequest {
    amount: CurrencyAmount::from_f32(Currency::BTC, 0.001),
    memo: Some("Order #123".to_string()),
    label: Some("order-123".to_string()),
}).await?;
```

### Lightning Payments

Lightning providers implement the `LightningNode` trait:

```rust,ignore
use payments_rs::lightning::{LndNode, LightningNode, AddInvoiceRequest};
use payments_rs::currency::CurrencyAmount;

let node = LndNode::new("https://localhost:10009", "/path/to/tls.cert", "/path/to/admin.macaroon").await?;
let invoice = node.add_invoice(AddInvoiceRequest {
    memo: "Payment for order #123".to_string(),
    amount: CurrencyAmount::millisats(100_000),
    expire: None,
}).await?;
```

## Feature Flags

| Feature | Description |
|---------|-------------|
| `method-lnd` | LND gRPC integration (default) |
| `method-lnd-onchain` | LND on-chain (receive) integration (default) |
| `method-bitvora` | Bitvora REST API integration (default, **deprecated** — no longer operational) |
| `mock` | `MockOnChainProvider` for downstream integration tests |
| `method-revolut` | Revolut Merchant API integration (default) |
| `method-stripe` | Stripe payment processing (default) |
| `tls-ring` | Use `ring` for TLS (default) |
| `tls-aws` | Use `aws-lc-rs` for TLS (mutually exclusive with `tls-ring`) |
| `webhook` | Webhook signature verification and message bridge |
| `rocket` | Rocket web framework integration for webhooks |

## License

MIT
