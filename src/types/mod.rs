use soroban_sdk::{contracttype, Address, Env, String as SorobanString, Vec};
use crate::alloc::string::ToString;
use alloc::format;

pub const MAX_RETRIES: u32 = 5;

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum TransactionStatus {
    Pending,
    Processing,
    Completed,
    Failed,
    Cancelled,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct Transaction {
    pub id: SorobanString,
    pub anchor_transaction_id: SorobanString,
    pub stellar_account: Address,
    pub relayer: Address,
    pub amount: i128,
    pub asset_code: SorobanString,
    pub memo: Option<SorobanString>,
    pub memo_type: Option<SorobanString>,
    pub callback_type: Option<SorobanString>,
    pub status: TransactionStatus,
    pub created_ledger: u32,
    pub updated_ledger: u32,
    pub settlement_id: SorobanString,
    pub retry_count: u32,
}

impl Transaction {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        env: &Env,
        id: SorobanString,
        anchor_transaction_id: SorobanString,
        stellar_account: Address,
        relayer: Address,
        amount: i128,
        asset_code: SorobanString,
        memo: Option<SorobanString>,
        memo_type: Option<SorobanString>,
        callback_type: Option<SorobanString>,
    ) -> Self {
        let ledger = env.ledger().sequence();
        Self {
            id,
            anchor_transaction_id,
            stellar_account,
            relayer,
            amount,
            asset_code,
            memo,
            memo_type,
            callback_type,
            status: TransactionStatus::Pending,
            created_ledger: ledger,
            updated_ledger: ledger,
            settlement_id: SorobanString::from_str(env, ""),
            retry_count: 0,
        }
    }
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct Settlement {
    pub id: SorobanString,
    pub asset_code: SorobanString,
    pub tx_ids: Vec<SorobanString>,
    pub total_amount: i128,
    pub period_start: u64,
    pub period_end: u64,
    pub created_ledger: u32,
}

impl Settlement {
    pub fn new(
        env: &Env,
        id: SorobanString,
        asset_code: SorobanString,
        tx_ids: Vec<SorobanString>,
        total_amount: i128,
        period_start: u64,
        period_end: u64,
    ) -> Self {
        Self {
            id,
            asset_code,
            tx_ids,
            total_amount,
            period_start,
            period_end,
            created_ledger: env.ledger().sequence(),
        }
    }
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct DlqEntry {
    pub tx_id: SorobanString,
    pub error_reason: SorobanString,
    pub retry_count: u32,
    pub moved_at_ledger: u32,
    pub last_retry_ledger: u32,
}

impl DlqEntry {
    pub fn new(env: &Env, tx_id: SorobanString, error_reason: SorobanString) -> Self {
        Self {
            tx_id,
            error_reason,
            retry_count: 0,
            moved_at_ledger: env.ledger().sequence(),
            last_retry_ledger: 0,
        }
    }
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Initialized(Address),
    AdminTransferred(Address, Address),
    AdminTransferProposed(Address, Address),
    RelayerGranted(Address),
    DepositRegistered(SorobanString, SorobanString),
    StatusUpdated(SorobanString, TransactionStatus, TransactionStatus),
    SettlementFinalized(SorobanString, SorobanString, i128),
    Settled(SorobanString, SorobanString),
    ContractPaused(Address),
    ContractUnpaused(Address),
    RelayerRevoked(Address),
    MovedToDlq(SorobanString, SorobanString),
    DlqRetried(SorobanString),
    MaxRetriesExceeded(SorobanString),
    AssetAdded(SorobanString),
    AssetRemoved(SorobanString),
    TransactionCompleted(SorobanString, Address, i128, SorobanString),
    TransactionFailed(SorobanString, Address, i128, SorobanString, SorobanString),
    TransactionCancelled(SorobanString, Address, i128, SorobanString),
}
