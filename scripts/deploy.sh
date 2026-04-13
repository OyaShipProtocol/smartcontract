#!/bin/bash
# Deploy OyaShip Escrow to Stellar testnet
# Prerequisites: soroban CLI installed, identity configured
# Usage: ./scripts/deploy.sh

set -e

echo "Building contract..."
soroban contract build

echo "Deploying to testnet..."
CONTRACT_ID=$(soroban contract deploy \
  --wasm target/wasm32-unknown-unknown/release/oyaship_escrow.wasm \
  --network testnet \
  --source default)

echo "Contract deployed: $CONTRACT_ID"

echo "Initializing with arbiter..."
soroban contract invoke \
  --id "$CONTRACT_ID" \
  --network testnet \
  --source default \
  -- initialize \
  --arbiter "$(soroban keys address default)"

echo "Done. Contract ID: $CONTRACT_ID"
