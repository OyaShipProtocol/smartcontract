#!/bin/bash
# Deploy OyaShip Escrow to Stellar testnet or mainnet
# Usage: ./scripts/deploy.sh [testnet|mainnet]
set -euo pipefail

NETWORK="${1:-testnet}"

if [[ "$NETWORK" != "testnet" && "$NETWORK" != "mainnet" ]]; then
  echo "Usage: $0 [testnet|mainnet]" >&2
  exit 1
fi

if [[ "$NETWORK" == "mainnet" ]]; then
  echo "⚠  WARNING: deploying to MAINNET. Press Ctrl+C within 5 seconds to abort."
  sleep 5
fi

echo "▸ Checking soroban CLI..."
if ! command -v soroban &>/dev/null; then
  echo "ERROR: soroban CLI not found. Install it from https://soroban.stellar.org/docs/getting-started/setup" >&2
  exit 1
fi

echo "▸ Building contracts..."
soroban contract build

WASM="target/wasm32-unknown-unknown/release/oyaship_escrow.wasm"
if [[ ! -f "$WASM" ]]; then
  echo "ERROR: WASM file not found at $WASM" >&2
  exit 1
fi

echo "▸ Deploying to $NETWORK..."
CONTRACT_ID=$(soroban contract deploy   --wasm "$WASM"   --network "$NETWORK"   --source default)

echo "▸ Contract deployed: $CONTRACT_ID"

echo "▸ Initializing with arbiter..."
ARBITER=$(soroban keys address default)
soroban contract invoke   --id "$CONTRACT_ID"   --network "$NETWORK"   --source default   -- initialize   --arbiter "$ARBITER"

echo ""
echo "✓ Done."
echo "  Network:     $NETWORK"
echo "  Contract ID: $CONTRACT_ID"
echo "  Arbiter:     $ARBITER"
