//! # OyaShip Escrow Contract
//!
//! Trustless escrow for cross-border trade on Stellar/Soroban.
//!
//! ## Deal lifecycle
//! ```
//! Created → Shipped → Confirmed (funds → seller)
//!         ↓           ↓
//!      Cancelled   Disputed → Resolved (arbiter splits or awards)
//!         ↓
//!       Expired (deadline passed without shipment, funds → buyer)
//! ```
//!
//! ## Fee model
//! A configurable platform fee (in basis points, e.g. 50 = 0.5%) is
//! deducted from the seller's payout on `confirm_received`. Fees
//! accumulate in the contract and are swept by the admin.
//!
//! ## Storage TTL
//! All persistent deal records are extended every write to survive
//! the Stellar ledger's expiry window (~1 year).

#![no_std]

use soroban_sdk::{
    contract, contractimpl, contracttype, contracterror,
    symbol_short, token, Address, BytesN, Env, String,
};

// ── TTL constants (ledgers; ~5 s each on mainnet) ─────────────────────────────
const INSTANCE_TTL: u32 = 518_400;    // ~30 days
const DEAL_TTL:     u32 = 6_307_200;  // ~1 year

// ── Domain types ──────────────────────────────────────────────────────────────
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DealStatus {
    Created,
    Shipped,
    Confirmed,
    Disputed,
    Resolved,
    Cancelled,
    Expired,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Deal {
    pub id:          u64,
    pub buyer:       Address,
    pub seller:      Address,
    pub token:       Address,
    /// Gross amount locked in escrow (before fees).
    pub amount:      i128,
    pub description: String,
    pub status:      DealStatus,
    pub created_at:  u64,
    /// Unix timestamp after which an unshipped deal can be expired.
    pub deadline:    u64,
}

// ── Storage keys ──────────────────────────────────────────────────────────────
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Deal(u64),
    NextDealId,
    Admin,
    Arbiter,
    /// Platform fee in basis points (0–1000 = 0–10%).
    FeeBps,
    /// Accumulated unclaimed platform fees per token.
    FeeBalance(Address),
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
    InvalidAmount      = 5,
    SelfDeal           = 6,
    DeadlineInPast     = 7,
    InvalidStatus      = 8,
    DeadlineNotPassed  = 9,
    DeadlinePassed     = 10,
    InvalidFeeBps      = 11,
    NoFeesAccumulated  = 12,
}

// ── Contract ──────────────────────────────────────────────────────────────────
#[contract]
pub struct EscrowContract;

#[contractimpl]
impl EscrowContract {

    // ── Admin setup ───────────────────────────────────────────────────────────

    pub fn initialize(env: Env, admin: Address, arbiter: Address, fee_bps: u32) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        if fee_bps > 1000 { return Err(Error::InvalidFeeBps); } // max 10%
        admin.require_auth();

        env.storage().instance().set(&DataKey::Admin,     &admin);
        env.storage().instance().set(&DataKey::Arbiter,   &arbiter);
        env.storage().instance().set(&DataKey::FeeBps,    &fee_bps);
        env.storage().instance().set(&DataKey::NextDealId, &0u64);
        env.storage().instance().extend_ttl(INSTANCE_TTL, INSTANCE_TTL);
        Ok(())
    }

    /// Update the platform fee (admin only, max 10%).
    pub fn set_fee(env: Env, admin: Address, fee_bps: u32) -> Result<(), Error> {
        Self::require_admin(&env, &admin)?;
        if fee_bps > 1000 { return Err(Error::InvalidFeeBps); }
        env.storage().instance().set(&DataKey::FeeBps, &fee_bps);
        env.storage().instance().extend_ttl(INSTANCE_TTL, INSTANCE_TTL);
        Ok(())
    }

    /// Rotate the arbiter address (admin only).
    pub fn set_arbiter(env: Env, admin: Address, new_arbiter: Address) -> Result<(), Error> {
        Self::require_admin(&env, &admin)?;
        env.storage().instance().set(&DataKey::Arbiter, &new_arbiter);
        env.storage().instance().extend_ttl(INSTANCE_TTL, INSTANCE_TTL);
        Ok(())
    }

    /// Upgrade the contract WASM (admin only).
    pub fn upgrade(env: Env, admin: Address, new_wasm: BytesN<32>) -> Result<(), Error> {
        Self::require_admin(&env, &admin)?;
        env.deployer().update_current_contract_wasm(new_wasm);
        Ok(())
    }

    /// Sweep accumulated platform fees to a treasury address (admin only).
    pub fn sweep_fees(env: Env, admin: Address, token: Address, to: Address) -> Result<i128, Error> {
        Self::require_admin(&env, &admin)?;

        let balance: i128 = env.storage().instance()
            .get(&DataKey::FeeBalance(token.clone())).unwrap_or(0);
        if balance == 0 { return Err(Error::NoFeesAccumulated); }

        token::Client::new(&env, &token)
            .transfer(&env.current_contract_address(), &to, &balance);

        env.storage().instance().set(&DataKey::FeeBalance(token.clone()), &0i128);
        env.events().publish(
            (symbol_short!("fees"), symbol_short!("swept")),
            (token, to, balance),
        );
        Ok(balance)
    }

    // ── Deal lifecycle ────────────────────────────────────────────────────────

    /// Buyer creates a deal and locks funds in escrow.
    ///
    /// `deadline` is a Unix timestamp — seller must mark shipment before this.
    pub fn create_deal(
        env:         Env,
        buyer:       Address,
        seller:      Address,
        token:       Address,
        amount:      i128,
        description: String,
        deadline:    u64,
    ) -> Result<u64, Error> {
        buyer.require_auth();
        if amount <= 0                             { return Err(Error::InvalidAmount);   }
        if buyer == seller                         { return Err(Error::SelfDeal);        }
        if deadline <= env.ledger().timestamp()   { return Err(Error::DeadlineInPast);  }

        token::Client::new(&env, &token)
            .transfer(&buyer, &env.current_contract_address(), &amount);

        env.storage().instance().extend_ttl(INSTANCE_TTL, INSTANCE_TTL);
        let id: u64 = env.storage().instance().get(&DataKey::NextDealId).unwrap_or(0);
        let new_id  = id + 1;

        let deal = Deal {
            id:          new_id,
            buyer:       buyer.clone(),
            seller:      seller.clone(),
            token,
            amount,
            description,
            status:      DealStatus::Created,
            created_at:  env.ledger().timestamp(),
            deadline,
        };

        env.storage().persistent().set(&DataKey::Deal(new_id), &deal);
        env.storage().persistent().extend_ttl(&DataKey::Deal(new_id), DEAL_TTL, DEAL_TTL);
        env.storage().instance().set(&DataKey::NextDealId, &new_id);

        env.events().publish(
            (symbol_short!("deal"), symbol_short!("created")),
            (new_id, buyer, seller, amount),
        );

        Ok(new_id)
    }

    /// Seller marks goods as shipped. Must happen before `deadline`.
    pub fn mark_shipped(env: Env, seller: Address, deal_id: u64) -> Result<(), Error> {
        seller.require_auth();
        let mut deal = Self::load_deal(&env, deal_id)?;

        if deal.seller != seller                        { return Err(Error::Unauthorized);    }
        if deal.status != DealStatus::Created           { return Err(Error::InvalidStatus);   }
        if env.ledger().timestamp() > deal.deadline     { return Err(Error::DeadlinePassed);  }

        deal.status = DealStatus::Shipped;
        Self::save_deal(&env, &deal);

        env.events().publish(
            (symbol_short!("deal"), symbol_short!("shipped")),
            (deal_id, seller),
        );
        Ok(())
    }

    /// Buyer confirms receipt — releases funds to seller (minus platform fee).
    pub fn confirm_received(env: Env, buyer: Address, deal_id: u64) -> Result<i128, Error> {
        buyer.require_auth();
        let mut deal = Self::load_deal(&env, deal_id)?;

        if deal.buyer != buyer             { return Err(Error::Unauthorized);  }
        if deal.status != DealStatus::Shipped { return Err(Error::InvalidStatus); }

        let fee_bps: u32 = env.storage().instance()
            .get(&DataKey::FeeBps).unwrap_or(0);
        let fee    = (deal.amount * fee_bps as i128) / 10_000;
        let payout = deal.amount - fee;

        // Transfer seller payout
        token::Client::new(&env, &deal.token)
            .transfer(&env.current_contract_address(), &deal.seller, &payout);

        // Accumulate platform fee
        if fee > 0 {
            let prev_fee: i128 = env.storage().instance()
                .get(&DataKey::FeeBalance(deal.token.clone())).unwrap_or(0);
            env.storage().instance()
                .set(&DataKey::FeeBalance(deal.token.clone()), &(prev_fee + fee));
        }

        deal.status = DealStatus::Confirmed;
        Self::save_deal(&env, &deal);

        env.events().publish(
            (symbol_short!("deal"), symbol_short!("done")),
            (deal_id, deal.seller, payout, fee),
        );
        Ok(payout)
    }

    /// Buyer cancels before shipment — full refund.
    pub fn cancel_deal(env: Env, buyer: Address, deal_id: u64) -> Result<(), Error> {
        buyer.require_auth();
        let mut deal = Self::load_deal(&env, deal_id)?;

        if deal.buyer != buyer                  { return Err(Error::Unauthorized);  }
        if deal.status != DealStatus::Created   { return Err(Error::InvalidStatus); }

        token::Client::new(&env, &deal.token)
            .transfer(&env.current_contract_address(), &deal.buyer, &deal.amount);

        deal.status = DealStatus::Cancelled;
        Self::save_deal(&env, &deal);

        env.events().publish(
            (symbol_short!("deal"), symbol_short!("cancel")),
            (deal_id, deal.buyer, deal.amount),
        );
        Ok(())
    }

    /// Anyone can expire a deal whose deadline has passed without shipment.
    /// Full refund goes to the buyer.
    pub fn expire_deal(env: Env, deal_id: u64) -> Result<(), Error> {
        let mut deal = Self::load_deal(&env, deal_id)?;

        if deal.status != DealStatus::Created        { return Err(Error::InvalidStatus);     }
        if env.ledger().timestamp() <= deal.deadline { return Err(Error::DeadlineNotPassed); }

        token::Client::new(&env, &deal.token)
            .transfer(&env.current_contract_address(), &deal.buyer, &deal.amount);

        deal.status = DealStatus::Expired;
        Self::save_deal(&env, &deal);

        env.events().publish(
            (symbol_short!("deal"), symbol_short!("expired")),
            (deal_id, deal.buyer, deal.amount),
        );
        Ok(())
    }

    /// Buyer or seller raises a dispute. Locks the deal for arbiter review.
    pub fn raise_dispute(env: Env, caller: Address, deal_id: u64) -> Result<(), Error> {
        caller.require_auth();
        let mut deal = Self::load_deal(&env, deal_id)?;

        if caller != deal.buyer && caller != deal.seller { return Err(Error::Unauthorized); }
        if deal.status != DealStatus::Created && deal.status != DealStatus::Shipped {
            return Err(Error::InvalidStatus);
        }

        deal.status = DealStatus::Disputed;
        Self::save_deal(&env, &deal);

        env.events().publish(
            (symbol_short!("deal"), symbol_short!("dispute")),
            (deal_id, caller),
        );
        Ok(())
    }

    /// Arbiter resolves a dispute by splitting or fully awarding to one party.
    ///
    /// `buyer_bps`: basis points of the locked amount awarded to the buyer
    /// (e.g. 5000 = 50/50 split, 10000 = full refund to buyer, 0 = full to seller).
    pub fn resolve_dispute(
        env:       Env,
        arbiter:   Address,
        deal_id:   u64,
        buyer_bps: u32,
    ) -> Result<(), Error> {
        arbiter.require_auth();
        let stored: Address = env.storage().instance()
            .get(&DataKey::Arbiter).ok_or(Error::NotInitialized)?;
        if arbiter != stored              { return Err(Error::Unauthorized);  }
        if buyer_bps > 10_000            { return Err(Error::InvalidAmount);  }

        let mut deal = Self::load_deal(&env, deal_id)?;
        if deal.status != DealStatus::Disputed { return Err(Error::InvalidStatus); }

        let buyer_amount  = (deal.amount * buyer_bps as i128) / 10_000;
        let seller_amount = deal.amount - buyer_amount;

        let tc = token::Client::new(&env, &deal.token);
        if buyer_amount > 0 {
            tc.transfer(&env.current_contract_address(), &deal.buyer, &buyer_amount);
        }
        if seller_amount > 0 {
            tc.transfer(&env.current_contract_address(), &deal.seller, &seller_amount);
        }

        deal.status = DealStatus::Resolved;
        Self::save_deal(&env, &deal);

        env.events().publish(
            (symbol_short!("deal"), symbol_short!("resolve")),
            (deal_id, buyer_amount, seller_amount),
        );
        Ok(())
    }

    // ── Views ──────────────────────────────────────────────────────────────────

    pub fn get_deal(env: Env, deal_id: u64) -> Result<Deal, Error> {
        Self::load_deal(&env, deal_id)
    }

    pub fn get_fee_bps(env: Env) -> u32 {
        env.storage().instance().get(&DataKey::FeeBps).unwrap_or(0)
    }

    pub fn get_accumulated_fees(env: Env, token: Address) -> i128 {
        env.storage().instance()
            .get(&DataKey::FeeBalance(token)).unwrap_or(0)
    }

    pub fn get_arbiter(env: Env) -> Result<Address, Error> {
        env.storage().instance().get(&DataKey::Arbiter).ok_or(Error::NotInitialized)
    }

    pub fn get_deal_count(env: Env) -> u64 {
        env.storage().instance().get(&DataKey::NextDealId).unwrap_or(0)
    }

    // ── Internal ───────────────────────────────────────────────────────────────

    fn load_deal(env: &Env, id: u64) -> Result<Deal, Error> {
        env.storage().persistent().get(&DataKey::Deal(id)).ok_or(Error::NotFound)
    }

    fn save_deal(env: &Env, deal: &Deal) {
        env.storage().persistent().set(&DataKey::Deal(deal.id), deal);
        env.storage().persistent().extend_ttl(&DataKey::Deal(deal.id), DEAL_TTL, DEAL_TTL);
    }

    fn require_admin(env: &Env, caller: &Address) -> Result<(), Error> {
        let admin: Address = env.storage().instance()
            .get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        if *caller != admin { return Err(Error::Unauthorized); }
        caller.require_auth();
        Ok(())
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::{Client as TokenClient, StellarAssetClient},
        Env, String,
    };

    const FEE_BPS: u32 = 100; // 1%

    fn setup(env: &Env) -> (EscrowContractClient, Address, Address, Address, Address, Address) {
        env.mock_all_auths();
        let contract = env.register_contract(None, EscrowContract);
        let client   = EscrowContractClient::new(env, &contract);
        let admin    = Address::generate(env);
        let arbiter  = Address::generate(env);
        let buyer    = Address::generate(env);
        let seller   = Address::generate(env);
        let tok_admin = Address::generate(env);
        let token    = env.register_stellar_asset_contract(tok_admin.clone());
        StellarAssetClient::new(env, &token).mint(&buyer, &10_000_000_000);
        client.initialize(&admin, &arbiter, &FEE_BPS);
        env.ledger().set_timestamp(1_000);
        (client, admin, arbiter, buyer, seller, token)
    }

    #[test]
    fn test_happy_path_with_fee() {
        let env = Env::default();
        let (c, _, _, buyer, seller, token) = setup(&env);
        let amount = 1_000_000_000_i128;
        let id = c.create_deal(&buyer, &seller, &token, &amount, &String::from_str(&env, "500 units"), &5_000);
        c.mark_shipped(&seller, &id);
        let payout = c.confirm_received(&buyer, &id);

        let fee    = amount / 100; // 1%
        assert_eq!(payout, amount - fee);
        assert_eq!(TokenClient::new(&env, &token).balance(&seller), payout);
        assert_eq!(c.get_accumulated_fees(&token), fee);
        assert_eq!(c.get_deal(&id).status, DealStatus::Confirmed);
    }

    #[test]
    fn test_sweep_fees() {
        let env = Env::default();
        let (c, admin, _, buyer, seller, token) = setup(&env);
        let amount = 1_000_000_000_i128;
        let id = c.create_deal(&buyer, &seller, &token, &amount, &String::from_str(&env, "goods"), &5_000);
        c.mark_shipped(&seller, &id);
        c.confirm_received(&buyer, &id);

        let treasury = Address::generate(&env);
        let swept = c.sweep_fees(&admin, &token, &treasury);
        assert_eq!(swept, amount / 100);
        assert_eq!(c.get_accumulated_fees(&token), 0);
        assert_eq!(TokenClient::new(&env, &token).balance(&treasury), amount / 100);
    }

    #[test]
    fn test_cancel_full_refund() {
        let env = Env::default();
        let (c, _, _, buyer, seller, token) = setup(&env);
        let id = c.create_deal(&buyer, &seller, &token, &500_000_000, &String::from_str(&env, "x"), &5_000);
        c.cancel_deal(&buyer, &id);
        assert_eq!(TokenClient::new(&env, &token).balance(&buyer), 10_000_000_000);
    }

    #[test]
    fn test_expire_refunds_buyer() {
        let env = Env::default();
        let (c, _, _, buyer, seller, token) = setup(&env);
        let id = c.create_deal(&buyer, &seller, &token, &500_000_000, &String::from_str(&env, "x"), &3_000);
        env.ledger().set_timestamp(4_000);
        c.expire_deal(&id);
        assert_eq!(c.get_deal(&id).status, DealStatus::Expired);
        assert_eq!(TokenClient::new(&env, &token).balance(&buyer), 10_000_000_000);
    }

    #[test]
    fn test_dispute_50_50_split() {
        let env = Env::default();
        let (c, _, arbiter, buyer, seller, token) = setup(&env);
        let amount = 1_000_000_000_i128;
        let id = c.create_deal(&buyer, &seller, &token, &amount, &String::from_str(&env, "disputed"), &5_000);
        c.mark_shipped(&seller, &id);
        c.raise_dispute(&buyer, &id);
        c.resolve_dispute(&arbiter, &id, &5000); // 50/50

        assert_eq!(TokenClient::new(&env, &token).balance(&buyer),  9_500_000_000 + amount / 2);
        assert_eq!(TokenClient::new(&env, &token).balance(&seller), amount / 2);
    }

    #[test]
    fn test_dispute_full_refund_to_buyer() {
        let env = Env::default();
        let (c, _, arbiter, buyer, seller, token) = setup(&env);
        let amount = 1_000_000_000_i128;
        let id = c.create_deal(&buyer, &seller, &token, &amount, &String::from_str(&env, "bad seller"), &5_000);
        c.raise_dispute(&buyer, &id);
        c.resolve_dispute(&arbiter, &id, &10000);
        assert_eq!(TokenClient::new(&env, &token).balance(&buyer), 10_000_000_000);
    }

    #[test]
    fn test_self_deal_rejected() {
        let env = Env::default();
        let (c, _, _, buyer, _, token) = setup(&env);
        let result = c.try_create_deal(&buyer, &buyer, &token, &100, &String::from_str(&env, "x"), &5_000);
        assert!(result.is_err());
    }

    #[test]
    fn test_mark_shipped_after_deadline_rejected() {
        let env = Env::default();
        let (c, _, _, buyer, seller, token) = setup(&env);
        let id = c.create_deal(&buyer, &seller, &token, &100_000, &String::from_str(&env, "x"), &2_000);
        env.ledger().set_timestamp(3_000);
        assert!(c.try_mark_shipped(&seller, &id).is_err());
    }

    #[test]
    fn test_set_fee_capped_at_10_pct() {
        let env = Env::default();
        let (c, admin, _, _, _, _) = setup(&env);
        assert!(c.try_set_fee(&admin, &1001).is_err()); // >10% rejected
        c.set_fee(&admin, &500); // 5% ok
        assert_eq!(c.get_fee_bps(), 500);
    }
}
