use anyhow::Result;
use futures::StreamExt;
use payments_rs::currency::{Currency, CurrencyAmount};
use payments_rs::lightning::setup_crypto_provider;
use payments_rs::onchain::{
    LndAddressType, LndOnChainConfig, LndOnChainProvider, NewAddressRequest, OnChainProvider,
    PaymentCursor,
};
use std::env::args;
use std::path::Path;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize the logger and rustls crypto provider (required before connecting).
    env_logger::init();
    setup_crypto_provider();

    // Args: <url> <tls.cert> <admin.macaroon>
    let url = args().nth(1).expect("lnd grpc url");
    let cert = args().nth(2).expect("tls cert path");
    let macaroon = args().nth(3).expect("macaroon path");

    // Connect to LND for on-chain receive.
    let provider = LndOnChainProvider::new(
        &url,
        Path::new(&cert),
        Path::new(&macaroon),
        LndOnChainConfig {
            address_type: LndAddressType::WitnessPubkeyHash,
            account: None,
            min_confirmations: 1,
        },
    )
    .await?;

    // Derive a fresh receive address tied to an order.
    let amount = CurrencyAmount::from_f32(Currency::BTC, 0.001);
    let address = provider
        .new_address(NewAddressRequest {
            amount,
            memo: Some("Order #123".to_string()),
            label: Some("order-123".to_string()),
        })
        .await?;
    println!("Send payment to: {}", address.address);

    // Subscribe to chain events, resuming from a persisted cursor if available.
    let resume_from: Option<PaymentCursor> = None;
    let mut updates = provider.subscribe_payments(resume_from).await?;
    println!("Watching for deposits... (ctrl-c to exit)");
    while let Some(update) = updates.next().await {
        println!("chain update: {:?}", update);
    }

    Ok(())
}
