# Changelog

All notable changes to this project will be documented here.

## [Unreleased]

### Added
- `expire_deal` function — anyone can trigger expiry after deadline passes; refunds buyer automatically
- Deadline validation in `create_deal` — rejects deadlines in the past
- Deadline enforcement in `mark_shipped` — seller cannot ship after deadline

### Changed
- `create_deal` now accepts a `token: Address` parameter for real token transfers
- `confirm_received` now transfers tokens to seller via `token::Client`
- `cancel_deal` now refunds buyer via `token::Client`
- `resolve_dispute` now transfers tokens to the winning party
- Added `Expired` variant to `DealStatus`

### Added
- Contract events emitted on every state transition (`deal.created`, `deal.shipped`,
  `deal.done`, `deal.cancel`, `deal.dispute`, `deal.resolve`, `deal.expired`)
- Unit tests covering happy path, cancellation, dispute resolution (buyer/seller), and expiry
- GitHub Actions CI workflow — build, test, clippy, format check
- `CONTRIBUTING.md`
