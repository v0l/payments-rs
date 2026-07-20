//! Shared helpers for the regtest LND integration tests.
//!
//! These tests are `#[ignore]`d by default and only run when the regtest stack
//! from `docker-compose.e2e.yaml` is up and `PAYMENTS_RS_E2E=1` is set (see
//! `scripts/e2e-up.sh`).

#![allow(dead_code)]

use std::path::PathBuf;

/// docker-compose file used by the integration stack.
pub const COMPOSE_FILE: &str = "docker-compose.e2e.yaml";

/// Returns `true` when the e2e stack is expected to be available.
pub fn e2e_enabled() -> bool {
    std::env::var("PAYMENTS_RS_E2E").as_deref() == Ok("1")
}

/// LND gRPC address (default `https://localhost:10009`).
pub fn lnd_address() -> String {
    std::env::var("PAYMENTS_RS_E2E_LND_ADDRESS")
        .unwrap_or_else(|_| "https://localhost:10009".to_string())
}

/// Path to the LND TLS certificate copied out by `scripts/e2e-up.sh`.
pub fn lnd_cert() -> PathBuf {
    std::env::var("PAYMENTS_RS_E2E_LND_CERT")
        .unwrap_or_else(|_| "/tmp/payments-rs-e2e-lnd/tls.cert".to_string())
        .into()
}

/// Path to the LND admin macaroon copied out by `scripts/e2e-up.sh`.
pub fn lnd_macaroon() -> PathBuf {
    std::env::var("PAYMENTS_RS_E2E_LND_MACAROON")
        .unwrap_or_else(|_| {
            "/tmp/payments-rs-e2e-lnd/data/chain/bitcoin/regtest/admin.macaroon".to_string()
        })
        .into()
}

/// Run `bitcoin-cli` inside the bitcoind container against the `e2e` wallet.
async fn bitcoin_cli(args: &[&str]) -> anyhow::Result<String> {
    let mut full = vec![
        "compose",
        "-f",
        COMPOSE_FILE,
        "exec",
        "-T",
        "bitcoind",
        "bitcoin-cli",
        "-regtest",
        "-rpcuser=polaruser",
        "-rpcpassword=polarpass",
        "-rpcwallet=e2e",
    ];
    full.extend_from_slice(args);

    let out = tokio::process::Command::new("docker")
        .args(&full)
        .output()
        .await?;
    anyhow::ensure!(
        out.status.success(),
        "bitcoin-cli {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

/// Send `amount_btc` to `address` from the bitcoind wallet; returns the txid.
pub async fn send_to_address(address: &str, amount_btc: f64) -> anyhow::Result<String> {
    bitcoin_cli(&["sendtoaddress", address, &format!("{amount_btc:.8}")]).await
}

/// Mine `n` blocks to the bitcoind wallet, confirming pending transactions.
pub async fn mine_blocks(n: u32) -> anyhow::Result<()> {
    let addr = bitcoin_cli(&["getnewaddress"]).await?;
    bitcoin_cli(&["generatetoaddress", &n.to_string(), &addr]).await?;
    Ok(())
}
