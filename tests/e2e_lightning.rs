//! Lightning LND integration smoke test against a real regtest node.
//!
//! Ignored by default. Bring the stack up with `scripts/e2e-up.sh`, then:
//!
//! ```bash
//! PAYMENTS_RS_E2E=1 cargo test --test e2e_lightning -- --ignored --nocapture
//! ```

mod common;

use payments_rs::lightning::{AddInvoiceRequest, LightningNode, LndNode, setup_crypto_provider};

#[tokio::test]
#[ignore = "requires regtest stack (scripts/e2e-up.sh) and PAYMENTS_RS_E2E=1"]
async fn add_invoice_against_real_lnd() {
    if !common::e2e_enabled() {
        eprintln!("skipping: PAYMENTS_RS_E2E != 1");
        return;
    }
    setup_crypto_provider();
    let lnd = LndNode::new(
        &common::lnd_address(),
        &common::lnd_cert(),
        &common::lnd_macaroon(),
    )
    .await
    .expect("connect to LND");

    let invoice = lnd
        .add_invoice(AddInvoiceRequest {
            amount: 100_000, // 100k msat = 100 sats
            memo: Some("integration test".to_string()),
            expire: Some(3600),
        })
        .await
        .expect("create invoice");

    // A real node must return a parseable BOLT11 with a non-empty payment hash.
    assert!(
        invoice.pr().starts_with("lnbcrt"),
        "expected a regtest invoice, got {}",
        invoice.pr()
    );
    assert!(!invoice.payment_hash().is_empty());
}
