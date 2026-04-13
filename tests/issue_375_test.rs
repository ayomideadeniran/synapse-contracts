#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Events as _},
    Address, Env, String as SorobanString, TryFromVal,
};
use synapse_contract::{
    types::Event,
    SynapseContract, SynapseContractClient,
};

fn setup(env: &Env) -> (Address, Address, SynapseContractClient<'_>) {
    env.mock_all_auths();
    let id = env.register_contract(None, SynapseContract);
    let client = SynapseContractClient::new(env, &id);
    let admin = Address::generate(env);
    client.initialize(&admin);
    (admin, id, client)
}

fn usd(env: &Env) -> SorobanString {
    SorobanString::from_str(env, "USD")
}

#[test]
fn test_transaction_failed_event_emitted() {
    let env = Env::default();
    let (admin, _, client) = setup(&env);
    let relayer = Address::generate(&env);
    client.grant_relayer(&admin, &relayer);
    client.add_asset(&admin, &usd(&env));

    let tx_id = client.register_deposit(
        &relayer,
        &SorobanString::from_str(&env, "anchor-fail"),
        &Address::generate(&env),
        &100_000_000,
        &usd(&env),
        &None,
        &None,
    );

    let error_reason = SorobanString::from_str(&env, "simulated failure");
    client.mark_failed(&relayer, &tx_id, &error_reason);

    let events = env.events().all();
    
    let mut found_simplified = false;

    for event in events.iter() {
        if let Ok((e, _ledger)) = <(Event, u32)>::try_from_val(&env, &event.2) {
            match e {
                Event::TransactionFailed(id, reason) => {
                    if id == tx_id && reason == error_reason {
                        found_simplified = true;
                    }
                }
                _ => {}
            }
        }
    }

    assert!(found_simplified, "Simplified TransactionFailed event not found");
}
