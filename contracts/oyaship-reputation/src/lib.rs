//! # OyaShip Reputation Contract
//!
//! Immutable, on-chain trust scoring for importers and suppliers.
//!
//! ## Scoring model
//!
//! Each completed trade increments `trades_completed` and adds `volume`.
//! Each dispute increments `disputes_raised` or `disputes_received`.
//! Disputes that the user *won* do not penalise them.
//!
//! ```
//! score = clamp(
//!     (trades_completed * 100)
//!     - (disputes_lost  * 30)
//!     - (disputes_abandoned * 10),
//!     0, 10_000               // 0–100.00, stored as basis points
//! )
//! ```
//!
//! Only whitelisted escrow contracts can record trade outcomes.
//! The admin manages the whitelist.

#![no_std]

use soroban_sdk::{
    contract, contractimpl, contracttype, contracterror,
    symbol_short, Address, BytesN, Env,
};

// ── TTL ───────────────────────────────────────────────────────────────────────
const INSTANCE_TTL: u32 = 518_400;
const PROFILE_TTL:  u32 = 6_307_200;

// ── Domain types ──────────────────────────────────────────────────────────────
#[contracttype]
#[derive(Clone, Debug, Default)]
pub struct TraderProfile {
    /// Total trades where this address was involved (buyer or seller).
    pub trades_completed:   u64,
    /// Disputes this user raised that were resolved in their favour.
    pub disputes_won:       u64,
    /// Disputes resolved against this user.
    pub disputes_lost:      u64,
    /// Disputes this user raised that were later abandoned/resolved for other party.
    pub disputes_abandoned: u64,
    /// Total token value traded (in XLM stroops equivalent, best-effort).
    pub volume_traded:      i128,
    /// Timestamp of first recorded trade.
    pub first_trade_at:     u64,
    /// Timestamp of most recent trade or dispute.
    pub last_active_at:     u64,
}

// ── Storage keys ──────────────────────────────────────────────────────────────
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    /// Whether an address is a whitelisted escrow contract.
    Escrow(Address),
    Profile(Address),
}

// ── Errors ────────────────────────────────────────────────────────────────────
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized     = 2,
    Unauthorized       = 3,
    NotFound           = 4,
}

// ── Contract ──────────────────────────────────────────────────────────────────
#[contract]
pub struct ReputationContract;

#[contractimpl]
impl ReputationContract {

    // ── Admin ─────────────────────────────────────────────────────────────────

    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().extend_ttl(INSTANCE_TTL, INSTANCE_TTL);
        Ok(())
    }

    pub fn upgrade(env: Env, admin: Address, new_wasm: BytesN<32>) -> Result<(), Error> {
        Self::require_admin(&env, &admin)?;
        env.deployer().update_current_contract_wasm(new_wasm);
        Ok(())
    }

    /// Whitelist an escrow contract so it can record trade outcomes.
    pub fn add_escrow(env: Env, admin: Address, escrow: Address) -> Result<(), Error> {
        Self::require_admin(&env, &admin)?;
        env.storage().persistent().set(&DataKey::Escrow(escrow.clone()), &true);
        env.storage().persistent().extend_ttl(&DataKey::Escrow(escrow.clone()), PROFILE_TTL, PROFILE_TTL);
        env.events().publish((symbol_short!("escrow"), symbol_short!("added")), (escrow,));
        Ok(())
    }

    pub fn remove_escrow(env: Env, admin: Address, escrow: Address) -> Result<(), Error> {
        Self::require_admin(&env, &admin)?;
        env.storage().persistent().set(&DataKey::Escrow(escrow.clone()), &false);
        Ok(())
    }

    // ── Recording (escrow-only) ───────────────────────────────────────────────

    /// Called by an escrow contract when a deal completes successfully.
    /// Both the buyer's and seller's profiles are updated.
    pub fn record_trade(
        env:    Env,
        escrow: Address,
        buyer:  Address,
        seller: Address,
        volume: i128,
    ) -> Result<(), Error> {
        Self::require_escrow(&env, &escrow)?;

        let now = env.ledger().timestamp();
        for trader in [&buyer, &seller] {
            let mut p = Self::load_profile(&env, trader);
            p.trades_completed += 1;
            p.volume_traded    += volume;
            p.last_active_at   = now;
            if p.first_trade_at == 0 { p.first_trade_at = now; }
            Self::save_profile(&env, trader, &p);
        }

        env.events().publish(
            (symbol_short!("trade"), symbol_short!("done")),
            (buyer, seller, volume),
        );
        Ok(())
    }

    /// Called by an escrow contract when a dispute is resolved.
    ///
    /// - `winner`: the address that received the disputed funds.
    /// - `loser` : the address whose claim was rejected.
    pub fn record_dispute_resolved(
        env:    Env,
        escrow: Address,
        winner: Address,
        loser:  Address,
    ) -> Result<(), Error> {
        Self::require_escrow(&env, &escrow)?;

        let now = env.ledger().timestamp();

        let mut wp = Self::load_profile(&env, &winner);
        wp.disputes_won   += 1;
        wp.last_active_at  = now;
        Self::save_profile(&env, &winner, &wp);

        let mut lp = Self::load_profile(&env, &loser);
        lp.disputes_lost  += 1;
        lp.last_active_at  = now;
        Self::save_profile(&env, &loser, &lp);

        env.events().publish(
            (symbol_short!("dispute"), symbol_short!("done")),
            (winner, loser),
        );
        Ok(())
    }

    /// Called when a dispute was raised but abandoned by the raiser.
    pub fn record_dispute_abandoned(
        env:    Env,
        escrow: Address,
        raiser: Address,
    ) -> Result<(), Error> {
        Self::require_escrow(&env, &escrow)?;

        let mut p = Self::load_profile(&env, &raiser);
        p.disputes_abandoned += 1;
        p.last_active_at      = env.ledger().timestamp();
        Self::save_profile(&env, &raiser, &p);

        Ok(())
    }

    // ── Views ──────────────────────────────────────────────────────────────────

    /// Returns the raw profile for an address.
    pub fn get_profile(env: Env, trader: Address) -> TraderProfile {
        Self::load_profile(&env, &trader)
    }

    /// Returns the computed reputation score in basis points (0–10_000).
    ///
    /// 10_000 = perfect (100.00%), 0 = extremely poor.
    pub fn get_score(env: Env, trader: Address) -> u64 {
        let p = Self::load_profile(&env, &trader);
        Self::compute_score(&p)
    }

    /// Returns (score, trades_completed, volume_traded, disputes_lost).
    pub fn get_summary(env: Env, trader: Address) -> (u64, u64, i128, u64) {
        let p = Self::load_profile(&env, &trader);
        (Self::compute_score(&p), p.trades_completed, p.volume_traded, p.disputes_lost)
    }

    pub fn is_whitelisted_escrow(env: Env, escrow: Address) -> bool {
        env.storage().persistent()
            .get(&DataKey::Escrow(escrow))
            .unwrap_or(false)
    }

    // ── Internal ───────────────────────────────────────────────────────────────

    fn compute_score(p: &TraderProfile) -> u64 {
        if p.trades_completed == 0 { return 5_000; } // neutral start

        let raw = (p.trades_completed * 100)
            .saturating_sub(p.disputes_lost * 30)
            .saturating_sub(p.disputes_abandoned * 10);

        // Normalise to 0–10_000 (basis points)
        let max = p.trades_completed * 100;
        if max == 0 { return 5_000; }
        ((raw * 10_000) / max).min(10_000)
    }

    fn load_profile(env: &Env, trader: &Address) -> TraderProfile {
        env.storage().persistent()
            .get(&DataKey::Profile(trader.clone()))
            .unwrap_or(TraderProfile {
                trades_completed:   0,
                disputes_won:       0,
                disputes_lost:      0,
                disputes_abandoned: 0,
                volume_traded:      0,
                first_trade_at:     0,
                last_active_at:     0,
            })
    }

    fn save_profile(env: &Env, trader: &Address, p: &TraderProfile) {
        env.storage().persistent().set(&DataKey::Profile(trader.clone()), p);
        env.storage().persistent().extend_ttl(&DataKey::Profile(trader.clone()), PROFILE_TTL, PROFILE_TTL);
    }

    fn require_admin(env: &Env, caller: &Address) -> Result<(), Error> {
        let admin: Address = env.storage().instance()
            .get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        if *caller != admin { return Err(Error::Unauthorized); }
        caller.require_auth();
        Ok(())
    }

    fn require_escrow(env: &Env, caller: &Address) -> Result<(), Error> {
        caller.require_auth();
        let ok: bool = env.storage().persistent()
            .get(&DataKey::Escrow(caller.clone())).unwrap_or(false);
        if !ok { Err(Error::Unauthorized) } else { Ok(()) }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env};

    fn setup(env: &Env) -> (ReputationContractClient, Address, Address) {
        env.mock_all_auths();
        let contract = env.register_contract(None, ReputationContract);
        let client   = ReputationContractClient::new(env, &contract);
        let admin    = Address::generate(env);
        let escrow   = Address::generate(env);
        client.initialize(&admin);
        client.add_escrow(&admin, &escrow);
        (client, admin, escrow)
    }

    #[test]
    fn test_neutral_score_before_any_trades() {
        let env = Env::default();
        let (c, _, _) = setup(&env);
        let trader = Address::generate(&env);
        assert_eq!(c.get_score(&trader), 5_000); // neutral
    }

    #[test]
    fn test_score_rises_with_successful_trades() {
        let env = Env::default();
        env.ledger().set_timestamp(1_000);
        let (c, _, escrow) = setup(&env);
        let buyer  = Address::generate(&env);
        let seller = Address::generate(&env);

        for _ in 0..10 {
            c.record_trade(&escrow, &buyer, &seller, &1_000_000);
        }

        assert_eq!(c.get_score(&seller), 10_000); // perfect
        assert_eq!(c.get_profile(&seller).trades_completed, 10);
    }

    #[test]
    fn test_score_drops_on_dispute_loss() {
        let env = Env::default();
        env.ledger().set_timestamp(1_000);
        let (c, _, escrow) = setup(&env);
        let buyer  = Address::generate(&env);
        let seller = Address::generate(&env);

        // 5 good trades then 1 dispute loss
        for _ in 0..5 {
            c.record_trade(&escrow, &buyer, &seller, &500_000);
        }
        c.record_dispute_resolved(&escrow, &buyer, &seller); // seller loses

        let score = c.get_score(&seller);
        assert!(score < 10_000);
        assert_eq!(c.get_profile(&seller).disputes_lost, 1);
    }

    #[test]
    fn test_dispute_winner_not_penalised() {
        let env = Env::default();
        env.ledger().set_timestamp(1_000);
        let (c, _, escrow) = setup(&env);
        let buyer  = Address::generate(&env);
        let seller = Address::generate(&env);

        c.record_trade(&escrow, &buyer, &seller, &1_000_000);
        c.record_dispute_resolved(&escrow, &buyer, &seller); // buyer wins

        assert_eq!(c.get_profile(&buyer).disputes_won, 1);
        assert_eq!(c.get_profile(&buyer).disputes_lost, 0);
    }

    #[test]
    fn test_volume_accumulates() {
        let env = Env::default();
        env.ledger().set_timestamp(1_000);
        let (c, _, escrow) = setup(&env);
        let buyer  = Address::generate(&env);
        let seller = Address::generate(&env);

        c.record_trade(&escrow, &buyer, &seller, &300_000);
        c.record_trade(&escrow, &buyer, &seller, &700_000);

        assert_eq!(c.get_profile(&seller).volume_traded, 1_000_000);
    }

    #[test]
    fn test_non_escrow_cannot_record() {
        let env = Env::default();
        env.mock_all_auths();
        let (c, _, _) = setup(&env);
        let faker  = Address::generate(&env);
        let buyer  = Address::generate(&env);
        let seller = Address::generate(&env);

        let result = c.try_record_trade(&faker, &buyer, &seller, &1_000);
        assert!(result.is_err());
    }

    #[test]
    fn test_summary_returns_correct_tuple() {
        let env = Env::default();
        env.ledger().set_timestamp(1_000);
        let (c, _, escrow) = setup(&env);
        let buyer  = Address::generate(&env);
        let seller = Address::generate(&env);

        c.record_trade(&escrow, &buyer, &seller, &2_000_000);
        let (score, trades, volume, disputes) = c.get_summary(&seller);
        assert_eq!(trades,   1);
        assert_eq!(volume,   2_000_000);
        assert_eq!(disputes, 0);
        assert_eq!(score,    10_000);
    }
}
