#![no_std]
extern crate alloc;
use crate::storage::unlock_temp;

pub fn get_pending_admin(env: Env) -> Option<Address> {
    storage::pending_admin::get(&env)
}

mod access;
mod events;
mod storage;
pub mod types;

use crate::access::{require_admin, require_not_paused, require_relayer};
use crate::events::emit;
use soroban_sdk::{
    contract, contractimpl, symbol_short, Address, Bytes, Env, FromVal, String as SorobanString,
    Symbol, Vec,
};
use storage::{assets, deposits, dlq, max_deposit, min_deposit, relayers, settlements};
use types::{DlqEntry, Event, Settlement, Transaction, TransactionStatus, MAX_RETRIES};

#[contract]
pub struct SynapseContract;

fn next_id(env: &Env, counter_key: Symbol) -> SorobanString {
    let nonce: u32 = env.storage().instance().get(&counter_key).unwrap_or(0);
    env.storage().instance().set(&counter_key, &(nonce + 1));

    let ts = env.ledger().timestamp();
    let seq = env.ledger().sequence();

    let mut data = [0u8; 16];
    data[..8].copy_from_slice(&ts.to_be_bytes());
    data[8..12].copy_from_slice(&seq.to_be_bytes());
    data[12..16].copy_from_slice(&nonce.to_be_bytes());

    let hash = env.crypto().sha256(&Bytes::from_slice(env, &data));
    let bytes = hash.to_array();

    let mut hex = [0u8; 32];
    const HEX: &[u8] = b"0123456789abcdef";
    for i in 0..16 {
        hex[i * 2] = HEX[(bytes[i] >> 4) as usize];
        hex[i * 2 + 1] = HEX[(bytes[i] & 0xf) as usize];
    }
    SorobanString::from_bytes(env, &hex)
}

#[contractimpl]
impl SynapseContract {
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().instance().has(&storage::StorageKey::Admin) {
            panic!("already initialised");
        }
        admin.require_auth();
        storage::admin::set(&env, &admin);
        emit(&env, Event::Initialized(admin));
    }

    // TODO(#3): emit `RelayerGranted` event
    pub fn grant_relayer(env: Env, caller: Address, relayer: Address) {
        require_not_paused(&env);
        let zero_addr = Address::from_string(&SorobanString::from_str(
            &env,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        ));
        if relayer == zero_addr {
            panic!("invalid relayer address")
        }
        require_admin(&env, &caller);
        relayers::add(&env, &relayer);
    }

    pub fn revoke_relayer(env: Env, caller: Address, relayer: Address) {
        require_not_paused(&env);
        require_admin(&env, &caller);
        if !relayers::has(&env, &relayer) {
            panic!("address is not a relayer")
        }
        relayers::remove(&env, &relayer);
        emit(&env, Event::RelayerRevoked(relayer));
    }

    // TODO(#8): two-step admin transfer (propose + accept) to prevent lockout
    pub fn transfer_admin(env: Env, caller: Address, new_admin: Address) {
        require_not_paused(&env);
        require_admin(&env, &caller);
        let old_admin = storage::admin::get(&env);
        storage::admin::set(&env, &new_admin);
        emit(&env, Event::AdminTransferred(old_admin, new_admin));
    }

    /// Propose a new admin address for transfer.
    /// Only the current admin can propose a new admin.
    /// The proposed admin must accept the transfer to complete it.
    pub fn propose_admin(env: Env, caller: Address, new_admin: Address) {
        require_admin(&env, &caller);
        storage::pending_admin::set(&env, &new_admin);
        let current_admin = storage::admin::get(&env);
        emit(&env, Event::AdminTransferProposed(current_admin, new_admin));
    }

    /// Accept the admin transfer proposal.
    /// Only the proposed admin can accept the transfer.
    /// After acceptance, the proposed admin becomes the new admin.
    pub fn accept_admin(env: Env, caller: Address) {
        caller.require_auth();
        let pending = storage::pending_admin::get(&env).expect("no pending admin transfer");
        if caller != pending {
            panic!("only proposed admin can accept");
        }
        let old_admin = storage::admin::get(&env);
        storage::admin::set(&env, &pending);
        storage::pending_admin::clear(&env);
        emit(&env, Event::AdminTransferred(old_admin, pending));
    }

    pub fn pause(env: Env, caller: Address) {
        require_admin(&env, &caller);
        storage::pause::set(&env, true);
        emit(&env, types::Event::ContractPaused(caller.clone()));
    }

    pub fn unpause(env: Env, caller: Address) {
        require_admin(&env, &caller);
        storage::pause::set(&env, false);
        emit(&env, Event::ContractUnpaused(caller.clone()));
    }

    // TODO(#13): cap the total number of allowed assets to bound instance storage
    pub fn add_asset(env: Env, caller: Address, asset_code: SorobanString) {
        require_not_paused(&env);
        require_admin(&env, &caller);
        if asset_code.len() == 0 {
            panic!("invalid asset code")
        }
        let mut buf = [0u8; 12];
        let len = asset_code.len() as usize;
        if len > buf.len() {
            panic!("invalid asset code")
        }
        asset_code.copy_into_slice(&mut buf[..len]);
        for b in &buf[..len] {
            if !b.is_ascii_uppercase() && !b.is_ascii_digit() {
                panic!("invalid asset code")
            }
        }
        assets::add(&env, &asset_code);
        emit(&env, Event::AssetAdded(asset_code));
    }

    pub fn remove_asset(env: Env, caller: Address, asset_code: SorobanString) {
        require_not_paused(&env);
        require_admin(&env, &caller);
        if !assets::is_allowed(&env, &asset_code) {
            panic!("asset not in allowlist");
        }
        assets::remove(&env, &asset_code);
        emit(&env, Event::AssetRemoved(asset_code));
    }

    pub fn set_min_deposit(env: Env, caller: Address, amount: i128) {
        require_admin(&env, &caller);
        if amount <= 0 {
            panic!("min deposit must be positive")
        }
        min_deposit::set(&env, amount);
    }

    pub fn get_min_deposit(env: Env) -> Option<i128> {
        min_deposit::get(&env)
    }

    pub fn set_max_deposit(env: Env, caller: Address, amount: i128) {
        require_not_paused(&env);
        require_admin(&env, &caller);
        if amount <= 0 {
            panic!("max deposit must be positive")
        }
        max_deposit::set(&env, &amount);
    }

    pub fn get_max_deposit(env: Env) -> i128 {
        max_deposit::get(&env).unwrap_or(0)
    }

    // TODO(#16): enforce maximum deposit amount (configurable by admin) — DONE

    // TODO(#15): enforce minimum deposit amount (configurable by admin)
    // TODO(#17): validate anchor_transaction_id is non-empty
    // TODO(#18): add `memo` field support (mirrors synapse-core CallbackPayload)
    // TODO(#20): add `callback_type` field (deposit | withdrawal)
    pub fn register_deposit(
        env: Env,
        caller: Address,
        anchor_transaction_id: SorobanString,
        stellar_account: Address,
        amount: i128,
        asset_code: SorobanString,
        memo: Option<SorobanString>,
        memo_type: Option<SorobanString>,
    ) -> SorobanString {
        require_not_paused(&env);
        require_relayer(&env, &caller);
        if anchor_transaction_id.len() == 0 {
            panic!("anchor_transaction_id must not be empty")
        }
        assets::require_allowed(&env, &asset_code);

        if let Some(min) = min_deposit::get(&env) {
            if amount < min {
                panic!("amount below min deposit")
            }
        }
        if let Some(max) = max_deposit::get(&env) {
            if amount > max {
                panic!("amount exceeds max deposit")
            }
        }
        if let Some(existing) = deposits::find_by_anchor_id(&env, &anchor_transaction_id) {
            unlock_temp(&env, &anchor_transaction_id);
            return existing;
        }

        let tx_id = next_id(&env, symbol_short!("txnonce"));
        let tx = Transaction::new(
            &env,
            tx_id,
            anchor_transaction_id.clone(),
            stellar_account,
            caller.clone(),
            amount,
            asset_code,
            memo,
            memo_type,
        );
        let id = tx.id.clone();
        deposits::save(&env, &tx);
        deposits::index_anchor_id(&env, &anchor_transaction_id, &id);
        emit(
            &env,
            Event::DepositRegistered(id.clone(), anchor_transaction_id),
        );
        id
    }

    pub fn mark_processing(env: Env, caller: Address, tx_id: SorobanString) {
        require_not_paused(&env);
        require_relayer(&env, &caller);
        let mut tx = deposits::get(&env, &tx_id);
        let old_status = tx.status.clone();
        if tx.status != TransactionStatus::Pending {
            panic!("transaction must be Pending");
        }
        tx.status = TransactionStatus::Processing;
        tx.updated_ledger = env.ledger().sequence();
        deposits::save(&env, &tx);
        emit(
            &env,
            Event::StatusUpdated(tx_id, TransactionStatus::Processing),
        );
    }

    pub fn mark_completed(env: Env, caller: Address, tx_id: SorobanString) {
        require_not_paused(&env);
        require_relayer(&env, &caller);
        let mut tx = deposits::get(&env, &tx_id);
        if tx.status != TransactionStatus::Processing {
            panic!("transaction must be Processing");
        }
        tx.status = TransactionStatus::Completed;
        tx.updated_ledger = env.ledger().sequence();
        deposits::save(&env, &tx);
        if dlq::get(&env, &tx_id).is_some() {
            dlq::remove(&env, &tx_id);
        }
        emit(
            &env,
            Event::StatusUpdated(tx_id, TransactionStatus::Completed),
        );
    }
    // TODO(#26): enforce transition guard — must be Pending or Processing
    // TODO(#28): validate error_reason is non-empty
    pub fn mark_failed(
        env: Env,
        caller: Address,
        tx_id: SorobanString,
        error_reason: SorobanString,
    ) {
        require_not_paused(&env);
        require_relayer(&env, &caller);
        if error_reason.len() == 0 {
            panic!("error_reason must not be empty");
        }
        let mut tx = deposits::get(&env, &tx_id);
        if tx.status == TransactionStatus::Completed {
            panic!("cannot fail completed transaction");
        }
        if tx.status == TransactionStatus::Failed {
            panic!("transaction already Failed");
        }
        let old_status = tx.status.clone();
        tx.status = TransactionStatus::Failed;
        tx.updated_ledger = env.ledger().sequence();
        deposits::save(&env, &tx);
        emit(
            &env,
            Event::StatusUpdated(tx_id.clone(), TransactionStatus::Failed),
        );
        let entry = DlqEntry::new(&env, tx_id.clone(), error_reason.clone());
        dlq::push(&env, &entry);
        emit(&env, Event::MovedToDlq(tx_id.clone(), error_reason.clone()));
    }

    // TODO(#31): emit `DlqRetried` event
    pub fn retry_dlq(env: Env, caller: Address, tx_id: SorobanString) {
        require_not_paused(&env);
        let mut entry = dlq::get(&env, &tx_id).expect("dlq entry not found");
        let mut tx = deposits::get(&env, &tx_id);
        let admin = storage::admin::get(&env);
        if caller != admin && caller != tx.relayer {
            panic!("not admin or original relayer");
        }
        caller.require_auth();

        if entry.retry_count >= MAX_RETRIES {
            emit(&env, Event::MaxRetriesExceeded(tx_id.clone()));
            panic!("max retries exceeded");
        }

        tx.status = TransactionStatus::Pending;
        tx.updated_ledger = env.ledger().sequence();
        entry.retry_count += 1;
        entry.last_retry_ledger = env.ledger().sequence();
        deposits::save(&env, &tx);
        dlq::remove(&env, &tx_id);

        emit(
            &env,
            Event::StatusUpdated(tx_id, TransactionStatus::Pending),
        );
    }

    // TODO(#32): enforce transition guard — must be Pending or Processing
    pub fn cancel_transaction(env: Env, caller: Address, tx_id: SorobanString) {
        require_not_paused(&env);
        require_admin(&env, &caller);
        let mut tx = deposits::get(&env, &tx_id);
        tx.status = TransactionStatus::Cancelled;
        tx.updated_ledger = env.ledger().sequence();
        deposits::save(&env, &tx);
        emit(
            &env,
            Event::StatusUpdated(tx_id, TransactionStatus::Cancelled),
        );
    }

    pub fn finalize_settlement(
        env: Env,
        caller: Address,
        asset_code: SorobanString,
        tx_ids: Vec<SorobanString>,
        total_amount: i128,
        period_start: u64,
        period_end: u64,
    ) -> SorobanString {
        require_not_paused(&env);
        require_relayer(&env, &caller);
        if period_start > period_end {
            panic!("period_start must be <= period_end")
        }
        let settlement_id = next_id(&env, symbol_short!("stlnonce"));
        let s = Settlement::new(
            &env,
            settlement_id.clone(),
            asset_code.clone(),
            tx_ids.clone(),
            total_amount,
            period_start,
            period_end,
        );
        // Update all transactions with the settlement id, check for already settled
        for tx_id in tx_ids.iter() {
            let mut tx = deposits::get(&env, &tx_id);
            if tx.settlement_id.len() > 0 {
                panic!("transaction already settled");
            }
            tx.settlement_id = settlement_id.clone();
            tx.updated_ledger = env.ledger().sequence();
            deposits::save(&env, &tx);
            emit(&env, Event::Settled(tx_id, settlement_id.clone()));
        }
        settlements::save(&env, &s);
        emit(
            &env,
            Event::SettlementFinalized(settlement_id.clone(), asset_code, total_amount),
        );
        settlement_id
    }

    // TODO(#40): add `get_dlq_entry(tx_id)` query
    // TODO(#41): add `get_admin()` query — DONE
    // TODO(#43): add `get_min_deposit()` query
    pub fn get_dlq_entry(env: Env, tx_id: SorobanString) -> Option<DlqEntry> {
        dlq::get(&env, &tx_id)
    }

    // TODO(#41): add `get_admin()` query
    // TODO(#43): add `get_min_deposit()` query — DONE
    // TODO(#44): add `get_max_deposit()` query — DONE

    pub fn get_admin(env: Env) -> Address {
        storage::admin::get(&env)
    }

    // Removed duplicate is_paused definition
    pub fn get_transaction(env: Env, tx_id: SorobanString) -> Transaction {
        env.storage()
            .persistent()
            .get(&storage::StorageKey::Tx(tx_id))
            .expect("transaction not found")
    }

    pub fn get_transaction_by_anchor_id(env: Env, anchor_transaction_id: SorobanString) -> Transaction {
        let tx_id = deposits::find_by_anchor_id(&env, &anchor_transaction_id)
            .expect("transaction not found");
        deposits::get(&env, &tx_id)
    }

    pub fn get_settlement(env: Env, settlement_id: SorobanString) -> Settlement {
        settlements::get(&env, &settlement_id)
    }

    pub fn is_asset_allowed(env: Env, asset_code: SorobanString) -> bool {
        assets::is_allowed(&env, &asset_code)
    }

    pub fn is_relayer(env: Env, address: Address) -> bool {
        relayers::has(&env, &address)
    }

    pub fn is_paused(env: Env) -> bool {
        storage::pause::is_paused(&env)
    }
}

#[cfg(test)]
mod tests {
    // Add test asset codes for asset cap tests
    const TEST_ASSET_CODES: &[&str] = &[
        "USD", "BTC", "ETH", "USDC", "USDT", "DAI", "SOL", "BNB", "AVAX", "MATIC", "ARB", "OP",
        "FTM", "DOGE", "LTC", "BCH", "XRP", "LINK", "UNI", "AAVE",
    ];
    use super::*;
    use crate::storage::StorageKey;
    use crate::types::Transaction;
    use soroban_sdk::testutils::storage::Persistent;
    use soroban_sdk::{
        symbol_short,
        testutils::{Address as _, Events as _, Ledger as _},
        vec, Env, IntoVal, String as SorobanString, TryFromVal,
    };

    fn setup(env: &Env) -> (Address, Address) {
        env.mock_all_auths();
        let contract_id = env.register_contract(None, SynapseContract);
        let client = SynapseContractClient::new(env, &contract_id);
        let admin = Address::generate(env);
        client.initialize(&admin);
        (admin, contract_id)
    }

    fn setup_relayer_deposit<'a>(
        env: &'a Env,
        anchor_label: &str,
    ) -> (SynapseContractClient<'a>, Address, SorobanString) {
        let (admin, contract_id) = setup(env);
        let client = SynapseContractClient::new(env, &contract_id);
        let relayer = Address::generate(env);
        let stellar = Address::generate(env);
        let asset = SorobanString::from_str(env, "USD");
        let anchor_id = SorobanString::from_str(env, anchor_label);
        client.grant_relayer(&admin, &relayer);
        client.add_asset(&admin, &asset);
        let tx_id =
            client.register_deposit(&relayer, &anchor_id, &stellar, &1i128, &asset, &None, &None);
        (client, relayer, tx_id)
    }

    #[test]
    fn test_register_deposit_stores_memo_type() {
        let env = Env::default();
        let (admin, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);
        let relayer = Address::generate(&env);
        let stellar = Address::generate(&env);
        let asset = SorobanString::from_str(&env, "USD");
        client.grant_relayer(&admin, &relayer);
        client.add_asset(&admin, &asset);
        let mt = SorobanString::from_str(&env, "text");
        let tx_id = client.register_deposit(
            &relayer,
            &SorobanString::from_str(&env, "mt-stored"),
            &stellar,
            &1i128,
            &asset,
            &None,
            &Some(mt.clone()),
        );
        let tx = client.get_transaction(&tx_id);
        assert_eq!(tx.memo_type, Some(mt));
    }

    #[should_panic(expected = "invalid memo_type")]
    fn test_register_deposit_panics_on_invalid_memo_type() {
        let env = Env::default();
        let (admin, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);
        let relayer = Address::generate(&env);
        let stellar = Address::generate(&env);
        let asset = SorobanString::from_str(&env, "USD");
        client.grant_relayer(&admin, &relayer);
        client.add_asset(&admin, &asset);
        client.register_deposit(
            &relayer,
            &SorobanString::from_str(&env, "mt-bad"),
            &stellar,
            &1i128,
            &asset,
            &None,
            &Some(SorobanString::from_str(&env, "bad")),
        );
    }

    #[test]
    fn test_register_deposit_memo_type_none_is_valid() {
        let env = Env::default();
        let (client, relayer, tx_id) = setup_relayer_deposit(&env, "mt-none");
        let tx = client.get_transaction(&tx_id);
        assert!(tx.memo_type.is_none());
    }

    #[test]
    #[should_panic(expected = "transaction must be Pending")]
    fn test_mark_processing_panics_when_not_pending() {
        let env = Env::default();
        let (client, relayer, tx_id) = setup_relayer_deposit(&env, "mp-not-pending");
        client.mark_processing(&relayer, &tx_id);
        client.mark_completed(&relayer, &tx_id);
        // tx is now Completed — mark_processing must panic
        client.mark_processing(&relayer, &tx_id);
    }

    #[test]
    #[should_panic(expected = "error_reason must not be empty")]
    fn test_mark_failed_panics_when_error_reason_empty() {
        let env = Env::default();
        let (client, relayer, tx_id) = setup_relayer_deposit(&env, "mf-empty-reason");
        client.mark_failed(&relayer, &tx_id, &SorobanString::from_str(&env, ""));
    }

    #[test]
    fn test_mark_failed_allowed_when_pending() {
        let env = Env::default();
        let (client, relayer, tx_id) = setup_relayer_deposit(&env, "mf-pending");
        let err = SorobanString::from_str(&env, "boom");
        client.mark_failed(&relayer, &tx_id, &err);
        let tx = client.get_transaction(&tx_id);
        assert!(matches!(tx.status, TransactionStatus::Failed));
    }

    #[test]
    fn test_mark_failed_allowed_when_processing() {
        let env = Env::default();
        let (client, relayer, tx_id) = setup_relayer_deposit(&env, "mf-processing");
        client.mark_processing(&relayer, &tx_id);
        let err = SorobanString::from_str(&env, "boom");
        client.mark_failed(&relayer, &tx_id, &err);
        let tx = client.get_transaction(&tx_id);
        assert!(matches!(tx.status, TransactionStatus::Failed));
    }

    #[test]
    #[should_panic(expected = "transaction must be Processing")]
    fn test_mark_completed_panics_when_pending() {
        let env = Env::default();
        let (client, relayer, tx_id) = setup_relayer_deposit(&env, "mc-pending");
        client.mark_completed(&relayer, &tx_id);
    }

    #[test]
    #[should_panic(expected = "transaction must be Processing")]
    fn test_mark_completed_panics_when_already_completed() {
        let env = Env::default();
        let (client, relayer, tx_id) = setup_relayer_deposit(&env, "mc-twice");
        client.mark_processing(&relayer, &tx_id);
        client.mark_completed(&relayer, &tx_id);
        client.mark_completed(&relayer, &tx_id);
    }

    #[test]
    #[should_panic(expected = "cannot fail completed transaction")]
    fn test_mark_failed_panics_when_completed() {
        let env = Env::default();
        let (client, relayer, tx_id) = setup_relayer_deposit(&env, "mf-completed");
        client.mark_processing(&relayer, &tx_id);
        client.mark_completed(&relayer, &tx_id);
        client.mark_failed(
            &relayer,
            &tx_id,
            &SorobanString::from_str(&env, "late-fail"),
        );
    }

    #[test]
    #[test]
    #[should_panic(expected = "transaction must be Processing")]
    fn test_mark_completed_panics_when_not_processing() {
        let env = Env::default();
        let (client, relayer, tx_id) = setup_relayer_deposit(&env, "mc-not-processing");
        client.mark_completed(&relayer, &tx_id);
    }

    #[test]
    fn test_mark_completed_succeeds_when_processing() {
        let env = Env::default();
        let (client, relayer, tx_id) = setup_relayer_deposit(&env, "mc-processing");
        client.mark_processing(&relayer, &tx_id);
        client.mark_completed(&relayer, &tx_id);
        let tx = client.get_transaction(&tx_id);
        assert!(matches!(tx.status, TransactionStatus::Completed));
    }

    #[test]
    #[should_panic(expected = "transaction already Failed")]
    fn test_mark_failed_panics_when_already_failed() {
        let env = Env::default();
        let (client, relayer, tx_id) = setup_relayer_deposit(&env, "mf-twice");
        let err = SorobanString::from_str(&env, "first");
        client.mark_failed(&relayer, &tx_id, &err);
        client.mark_failed(&relayer, &tx_id, &SorobanString::from_str(&env, "second"));
    }

    #[test]
    fn test_initialize_emits_initialized_event() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, SynapseContract);
        let client = SynapseContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin);
        let events = env.events().all();
        assert_eq!(events.len(), 1);
        let (emitting_contract, topics, _data) = events.get(0).unwrap();
        assert_eq!(emitting_contract, contract_id);
        assert_eq!(topics, (symbol_short!("synapse"),).into_val(&env));
    }

    #[test]
    fn test_transfer_admin_emits_event() {
        let env = Env::default();
        let (admin, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);
        let new_admin = Address::generate(&env);

        client.transfer_admin(&admin, &new_admin);

        let events = env.events().all();
        let (_, _, data) = events.last().unwrap();
        let (event, _ledger): (Event, u32) = <(Event, u32)>::from_val(&env, &data);
        assert_eq!(event, Event::AdminTransferred(admin, new_admin.clone()),);
        // new admin is now stored
        assert_eq!(client.get_admin(), new_admin);
    }

    #[test]
    fn test_is_paused() {
        let env = Env::default();
        let (admin, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);
        assert!(!client.is_paused());
        client.pause(&admin);
        assert!(client.is_paused());
        client.unpause(&admin);
        assert!(!client.is_paused());
    }

    #[test]
    fn test_min_deposit() {
        let env = Env::default();
        let (admin, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);

        // Default should be 0
        assert_eq!(client.get_max_deposit(), 0);

        // Set to 1000
        client.set_max_deposit(&admin, &1000i128);
        assert_eq!(client.get_max_deposit(), 1000i128);
        // Not set yet — should return None
        assert_eq!(client.get_min_deposit(), None);

        // Set to 100
        client.set_min_deposit(&admin, &100i128);
        assert_eq!(client.get_min_deposit(), Some(100i128));

        // Update to 500
        client.set_min_deposit(&admin, &500i128);
        assert_eq!(client.get_min_deposit(), Some(500i128));
    }

    #[test]
    fn test_max_deposit() {
        let env = Env::default();
        let (admin, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);
        assert_eq!(client.get_max_deposit(), 0i128);
        client.set_max_deposit(&admin, &1000i128);
        assert_eq!(client.get_max_deposit(), 1000i128);
        client.set_max_deposit(&admin, &5000i128);
        assert_eq!(client.get_max_deposit(), 5000i128);
    }

    #[test]
    fn test_add_asset_respects_max_assets_cap() {
        let env = Env::default();
        let (admin, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);
        for code in TEST_ASSET_CODES {
            client.add_asset(&admin, &SorobanString::from_str(&env, code));
        }
    }

    #[test]
    #[should_panic(expected = "asset cap reached")]
    fn test_add_asset_panics_when_cap_exceeded() {
        let env = Env::default();
        let (admin, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);
        for code in TEST_ASSET_CODES {
            client.add_asset(&admin, &SorobanString::from_str(&env, code));
        }
        // This should panic with "asset cap reached"
        client.add_asset(&admin, &SorobanString::from_str(&env, "OVERFLOW"));
    }

    #[test]
    fn test_transaction_id_deterministic_across_envs() {
        use alloc::string::ToString;
        let env1 = Env::default();
        let env2 = Env::default();
        let (admin1, contract_id1) = setup(&env1);
        let (admin2, contract_id2) = setup(&env2);
        let client1 = SynapseContractClient::new(&env1, &contract_id1);
        let client2 = SynapseContractClient::new(&env2, &contract_id2);

        let relayer1 = Address::generate(&env1);
        let relayer2 = Address::generate(&env2);
        client1.grant_relayer(&admin1, &relayer1);
        client2.grant_relayer(&admin2, &relayer2);

        client1.add_asset(&admin1, &SorobanString::from_str(&env1, "USD"));
        client2.add_asset(&admin2, &SorobanString::from_str(&env2, "USD"));

        let anchor_id = SorobanString::from_str(&env1, "deterministic-anchor");
        let tx_id_1 = client1.register_deposit(
            &relayer1,
            &anchor_id,
            &Address::generate(&env1),
            &100_000_000,
            &SorobanString::from_str(&env1, "USD"),
            &None,
            &None,
        );

        let tx_id_2 = client2.register_deposit(
            &relayer2,
            &SorobanString::from_str(&env2, "deterministic-anchor"),
            &Address::generate(&env2),
            &100_000_000,
            &SorobanString::from_str(&env2, "USD"),
            &None,
            &None,
        );

        assert_eq!(tx_id_1.to_string(), tx_id_2.to_string());
    }
    #[test]
    fn test_register_deposit_extends_anchor_idx_ttl() {
        let env = Env::default();
        let (admin, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);
        let relayer = Address::generate(&env);
        let stellar = Address::generate(&env);
        let asset = SorobanString::from_str(&env, "USD");
        let anchor_id = SorobanString::from_str(&env, "ttl-anchor");
        client.grant_relayer(&admin, &relayer);
        client.add_asset(&admin, &asset);
        let tx_id =
            client.register_deposit(&relayer, &anchor_id, &stellar, &1i128, &asset, &None, &None);
        let anchor_key = StorageKey::AnchorIdx(anchor_id);
        let tx_key = StorageKey::Tx(tx_id);
        let (ttl_anchor, ttl_tx) = env.as_contract(&contract_id, || {
            let p = env.storage().persistent();
            (p.get_ttl(&anchor_key), p.get_ttl(&tx_key))
        });
        assert_eq!(ttl_anchor, ttl_tx);
    }

    #[test]
    fn test_register_deposit_initializes_memo_to_none() {
        let env = Env::default();
        let (client, _relayer, tx_id) = setup_relayer_deposit(&env, "memo-none");
        let tx = client.get_transaction(&tx_id);
        assert!(tx.memo.is_none());
    }

    // Removed orphaned #[test] attribute
    // Removed duplicate #[test] attribute

    #[test]
    fn test_retry_dlq_removes_dlq_entry() {
        let env = Env::default();
        let (admin, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);
        let relayer = Address::generate(&env);
        let stellar = Address::generate(&env);
        let asset = SorobanString::from_str(&env, "USD");
        client.grant_relayer(&admin, &relayer);
        client.add_asset(&admin, &asset);
        let tx_id = client.register_deposit(
            &relayer,
            &SorobanString::from_str(&env, "retry-remove"),
            &stellar,
            &1i128,
            &asset,
            &None,
            &None,
        );
        client.mark_failed(&relayer, &tx_id, &SorobanString::from_str(&env, "err"));
        client.retry_dlq(&admin, &tx_id);
        let entry = env.as_contract(&contract_id, || storage::dlq::get(&env, &tx_id));
        assert!(entry.is_none());
    }

    #[test]
    #[should_panic(expected = "period_start must be <= period_end")]
    fn test_finalize_settlement_panics_when_period_start_exceeds_period_end() {
        let env = Env::default();
        let (admin, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);
        let relayer = Address::generate(&env);
        client.grant_relayer(&admin, &relayer);
        client.add_asset(&admin, &SorobanString::from_str(&env, "USD"));
        client.finalize_settlement(
            &relayer,
            &SorobanString::from_str(&env, "USD"),
            &vec![&env],
            &0i128,
            &2u64,
            &1u64,
        );
    }

    #[test]
    #[should_panic(expected = "anchor_transaction_id must not be empty")]
    fn test_register_deposit_panics_on_empty_anchor_id() {
        let env = Env::default();
        let (admin, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);
        let relayer = Address::generate(&env);
        let stellar = Address::generate(&env);
        let asset = SorobanString::from_str(&env, "USD");
        client.grant_relayer(&admin, &relayer);
        client.add_asset(&admin, &asset);
        client.register_deposit(
            &relayer,
            &SorobanString::from_str(&env, ""),
            &stellar,
            &100i128,
            &asset,
            &None,
            &None,
        );
    }

    #[test]
    fn test_finalize_settlement_writes_settlement_id_back_onto_transactions() {
        let env = Env::default();
        let (client, relayer, tx_id) = setup_relayer_deposit(&env, "settle-backref");
        client.mark_processing(&relayer, &tx_id);
        client.mark_completed(&relayer, &tx_id);
        let settlement_id = client.finalize_settlement(
            &relayer,
            &SorobanString::from_str(&env, "USD"),
            &vec![&env, tx_id.clone()],
            &1i128,
            &0u64,
            &1u64,
        );
        let tx = client.get_transaction(&tx_id);
        assert_eq!(tx.settlement_id, settlement_id);
    }

    #[test]
    fn test_retry_dlq_success() {
        let env = Env::default();
        let (client, relayer, tx_id) = setup_relayer_deposit(&env, "retry-tx");

        let admin = env.as_contract(&client.address, || storage::admin::get(&env));
        let err = SorobanString::from_str(&env, "failed-initially");

        // 1. Mark as failed
        client.mark_failed(&relayer, &tx_id, &err);
        let tx_failed = client.get_transaction(&tx_id);
        assert!(matches!(tx_failed.status, TransactionStatus::Failed));

        // 2. Retry DLQ
        env.ledger().set_sequence_number(100); // Advance ledger to check updates
        client.retry_dlq(&admin, &tx_id);

        // 3. Verify Transaction
        let tx_retried = client.get_transaction(&tx_id);
        assert!(matches!(tx_retried.status, TransactionStatus::Pending));
        assert_eq!(tx_retried.updated_ledger, 100);

        // 4. Verify DLQ Entry is removed after retry
        let entry = env.as_contract(&client.address, || storage::dlq::get(&env, &tx_id));
        assert!(entry.is_none());
    }

    #[test]
    fn test_finalize_settlement_succeeds_when_transactions_unsettled() {
        let env = Env::default();
        let (admin, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);
        let relayer = Address::generate(&env);
        let stellar = Address::generate(&env);
        let asset = SorobanString::from_str(&env, "USD");
        let anchor_id = SorobanString::from_str(&env, "finalize-ok-anchor");
        client.grant_relayer(&admin, &relayer);
        client.add_asset(&admin, &asset);
        let tx_id = client.register_deposit(
            &relayer, &anchor_id, &stellar, &100i128, &asset, &None, &None,
        );

        client.mark_processing(&relayer, &tx_id);
        client.mark_completed(&relayer, &tx_id);

        let settlement_id = client.finalize_settlement(
            &relayer,
            &asset,
            &vec![&env, tx_id.clone()],
            &100i128,
            &1u64,
            &2u64,
        );
        assert!(settlement_id.len() > 0);
        let s = client.get_settlement(&settlement_id);
        assert_eq!(s.total_amount, 100i128);
    }

    #[test]
    #[should_panic(expected = "invalid asset code")]
    fn test_add_asset_panics_on_empty_code() {
        let env = Env::default();
        let (admin, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);
        client.add_asset(&admin, &SorobanString::from_str(&env, ""));
    }

    #[test]
    #[should_panic(expected = "invalid asset code")]
    fn test_add_asset_panics_on_lowercase_code() {
        let env = Env::default();
        let (admin, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);
        client.add_asset(&admin, &SorobanString::from_str(&env, "usd"));
    }

    #[test]
    #[should_panic(expected = "invalid asset code")]
    fn test_add_asset_panics_on_special_char_code() {
        let env = Env::default();
        let (admin, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);
        client.add_asset(&admin, &SorobanString::from_str(&env, "US$"));
    }
    #[test]
    fn test_cancel_transaction_success() {
        let env = Env::default();
        let (admin, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);
        let relayer = Address::generate(&env);
        let stellar = Address::generate(&env);
        let asset = SorobanString::from_str(&env, "USD");
        let anchor_id = SorobanString::from_str(&env, "cancel-tx");

        client.grant_relayer(&admin, &relayer);
        client.add_asset(&admin, &asset);
        let tx_id = client.register_deposit(
            &relayer, &anchor_id, &stellar, &100i128, &asset, &None, &None,
        );

        // Cancel the transaction
        client.cancel_transaction(&admin, &tx_id);

        let tx = client.get_transaction(&tx_id);
        assert!(matches!(tx.status, TransactionStatus::Cancelled));
        assert!(tx.updated_ledger >= tx.created_ledger);
    }

    #[test]
    #[should_panic(expected = "not admin")]
    fn test_cancel_transaction_panics_when_not_admin() {
        let env = Env::default();
        let (client, relayer, tx_id) = setup_relayer_deposit(&env, "cancel-unauth");

        // Try to cancel with non-admin (relayer)
        client.cancel_transaction(&relayer, &tx_id);
    }

    #[test]
    #[should_panic(expected = "transaction already settled")]
    fn test_finalize_settlement_panics_when_transaction_already_settled() {
        let env = Env::default();
        let (admin, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);
        let relayer = Address::generate(&env);
        let stellar = Address::generate(&env);
        let asset = SorobanString::from_str(&env, "USD");
        let anchor_id = SorobanString::from_str(&env, "finalize-dup-tx");
        client.grant_relayer(&admin, &relayer);
        client.add_asset(&admin, &asset);
        let tx_id = client.register_deposit(
            &relayer, &anchor_id, &stellar, &100i128, &asset, &None, &None,
        );

        client.mark_processing(&relayer, &tx_id);
        client.mark_completed(&relayer, &tx_id);

        env.as_contract(&contract_id, || {
            let p = env.storage().persistent();
            let key = StorageKey::Tx(tx_id.clone());
            let mut tx: Transaction = p.get(&key).expect("tx");
            tx.settlement_id = SorobanString::from_str(&env, "prior-settlement");
            p.set(&key, &tx);
        });
        client.finalize_settlement(
            &relayer,
            &asset,
            &vec![&env, tx_id.clone()],
            &100i128,
            &1u64,
            &2u64,
        );

        client.finalize_settlement(
            &relayer,
            &asset,
            &vec![&env, tx_id.clone()],
            &100i128,
            &1u64,
            &2u64,
        );
    }

    // -----------------------------------------------------------------------
    // Pause enforcement — issue #10
    // -----------------------------------------------------------------------

    fn setup_with_relayer(env: &Env) -> (Address, Address, Address, SynapseContractClient) {
        let (admin, contract_id) = setup(env);
        let client = SynapseContractClient::new(env, &contract_id);
        let relayer = Address::generate(env);
        client.grant_relayer(&admin, &relayer);
        client.add_asset(&admin, &SorobanString::from_str(env, "USD"));
        (admin, contract_id, relayer, client)
    }

    #[test]
    #[should_panic(expected = "contract paused")]
    fn test_paused_blocks_grant_relayer() {
        let env = Env::default();
        let (admin, _, _, client) = setup_with_relayer(&env);
        client.pause(&admin);
        client.grant_relayer(&admin, &Address::generate(&env));
    }

    #[test]
    #[should_panic(expected = "contract paused")]
    fn test_paused_blocks_revoke_relayer() {
        let env = Env::default();
        let (admin, _, relayer, client) = setup_with_relayer(&env);
        client.pause(&admin);
        client.revoke_relayer(&admin, &relayer);
    }

    #[test]
    #[should_panic(expected = "contract paused")]
    fn test_paused_blocks_transfer_admin() {
        let env = Env::default();
        let (admin, _, _, client) = setup_with_relayer(&env);
        client.pause(&admin);
        client.transfer_admin(&admin, &Address::generate(&env));
    }

    #[test]
    #[should_panic(expected = "contract paused")]
    fn test_paused_blocks_add_asset() {
        let env = Env::default();
        let (admin, _, _, client) = setup_with_relayer(&env);
        client.pause(&admin);
        client.add_asset(&admin, &SorobanString::from_str(&env, "BTC"));
    }

    #[test]
    #[should_panic(expected = "contract paused")]
    fn test_paused_blocks_remove_asset() {
        let env = Env::default();
        let (admin, _, _, client) = setup_with_relayer(&env);
        client.pause(&admin);
        client.remove_asset(&admin, &SorobanString::from_str(&env, "USD"));
    }

    #[test]
    #[should_panic(expected = "contract paused")]
    fn test_paused_blocks_set_max_deposit() {
        let env = Env::default();
        let (admin, _, _, client) = setup_with_relayer(&env);
        client.pause(&admin);
        client.set_max_deposit(&admin, &1000i128);
    }

    #[test]
    #[should_panic(expected = "contract paused")]
    fn test_paused_blocks_register_deposit() {
        let env = Env::default();
        let (admin, _, relayer, client) = setup_with_relayer(&env);
        client.pause(&admin);
        client.register_deposit(
            &relayer,
            &SorobanString::from_str(&env, "anchor-paused"),
            &Address::generate(&env),
            &100i128,
            &SorobanString::from_str(&env, "USD"),
            &None,
            &None,
        );
    }

    #[test]
    #[should_panic(expected = "contract paused")]
    fn test_paused_blocks_mark_processing() {
        let env = Env::default();
        let (admin, _, relayer, client) = setup_with_relayer(&env);
        let tx_id = client.register_deposit(
            &relayer,
            &SorobanString::from_str(&env, "anchor-mp"),
            &Address::generate(&env),
            &100i128,
            &SorobanString::from_str(&env, "USD"),
            &None,
            &None,
        );
        client.pause(&admin);
        client.mark_processing(&relayer, &tx_id);
    }

    #[test]
    #[should_panic(expected = "contract paused")]
    fn test_paused_blocks_mark_completed() {
        let env = Env::default();
        let (admin, _, relayer, client) = setup_with_relayer(&env);
        let tx_id = client.register_deposit(
            &relayer,
            &SorobanString::from_str(&env, "anchor-mc"),
            &Address::generate(&env),
            &100i128,
            &SorobanString::from_str(&env, "USD"),
            &None,
            &None,
        );
        client.mark_processing(&relayer, &tx_id);
        client.pause(&admin);
        client.mark_completed(&relayer, &tx_id);
    }

    #[test]
    #[should_panic(expected = "contract paused")]
    fn test_paused_blocks_mark_failed() {
        let env = Env::default();
        let (admin, _, relayer, client) = setup_with_relayer(&env);
        let tx_id = client.register_deposit(
            &relayer,
            &SorobanString::from_str(&env, "anchor-mf"),
            &Address::generate(&env),
            &100i128,
            &SorobanString::from_str(&env, "USD"),
            &None,
            &None,
        );
        client.pause(&admin);
        client.mark_failed(&relayer, &tx_id, &SorobanString::from_str(&env, "err"));
    }

    #[test]
    #[should_panic(expected = "contract paused")]
    fn test_paused_blocks_retry_dlq() {
        let env = Env::default();
        let (admin, _, relayer, client) = setup_with_relayer(&env);
        let tx_id = client.register_deposit(
            &relayer,
            &SorobanString::from_str(&env, "anchor-rdlq"),
            &Address::generate(&env),
            &100i128,
            &SorobanString::from_str(&env, "USD"),
            &None,
            &None,
        );
        client.mark_failed(&relayer, &tx_id, &SorobanString::from_str(&env, "err"));
        client.pause(&admin);
        client.retry_dlq(&admin, &tx_id);
    }

    #[test]
    #[should_panic(expected = "contract paused")]
    fn test_paused_blocks_finalize_settlement() {
        let env = Env::default();
        let (admin, _, relayer, client) = setup_with_relayer(&env);
        let tx_id = client.register_deposit(
            &relayer,
            &SorobanString::from_str(&env, "anchor-fs"),
            &Address::generate(&env),
            &100i128,
            &SorobanString::from_str(&env, "USD"),
            &None,
            &None,
        );
        client.pause(&admin);
        client.finalize_settlement(
            &relayer,
            &SorobanString::from_str(&env, "USD"),
            &vec![&env, tx_id],
            &100i128,
            &0u64,
            &1u64,
        );
    }

    // -----------------------------------------------------------------------
    // Two-step admin transfer — issue #8
    // -----------------------------------------------------------------------

    #[test]
    fn test_propose_admin_sets_pending() {
        let env = Env::default();
        let (admin, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);
        let new_admin = Address::generate(&env);

        client.propose_admin(&admin, &new_admin);
        env.as_contract(&contract_id, || {
            assert_eq!(get_pending_admin(env.clone()), Some(new_admin));
        });
    }

    #[test]
    fn test_accept_admin_transfers_and_clears_pending() {
        let env = Env::default();
        let (admin, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);
        let new_admin = Address::generate(&env);

        client.propose_admin(&admin, &new_admin);
        client.accept_admin(&new_admin);
        assert_eq!(client.get_admin(), new_admin);
        env.as_contract(&contract_id, || {
            assert_eq!(get_pending_admin(env.clone()), None);
        });
    }

    #[test]
    #[should_panic(expected = "HostError: Error(WasmVm, InvalidAction)")]
    fn test_accept_admin_panics_if_wrong_caller() {
        let env = Env::default();
        let (admin, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);
        let new_admin = Address::generate(&env);
        let rando = Address::generate(&env);

        client.propose_admin(&admin, &new_admin);
        client.accept_admin(&rando);
    }

    #[test]
    #[should_panic(expected = "no pending admin")]
    fn test_accept_admin_panics_if_no_proposal() {
        let env = Env::default();
        let (_, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);
        let rando = Address::generate(&env);

        client.accept_admin(&rando);
    }

    #[test]
    #[should_panic(expected = "not admin")]
    fn test_propose_admin_panics_if_not_admin() {
        let env = Env::default();
        let (_, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);
        let rando = Address::generate(&env);

        client.propose_admin(&rando, &rando);
    }

    #[test]
    fn test_propose_admin_emits_event() {
        let env = Env::default();
        let (admin, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);
        let new_admin = Address::generate(&env);

        client.propose_admin(&admin, &new_admin);
        let events = env.events().all();
        assert!(!events.is_empty());
        let (contract, topics, _) = events.last().unwrap();
        assert_eq!(contract, contract_id);
        assert_eq!(topics, (symbol_short!("synapse"),).into_val(&env));
    }

    #[test]
    fn test_accept_admin_emits_event() {
        let env = Env::default();
        let (admin, contract_id) = setup(&env);
        let client = SynapseContractClient::new(&env, &contract_id);
        let new_admin = Address::generate(&env);

        client.propose_admin(&admin, &new_admin);
        client.accept_admin(&new_admin);
        let events = env.events().all();
        assert!(!events.is_empty());
        let (contract, topics, _) = events.last().unwrap();
        assert_eq!(contract, contract_id);
        assert_eq!(topics, (symbol_short!("synapse"),).into_val(&env));
    }
}
