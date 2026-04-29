.PHONY: build test fmt lint clean deploy-testnet deploy-mainnet

build:
	soroban contract build

test:
	cargo test --all

fmt:
	cargo fmt --all

lint:
	cargo clippy --all -- -D warnings

clean:
	cargo clean

deploy-testnet:
	./scripts/deploy.sh testnet

deploy-mainnet:
	@echo "WARNING: deploying to mainnet"
	./scripts/deploy.sh mainnet
