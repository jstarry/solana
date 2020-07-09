use solana_core::validator::{TestValidator, TestValidatorOptions};
use solana_sdk::{banks_client::start_tcp_client, native_token::sol_to_lamports};
use solana_tokens::{
    commands::test_process_distribute_tokens_with_client, thin_client::ThinClient,
};
use tokio::runtime::Runtime;

#[test]
fn test_process_distribute_with_rpc_client() {
    let validator = TestValidator::run_with_options(TestValidatorOptions {
        mint_lamports: sol_to_lamports(9_000_000.0),
        ..TestValidatorOptions::default()
    });

    Runtime::new().unwrap().block_on(async {
        let banks_client = start_tcp_client(validator.leader_data.rpc_banks)
            .await
            .unwrap();
        let thin_client = ThinClient::new(banks_client, false);
        test_process_distribute_tokens_with_client(thin_client, validator.alice).await
    });
}
