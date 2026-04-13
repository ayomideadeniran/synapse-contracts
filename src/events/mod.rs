use crate::types::Event;
use soroban_sdk::{symbol_short, Address, Env};

// TODO(#67): include caller address in every event for attribution

pub fn emit(env: &Env, caller: &Address, event: Event) {
    let ledger = env.ledger().sequence();
    env.events()
        .publish((symbol_short!("synapse"),), (event, ledger));
}
