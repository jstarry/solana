use solana_runtime::{
    bank::Bank, bank_forks::BankForks, banks_service::start_local_tcp_service,
    genesis_utils::create_genesis_config,
};
use solana_sdk::native_token::sol_to_lamports;
use solana_tokens::{
    commands::test_process_distribute_tokens_with_client, thin_client::ThinClient,
};
use std::sync::Arc;
use tokio::runtime::Runtime;

#[test]
fn test_process_distribute_with_rpc_client() {
    let genesis = create_genesis_config(sol_to_lamports(9_000_000.0));
    let bank_forks = Arc::new(BankForks::new(Bank::new(&genesis.genesis_config)));

    let mut runtime = Runtime::new().unwrap();
    let banks_client = runtime
        .block_on(start_local_tcp_service(bank_forks))
        .unwrap();

    let thin_client = ThinClient::new(banks_client, false);
    runtime.block_on(test_process_distribute_tokens_with_client(
        thin_client,
        genesis.mint_keypair,
    ));
}
