#!/usr/bin/env bash
# e2e-up.sh — start the regtest bitcoind + lnd stack, fund it, and copy the
# LND credentials to a host path the integration tests can read.
#
# After it completes, run:
#   PAYMENTS_RS_E2E=1 cargo test --test e2e_onchain -- --ignored --nocapture
#
# Tear down with:
#   docker compose -f docker-compose.e2e.yaml down -v
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

COMPOSE_FILE="${COMPOSE_FILE:-docker-compose.e2e.yaml}"
CRED_DIR="${PAYMENTS_RS_E2E_LND_DIR:-/tmp/payments-rs-e2e-lnd}"
TIMEOUT="${TIMEOUT:-120}"

echo "=== Starting regtest stack ($COMPOSE_FILE) ==="
docker compose -f "$COMPOSE_FILE" up -d

BITCOIND=$(docker compose -f "$COMPOSE_FILE" ps -q bitcoind)
LND=$(docker compose -f "$COMPOSE_FILE" ps -q lnd)

BTC() { docker exec "$BITCOIND" bitcoin-cli -regtest -rpcuser=polaruser -rpcpassword=polarpass "$@"; }
LNC() { docker exec "$LND" lncli --network=regtest "$@"; }

echo "=== Waiting for lnd ==="
for i in $(seq 1 "$TIMEOUT"); do
    if LNC getinfo >/dev/null 2>&1; then echo "lnd ready after ${i}s"; break; fi
    if [[ "$i" -eq "$TIMEOUT" ]]; then echo "ERROR: lnd not ready"; exit 1; fi
    sleep 1
done

echo "=== Copying lnd credentials to $CRED_DIR ==="
mkdir -p "$CRED_DIR/data/chain/bitcoin/regtest"
docker cp "$LND":/root/.lnd/tls.cert "$CRED_DIR/tls.cert"
docker cp "$LND":/root/.lnd/data/chain/bitcoin/regtest/admin.macaroon \
    "$CRED_DIR/data/chain/bitcoin/regtest/admin.macaroon"

echo "=== Funding bitcoind wallet ==="
# The image may or may not have a default wallet; create/load defensively.
BTC createwallet e2e >/dev/null 2>&1 || BTC loadwallet e2e >/dev/null 2>&1 || true
BTC_ADDR=$(BTC -rpcwallet=e2e getnewaddress)
BTC -rpcwallet=e2e generatetoaddress 101 "$BTC_ADDR" >/dev/null
echo "bitcoind wallet funded (address $BTC_ADDR)"

cat <<EOF

=== Ready ===
Export these before running the tests:

  export PAYMENTS_RS_E2E=1
  export PAYMENTS_RS_E2E_LND_ADDRESS="https://localhost:10009"
  export PAYMENTS_RS_E2E_LND_CERT="$CRED_DIR/tls.cert"
  export PAYMENTS_RS_E2E_LND_MACAROON="$CRED_DIR/data/chain/bitcoin/regtest/admin.macaroon"

Then:

  cargo test --test e2e_onchain -- --ignored --nocapture
EOF
