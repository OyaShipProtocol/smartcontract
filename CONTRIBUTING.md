# Contributing to OyaShip Smart Contracts

Thank you for your interest in contributing!

## Development Setup

1. Install Rust: https://rustup.rs
2. Add the WASM target: `rustup target add wasm32-unknown-unknown`
3. Install Soroban CLI: https://soroban.stellar.org/docs/getting-started/setup

## Build

```bash
soroban contract build
```

## Tests

```bash
cargo test
```

## Pull Request Process

1. Fork the repo and branch off `main`: `git checkout -b feat/your-feature`
2. Make your changes with clear, descriptive commits
3. Ensure `cargo test` passes and `cargo clippy` produces no warnings
4. Open a PR against `main` and fill in the PR template

## Commit Message Format

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
feat: add multi-token support
fix: prevent zero-amount deals
chore: update soroban-sdk to 21.8
test: add dispute resolution tests
docs: update README with new functions
```

## Code Style

- Run `cargo fmt` before committing
- All public state-mutating functions must call `require_auth()` on the authorized party
- Add a test for every new function
- No `unwrap()` in production code paths
