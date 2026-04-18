#![no_std]
use soroban_sdk::{contract, contractimpl, contracttype, symbol_short, token, Address, Env, String, Vec};

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
    pub id: u64,
    pub buyer: Address,
    pub seller: Address,
    pub token: Address,
    pub amount: i128,
    pub description: String,
    pub status: DealStatus,
    pub created_at: u64,
    pub deadline: u64,
}

#[contracttype]
pub enum DataKey {
    Deal(u64),
    NextDealId,
    Arbiter,
    UserDeals(Address),
    Reputation(Address),
}

#[contract]
pub struct EscrowContract;

#[contractimpl]
impl EscrowContract {
    pub fn initialize(env: Env, arbiter: Address) {
        env.storage().instance().set(&DataKey::Arbiter, &arbiter);
        env.storage().instance().set(&DataKey::NextDealId, &0u64);
    }

    pub fn create_deal(
        env: Env,
        buyer: Address,
        seller: Address,
        token: Address,
        amount: i128,
        description: String,
        deadline: u64,
    ) -> u64 {
        buyer.require_auth();
        assert!(amount > 0, "amount must be positive");
        assert!(buyer != seller, "buyer cannot be seller");
        assert!(deadline > env.ledger().timestamp(), "deadline must be in the future");

        token::Client::new(&env, &token)
            .transfer(&buyer, &env.current_contract_address(), &amount);

        let id: u64 = env.storage().instance().get(&DataKey::NextDealId).unwrap_or(0);
        let deal = Deal {
            id,
            buyer: buyer.clone(),
            seller: seller.clone(),
            token,
            amount,
            description,
            status: DealStatus::Created,
            created_at: env.ledger().timestamp(),
            deadline,
        };

        env.storage().persistent().set(&DataKey::Deal(id), &deal);
        env.storage().instance().set(&DataKey::NextDealId, &(id + 1));

        Self::push_user_deal(&env, &buyer, id);
        Self::push_user_deal(&env, &seller, id);

        env.events().publish(
            (symbol_short!("deal"), symbol_short!("created")),
            (id, buyer, seller, amount),
        );

        id
    }

    pub fn mark_shipped(env: Env, seller: Address, deal_id: u64) {
        seller.require_auth();
        let mut deal: Deal = env.storage().persistent().get(&DataKey::Deal(deal_id)).unwrap();
        assert!(deal.seller == seller, "not the seller");
        assert!(deal.status == DealStatus::Created, "invalid status");
        assert!(env.ledger().timestamp() <= deal.deadline, "deal deadline has passed");

        deal.status = DealStatus::Shipped;
        env.storage().persistent().set(&DataKey::Deal(deal_id), &deal);

        env.events().publish(
            (symbol_short!("deal"), symbol_short!("shipped")),
            (deal_id, seller),
        );
    }

    pub fn expire_deal(env: Env, deal_id: u64) {
        let mut deal: Deal = env.storage().persistent().get(&DataKey::Deal(deal_id)).unwrap();
        assert!(deal.status == DealStatus::Created, "can only expire unshipped deals");
        assert!(env.ledger().timestamp() > deal.deadline, "deadline not yet passed");

        deal.status = DealStatus::Expired;
        env.storage().persistent().set(&DataKey::Deal(deal_id), &deal);

        token::Client::new(&env, &deal.token)
            .transfer(&env.current_contract_address(), &deal.buyer, &deal.amount);

        env.events().publish(
            (symbol_short!("deal"), symbol_short!("expired")),
            (deal_id, deal.buyer),
        );
    }

    pub fn confirm_received(env: Env, buyer: Address, deal_id: u64) {
        buyer.require_auth();
        let mut deal: Deal = env.storage().persistent().get(&DataKey::Deal(deal_id)).unwrap();
        assert!(deal.buyer == buyer, "not the buyer");
        assert!(deal.status == DealStatus::Shipped, "invalid status");

        deal.status = DealStatus::Confirmed;
        env.storage().persistent().set(&DataKey::Deal(deal_id), &deal);

        token::Client::new(&env, &deal.token)
            .transfer(&env.current_contract_address(), &deal.seller, &deal.amount);

        let rep: u64 = env.storage().persistent()
            .get(&DataKey::Reputation(deal.seller.clone())).unwrap_or(0);
        env.storage().persistent()
            .set(&DataKey::Reputation(deal.seller.clone()), &(rep + 1));

        env.events().publish(
            (symbol_short!("deal"), symbol_short!("done")),
            (deal_id, deal.seller, deal.amount),
        );
    }

    pub fn cancel_deal(env: Env, buyer: Address, deal_id: u64) {
        buyer.require_auth();
        let mut deal: Deal = env.storage().persistent().get(&DataKey::Deal(deal_id)).unwrap();
        assert!(deal.buyer == buyer, "not the buyer");
        assert!(deal.status == DealStatus::Created, "can only cancel before shipment");

        deal.status = DealStatus::Cancelled;
        env.storage().persistent().set(&DataKey::Deal(deal_id), &deal);

        token::Client::new(&env, &deal.token)
            .transfer(&env.current_contract_address(), &deal.buyer, &deal.amount);

        env.events().publish(
            (symbol_short!("deal"), symbol_short!("cancel")),
            (deal_id, deal.buyer),
        );
    }

    pub fn raise_dispute(env: Env, caller: Address, deal_id: u64) {
        caller.require_auth();
        let mut deal: Deal = env.storage().persistent().get(&DataKey::Deal(deal_id)).unwrap();
        assert!(caller == deal.buyer || caller == deal.seller, "unauthorized");
        assert!(
            deal.status == DealStatus::Created || deal.status == DealStatus::Shipped,
            "invalid status"
        );
        deal.status = DealStatus::Disputed;
        env.storage().persistent().set(&DataKey::Deal(deal_id), &deal);

        env.events().publish(
            (symbol_short!("deal"), symbol_short!("dispute")),
            (deal_id, caller),
        );
    }

    pub fn resolve_dispute(env: Env, arbiter: Address, deal_id: u64, refund_buyer: bool) {
        arbiter.require_auth();
        let stored_arbiter: Address = env.storage().instance().get(&DataKey::Arbiter).unwrap();
        assert!(arbiter == stored_arbiter, "not the arbiter");

        let mut deal: Deal = env.storage().persistent().get(&DataKey::Deal(deal_id)).unwrap();
        assert!(deal.status == DealStatus::Disputed, "not disputed");

        deal.status = DealStatus::Resolved;
        env.storage().persistent().set(&DataKey::Deal(deal_id), &deal);

        let recipient = if refund_buyer { deal.buyer.clone() } else { deal.seller.clone() };
        token::Client::new(&env, &deal.token)
            .transfer(&env.current_contract_address(), &recipient, &deal.amount);

        env.events().publish(
            (symbol_short!("deal"), symbol_short!("resolve")),
            (deal_id, refund_buyer),
        );
    }

    pub fn get_deal(env: Env, deal_id: u64) -> Deal {
        env.storage().persistent().get(&DataKey::Deal(deal_id)).unwrap()
    }

    pub fn get_reputation(env: Env, user: Address) -> u64 {
        env.storage().persistent().get(&DataKey::Reputation(user)).unwrap_or(0)
    }

    fn push_user_deal(env: &Env, user: &Address, deal_id: u64) {
        let key = DataKey::UserDeals(user.clone());
        let mut deals: Vec<u64> = env.storage().persistent().get(&key).unwrap_or(Vec::new(env));
        deals.push_back(deal_id);
        env.storage().persistent().set(&key, &deals);
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::{Client as TokenClient, StellarAssetClient},
        Env,
    };

    fn setup(env: &Env) -> (EscrowContractClient, Address, Address, Address, Address) {
        env.mock_all_auths();
        let contract_id = env.register_contract(None, EscrowContract);
        let client  = EscrowContractClient::new(env, &contract_id);
        let arbiter = Address::generate(env);
        let buyer   = Address::generate(env);
        let seller  = Address::generate(env);
        let admin   = Address::generate(env);
        let token   = env.register_stellar_asset_contract(admin.clone());
        StellarAssetClient::new(env, &token).mint(&buyer, &1_000_000_000);
        client.initialize(&arbiter);
        env.ledger().set_timestamp(1_000);
        (client, arbiter, buyer, seller, token)
    }

    #[test]
    fn test_happy_path() {
        let env = Env::default();
        let (client, _, buyer, seller, token) = setup(&env);

        let id = client.create_deal(
            &buyer, &seller, &token, &500_000_000,
            &String::from_str(&env, "500 phone cases"), &2_000,
        );
        client.mark_shipped(&seller, &id);
        client.confirm_received(&buyer, &id);

        assert_eq!(client.get_deal(&id).status, DealStatus::Confirmed);
        assert_eq!(client.get_reputation(&seller), 1);
        assert_eq!(TokenClient::new(&env, &token).balance(&seller), 500_000_000);
    }

    #[test]
    fn test_cancel_refunds_buyer() {
        let env = Env::default();
        let (client, _, buyer, seller, token) = setup(&env);

        let id = client.create_deal(
            &buyer, &seller, &token, &500_000_000,
            &String::from_str(&env, "test"), &2_000,
        );
        client.cancel_deal(&buyer, &id);

        assert_eq!(client.get_deal(&id).status, DealStatus::Cancelled);
        assert_eq!(TokenClient::new(&env, &token).balance(&buyer), 1_000_000_000);
    }

    #[test]
    fn test_dispute_resolved_for_buyer() {
        let env = Env::default();
        let (client, arbiter, buyer, seller, token) = setup(&env);

        let id = client.create_deal(
            &buyer, &seller, &token, &500_000_000,
            &String::from_str(&env, "disputed"), &2_000,
        );
        client.raise_dispute(&buyer, &id);
        client.resolve_dispute(&arbiter, &id, &true);

        assert_eq!(client.get_deal(&id).status, DealStatus::Resolved);
        assert_eq!(TokenClient::new(&env, &token).balance(&buyer), 1_000_000_000);
    }

    #[test]
    fn test_dispute_resolved_for_seller() {
        let env = Env::default();
        let (client, arbiter, buyer, seller, token) = setup(&env);

        let id = client.create_deal(
            &buyer, &seller, &token, &500_000_000,
            &String::from_str(&env, "disputed"), &2_000,
        );
        client.raise_dispute(&seller, &id);
        client.resolve_dispute(&arbiter, &id, &false);

        assert_eq!(TokenClient::new(&env, &token).balance(&seller), 500_000_000);
    }

    #[test]
    fn test_expire_deal() {
        let env = Env::default();
        let (client, _, buyer, seller, token) = setup(&env);

        let id = client.create_deal(
            &buyer, &seller, &token, &500_000_000,
            &String::from_str(&env, "expiring"), &2_000,
        );

        env.ledger().set_timestamp(3_000);
        client.expire_deal(&id);

        assert_eq!(client.get_deal(&id).status, DealStatus::Expired);
        assert_eq!(TokenClient::new(&env, &token).balance(&buyer), 1_000_000_000);
    }
}
