# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| main    | ✅        |

## Reporting a Vulnerability

If you discover a security vulnerability in the OyaShip smart contracts or backend,
please **do not open a public GitHub issue**.

Instead, email the maintainers directly at: **security@oyaship.app**

Please include:
- A description of the vulnerability
- Steps to reproduce
- Potential impact
- Any suggested fixes (optional)

We aim to respond within **48 hours** and will keep you updated on the fix timeline.

## Scope

The following are in scope:
- `contracts/oyaship-escrow` — Soroban escrow contract
- `backend/` — Node.js API
- Any logic that handles user funds or authentication

## Out of Scope

- UI cosmetic issues
- Rate limiting on public read endpoints
- Issues requiring physical access to a user's device
