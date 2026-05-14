//! # OyaShip Marketplace Contract
//!
//! On-chain product listing registry for the OyaShip cross-border trade platform.
//!
//! ## What lives on-chain
//! - Product listings: name, category, price, token accepted, stock, MOQ
//! - Verified seller badges (admin-granted)
//! - Listing ownership and active/inactive status
//!
//! ## What stays off-chain
//! - Images, long descriptions, chat messages, shipping details
//!   (stored in the OyaShip backend and anchored via `metadata_hash`)

#![no_std]

use soroban_sdk::{
    contract, contractimpl, contracttype, contracterror,
    symbol_short, Address, BytesN, Env, String, Vec,
};

// ── TTL ───────────────────────────────────────────────────────────────────────
const INSTANCE_TTL:  u32 = 518_400;   // ~30 days
const LISTING_TTL:   u32 = 6_307_200; // ~1 year

// ── Domain types ──────────────────────────────────────────────────────────────
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Category {
    Textiles,
    Electronics,
    Agriculture,
    Machinery,
    Chemicals,
    FoodBeverage,
    Furniture,
    Automotive,
    Other,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Listing {
    pub id:            u64,
    pub seller:        Address,
    /// Short product name (max ~50 chars enforced off-chain).
    pub name:          String,
    pub category:      Category,
    /// Price per unit in the accepted token's base unit (e.g. stroops for XLM).
    pub price_per_unit: i128,
    /// Stellar asset accepted as payment.
    pub token:         Address,
    /// Units available.
    pub stock:         u32,
    /// Minimum order quantity.
    pub moq:           u32,
    /// IPFS/SHA-256 hash of off-chain metadata (images, long desc, certs).
    pub metadata_hash: BytesN<32>,
    pub active:        bool,
    pub created_at:    u64,
    pub updated_at:    u64,
}

// ── Storage keys ──────────────────────────────────────────────────────────────
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    NextListingId,
    Listing(u64),
    SellerListings(Address),
    VerifiedSeller(Address),
}

// ── Errors ────────────────────────────────────────────────────────────────────
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized  = 1,
    NotInitialized      = 2,
    Unauthorized        = 3,
    NotFound            = 4,
    InvalidPrice        = 5,
    InvalidStock        = 6,
    InvalidMoq          = 7,
    ListingInactive     = 8,
    InsufficientStock   = 9,
}

// ── Contract ──────────────────────────────────────────────────────────────────
#[contract]
pub struct MarketplaceContract;

#[contractimpl]
impl MarketplaceContract {

    // ── Admin ─────────────────────────────────────────────────────────────────

    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::NextListingId, &0u64);
        env.storage().instance().extend_ttl(INSTANCE_TTL, INSTANCE_TTL);
        Ok(())
    }

    /// Grant or revoke a verified badge for a seller (admin only).
    pub fn set_verified(env: Env, admin: Address, seller: Address, verified: bool) -> Result<(), Error> {
        Self::require_admin(&env, &admin)?;
        env.storage().persistent().set(&DataKey::VerifiedSeller(seller.clone()), &verified);
        env.storage().persistent().extend_ttl(&DataKey::VerifiedSeller(seller.clone()), LISTING_TTL, LISTING_TTL);
        env.events().publish(
            (symbol_short!("seller"), symbol_short!("verify")),
            (seller, verified),
        );
        Ok(())
    }

    // ── Seller actions ────────────────────────────────────────────────────────

    /// Create a new product listing.
    pub fn create_listing(
        env:           Env,
        seller:        Address,
        name:          String,
        category:      Category,
        price_per_unit: i128,
        token:         Address,
        stock:         u32,
        moq:           u32,
        metadata_hash: BytesN<32>,
    ) -> Result<u64, Error> {
        seller.require_auth();
        if price_per_unit <= 0 { return Err(Error::InvalidPrice); }
        if stock == 0         { return Err(Error::InvalidStock); }
        if moq == 0 || moq > stock { return Err(Error::InvalidMoq); }

        env.storage().instance().extend_ttl(INSTANCE_TTL, INSTANCE_TTL);
        let id: u64 = env.storage().instance().get(&DataKey::NextListingId).unwrap_or(0) + 1;
        env.storage().instance().set(&DataKey::NextListingId, &id);

        let now = env.ledger().timestamp();
        let listing = Listing {
            id,
            seller: seller.clone(),
            name: name.clone(),
            category,
            price_per_unit,
            token,
            stock,
            moq,
            metadata_hash,
            active:     true,
            created_at: now,
            updated_at: now,
        };

        env.storage().persistent().set(&DataKey::Listing(id), &listing);
        env.storage().persistent().extend_ttl(&DataKey::Listing(id), LISTING_TTL, LISTING_TTL);

        // Track this listing under the seller's portfolio
        let mut seller_listings: Vec<u64> = env.storage().persistent()
            .get(&DataKey::SellerListings(seller.clone()))
            .unwrap_or(Vec::new(&env));
        seller_listings.push_back(id);
        env.storage().persistent().set(&DataKey::SellerListings(seller.clone()), &seller_listings);
        env.storage().persistent().extend_ttl(&DataKey::SellerListings(seller.clone()), LISTING_TTL, LISTING_TTL);

        env.events().publish(
            (symbol_short!("listing"), symbol_short!("created")),
            (id, seller, name),
        );

        Ok(id)
    }

    /// Update price, stock, and/or metadata hash.
    pub fn update_listing(
        env:            Env,
        seller:         Address,
        listing_id:     u64,
        price_per_unit: i128,
        stock:          u32,
        metadata_hash:  BytesN<32>,
    ) -> Result<(), Error> {
        seller.require_auth();
        let mut listing = Self::load_listing(&env, listing_id)?;
        if listing.seller != seller { return Err(Error::Unauthorized); }
        if price_per_unit <= 0     { return Err(Error::InvalidPrice);  }

        listing.price_per_unit = price_per_unit;
        listing.stock          = stock;
        listing.active         = stock > 0;
        listing.metadata_hash  = metadata_hash;
        listing.updated_at     = env.ledger().timestamp();
        Self::save_listing(&env, &listing);

        env.events().publish(
            (symbol_short!("listing"), symbol_short!("updated")),
            (listing_id, seller, stock),
        );
        Ok(())
    }

    /// Deactivate a listing (seller only). Does not delete; remains auditable.
    pub fn delist(env: Env, seller: Address, listing_id: u64) -> Result<(), Error> {
        seller.require_auth();
        let mut listing = Self::load_listing(&env, listing_id)?;
        if listing.seller != seller { return Err(Error::Unauthorized); }

        listing.active     = false;
        listing.updated_at = env.ledger().timestamp();
        Self::save_listing(&env, &listing);

        env.events().publish(
            (symbol_short!("listing"), symbol_short!("delisted")),
            (listing_id, seller),
        );
        Ok(())
    }

    /// Called by the escrow contract when a deal is confirmed —
    /// decrements stock. Returns remaining stock.
    pub fn deduct_stock(
        env:        Env,
        caller:     Address,
        listing_id: u64,
        quantity:   u32,
    ) -> Result<u32, Error> {
        caller.require_auth();
        let mut listing = Self::load_listing(&env, listing_id)?;
        if !listing.active               { return Err(Error::ListingInactive);    }
        if quantity > listing.stock      { return Err(Error::InsufficientStock);  }

        listing.stock -= quantity;
        if listing.stock == 0 { listing.active = false; }
        listing.updated_at = env.ledger().timestamp();
        Self::save_listing(&env, &listing);

        Ok(listing.stock)
    }

    // ── Views ──────────────────────────────────────────────────────────────────

    pub fn get_listing(env: Env, listing_id: u64) -> Result<Listing, Error> {
        Self::load_listing(&env, listing_id)
    }

    pub fn get_seller_listings(env: Env, seller: Address) -> Vec<u64> {
        env.storage().persistent()
            .get(&DataKey::SellerListings(seller))
            .unwrap_or(Vec::new(&env))
    }

    pub fn is_verified(env: Env, seller: Address) -> bool {
        env.storage().persistent()
            .get(&DataKey::VerifiedSeller(seller))
            .unwrap_or(false)
    }

    pub fn get_listing_count(env: Env) -> u64 {
        env.storage().instance().get(&DataKey::NextListingId).unwrap_or(0)
    }

    // ── Internal ───────────────────────────────────────────────────────────────

    fn load_listing(env: &Env, id: u64) -> Result<Listing, Error> {
        env.storage().persistent().get(&DataKey::Listing(id)).ok_or(Error::NotFound)
    }

    fn save_listing(env: &Env, listing: &Listing) {
        env.storage().persistent().set(&DataKey::Listing(listing.id), listing);
        env.storage().persistent().extend_ttl(&DataKey::Listing(listing.id), LISTING_TTL, LISTING_TTL);
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
    use soroban_sdk::{testutils::Address as _, Env, String};

    fn dummy_hash(env: &Env) -> BytesN<32> {
        BytesN::from_array(env, &[1u8; 32])
    }

    fn setup(env: &Env) -> (MarketplaceContractClient, Address, Address, Address) {
        env.mock_all_auths();
        let contract = env.register_contract(None, MarketplaceContract);
        let client   = MarketplaceContractClient::new(env, &contract);
        let admin    = Address::generate(env);
        let seller   = Address::generate(env);
        let token    = Address::generate(env);
        client.initialize(&admin);
        (client, admin, seller, token)
    }

    #[test]
    fn test_create_and_get_listing() {
        let env = Env::default();
        env.ledger().set_timestamp(1_000);
        let (c, _, seller, token) = setup(&env);

        let id = c.create_listing(
            &seller,
            &String::from_str(&env, "Cotton Fabric"),
            &Category::Textiles,
            &500_000_000,
            &token,
            &1_000,
            &100,
            &dummy_hash(&env),
        );

        let listing = c.get_listing(&id);
        assert_eq!(listing.stock, 1_000);
        assert_eq!(listing.moq, 100);
        assert!(listing.active);
        assert_eq!(listing.seller, seller);
    }

    #[test]
    fn test_update_listing() {
        let env = Env::default();
        env.ledger().set_timestamp(1_000);
        let (c, _, seller, token) = setup(&env);

        let id = c.create_listing(&seller, &String::from_str(&env, "Phones"),
            &Category::Electronics, &200_000, &token, &500, &10, &dummy_hash(&env));

        c.update_listing(&seller, &id, &250_000, &400, &dummy_hash(&env));
        let listing = c.get_listing(&id);
        assert_eq!(listing.price_per_unit, 250_000);
        assert_eq!(listing.stock, 400);
    }

    #[test]
    fn test_delist() {
        let env = Env::default();
        env.ledger().set_timestamp(1_000);
        let (c, _, seller, token) = setup(&env);

        let id = c.create_listing(&seller, &String::from_str(&env, "Machinery"),
            &Category::Machinery, &10_000_000, &token, &50, &1, &dummy_hash(&env));

        c.delist(&seller, &id);
        assert!(!c.get_listing(&id).active);
    }

    #[test]
    fn test_deduct_stock_auto_deactivates() {
        let env = Env::default();
        env.ledger().set_timestamp(1_000);
        let (c, _, seller, token) = setup(&env);

        let id = c.create_listing(&seller, &String::from_str(&env, "Chairs"),
            &Category::Furniture, &50_000, &token, &10, &2, &dummy_hash(&env));

        let caller = Address::generate(&env);
        let remaining = c.deduct_stock(&caller, &id, &10);
        assert_eq!(remaining, 0);
        assert!(!c.get_listing(&id).active);
    }

    #[test]
    fn test_verify_seller() {
        let env = Env::default();
        let (c, admin, seller, _) = setup(&env);

        assert!(!c.is_verified(&seller));
        c.set_verified(&admin, &seller, &true);
        assert!(c.is_verified(&seller));
        c.set_verified(&admin, &seller, &false);
        assert!(!c.is_verified(&seller));
    }

    #[test]
    fn test_seller_listings_tracked() {
        let env = Env::default();
        env.ledger().set_timestamp(1_000);
        let (c, _, seller, token) = setup(&env);

        let id1 = c.create_listing(&seller, &String::from_str(&env, "A"),
            &Category::Other, &100, &token, &10, &1, &dummy_hash(&env));
        let id2 = c.create_listing(&seller, &String::from_str(&env, "B"),
            &Category::Other, &200, &token, &5, &1, &dummy_hash(&env));

        let ids = c.get_seller_listings(&seller);
        assert!(ids.contains(&id1));
        assert!(ids.contains(&id2));
    }
}
