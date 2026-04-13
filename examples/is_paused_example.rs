// Example demonstrating the is_paused() query endpoint
use soroban_sdk::{Address, Env};

// This example shows how external systems can check the pause state
// before submitting transactions to the Synapse contract
fn main() {
    println!("Example: Checking contract pause state");
    println!("External systems should call is_paused() before submitting transactions");
    println!("Returns: true if paused, false if active");
}

#[cfg(test)]
mod example_tests {
    use soroban_sdk::{testutils::Address as _, Env};
    use synapse_contract::{SynapseContract, SynapseContractClient};

    #[test]
    fn example_pause_check() {
        let env = Env::default();
        env.mock_all_auths();

        // Deploy contract
        let contract_id = env.register_contract(None, SynapseContract);
        let client = SynapseContractClient::new(&env, &contract_id);

        // Initialize with admin
        let admin = Address::generate(&env);
        client.initialize(&admin);

        // Check initial state - should not be paused
        assert!(
            !client.is_paused(),
            "Contract should not be paused initially"
        );

        // Admin pauses the contract
        client.pause(&admin);

        // External system checks pause state before submitting transaction
        if client.is_paused() {
            println!("Contract is paused - transaction submission blocked");
            assert!(true, "Correctly detected paused state");
        } else {
            panic!("Should have detected paused state");
        }

        // Admin unpauses the contract
        client.unpause(&admin);

        // External system can now proceed with transactions
        assert!(
            !client.is_paused(),
            "Contract should be active after unpause"
        );
    }
}
