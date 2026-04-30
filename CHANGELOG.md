# Changelog

All notable changes to the OyaShip smart contracts are documented here.

## [Unreleased]

### Added
- `get_user_deals(user)` — returns all deal IDs for a user (buyer or seller)
- `get_arbiter()` — returns the stored arbiter address
- `expire_deal(deal_id)` — permissionless expiry after deadline; refunds buyer
- Deadline validation in `create_deal` — rejects past deadlines
- Deadline enforcement in `mark_shipped` — seller cannot ship after deadline
- `SECURITY.md` — vulnerability disclosure policy
- `Makefile` — common build, test, and deploy commands

### Changed
- `create_deal` now accepts `token: Address` for real token transfers
- `confirm_received` transfers tokens to seller via `token::Client`
- `cancel_deal` refunds buyer via `token::Client`
- `resolve_dispute` transfers tokens to winning party
- Added `Expired` variant to `DealStatus`

### Added (infrastructure)
- GitHub Actions CI: build, test, clippy, format check
- `CONTRIBUTING.md`
- Improved `scripts/deploy.sh` with error handling and mainnet warning
