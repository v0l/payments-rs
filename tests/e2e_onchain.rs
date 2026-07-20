//! On-chain LND integration tests against a real regtest node.
//!
//! Ignored by default. Bring the stack up with `scripts/e2e-up.sh`, then:
//!
//! ```bash
//! PAYMENTS_RS_E2E=1 cargo test --test e2e_onchain -- --ignored --nocapture
//! ```

mod common;

use futures::StreamExt;
use payments_rs::currency::{Currency, CurrencyAmount};
use payments_rs::lightning::setup_crypto_provider;
use payments_rs::onchain::{
    ChainPaymentUpdate, LndAddressType, LndOnChainConfig, LndOnChainProvider, NewAddressRequest,
    OnChainProvider,
};
use std::time::Duration;

async fn provider(min_confirmations: u32) -> LndOnChainProvider {
    setup_crypto_provider();
    LndOnChainProvider::new(
        &common::lnd_address(),
        &common::lnd_cert(),
        &common::lnd_macaroon(),
        LndOnChainConfig {
            address_type: LndAddressType::WitnessPubkeyHash,
            account: None,
            min_confirmations,
        },
    )
    .await
    .expect("connect to LND")
}

#[tokio::test]
#[ignore = "requires regtest stack (scripts/e2e-up.sh) and PAYMENTS_RS_E2E=1"]
async fn new_address_returns_regtest_address() {
    if !common::e2e_enabled() {
        eprintln!("skipping: PAYMENTS_RS_E2E != 1");
        return;
    }
    let provider = provider(1).await;
    let rsp = provider
        .new_address(NewAddressRequest {
            amount: CurrencyAmount::from_f32(Currency::BTC, 0.001),
            memo: Some("integration test".to_string()),
            label: Some("order-1".to_string()),
        })
        .await
        .expect("derive address");

    assert!(
        rsp.address.starts_with("bcrt1"),
        "expected a regtest bech32 address, got {}",
        rsp.address
    );
    assert_eq!(rsp.label, Some("order-1".to_string()));
}

#[tokio::test]
#[ignore = "requires regtest stack (scripts/e2e-up.sh) and PAYMENTS_RS_E2E=1"]
async fn deposit_is_detected_and_confirmed() {
    if !common::e2e_enabled() {
        eprintln!("skipping: PAYMENTS_RS_E2E != 1");
        return;
    }
    let provider = provider(1).await;

    // Derive the receive address for our "order".
    let addr = provider
        .new_address(NewAddressRequest {
            amount: CurrencyAmount::from_f32(Currency::BTC, 0.001),
            memo: None,
            label: Some("order-42".to_string()),
        })
        .await
        .expect("derive address")
        .address;
    eprintln!("derived address {addr}");

    // Broadcast 0.001 BTC (= 100_000 sats = 100_000_000 msat) and confirm it.
    //
    // NB: LND's SubscribeTransactions defers the gRPC response headers until it
    // has something to stream, so `subscribe_payments().await` only returns once
    // at least one matching transaction exists. We therefore broadcast first and
    // subscribe afterwards, which also exercises resumable/historical replay.
    let txid = common::send_to_address(&addr, 0.001)
        .await
        .expect("send funds");
    eprintln!("sent txid {txid}");
    common::mine_blocks(1).await.expect("mine block");
    eprintln!("mined 1 block");

    // Subscribe from genesis and read the historical (now confirmed) deposit.
    let mut updates = provider.subscribe_payments(None).await.expect("subscribe");

    let amount_msat = tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(update) = updates.next().await {
            match update {
                ChainPaymentUpdate::Detected { .. } => {}
                ChainPaymentUpdate::Confirmed {
                    address,
                    txid: t,
                    amount_msat,
                    confirmations,
                    ..
                } if address == addr && t == txid => {
                    assert!(confirmations >= 1);
                    return Some(amount_msat);
                }
                ChainPaymentUpdate::Confirmed { .. } => {}
                ChainPaymentUpdate::Error(e) => panic!("chain error: {e}"),
            }
        }
        None
    })
    .await
    .expect("timed out waiting for confirmed deposit")
    .expect("stream ended before confirmation");

    assert_eq!(
        amount_msat, 100_000_000,
        "0.001 BTC should be 100_000_000 msat"
    );
    assert!(!txid.is_empty(), "txid must be surfaced");
}
