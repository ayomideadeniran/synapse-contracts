use soroban_sdk::{testutils::Address as _, Address, Env, String as SorobanString};
use synapse_contract::{SynapseContract, SynapseContractClient};

fn setup(env: &Env) -> (Address, Address, SynapseContractClient) {
    env.mock_all_auths();
    let contract_id = env.register_contract(None, SynapseContract);
    let client = SynapseContractClient::new(env, &contract_id);
    let admin = Address::generate(env);
    client.initialize(&admin);
    (admin, contract_id, client)
}

#[test]
fn test_asset_count_and_cap() {
    let env = Env::default();
    let (admin, _contract_id, client) = setup(&env);

    // Initial count should be 0
    assert_eq!(client.asset_count(), 0);

    // Add 2 assets
    client.add_asset(&admin, &SorobanString::from_str(&env, "USD"));
    client.add_asset(&admin, &SorobanString::from_str(&env, "BTC"));
    assert_eq!(client.asset_count(), 2);

    // Default cap is 20. Set it to 3.
    client.set_max_assets(&admin, &3);
    assert_eq!(client.get_max_assets(), 3);

    // Add 3rd asset - should be fine
    client.add_asset(&admin, &SorobanString::from_str(&env, "ETH"));
    assert_eq!(client.asset_count(), 3);

    // Add 4th asset - should panic
    let res = client.try_add_asset(&admin, &SorobanString::from_str(&env, "XLM"));
    assert!(res.is_err());
}

#[test]
fn test_remove_asset_decrements_count() {
    let env = Env::default();
    let (admin, _contract_id, client) = setup(&env);

    let usd = SorobanString::from_str(&env, "USD");
    client.add_asset(&admin, &usd);
    assert_eq!(client.asset_count(), 1);

    client.remove_asset(&admin, &usd);
    assert_eq!(client.asset_count(), 0);
}
