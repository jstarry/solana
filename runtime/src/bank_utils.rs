use crate::{
    bank::{Bank, TransactionResults},
    genesis_utils::{self, GenesisConfigInfo, ValidatorVoteKeypairs},
    transaction_batch::TransactionBatch,
    vote_sender_types::ReplayVoteSender,
};
use solana_sdk::{pubkey::Pubkey, signature::Signer};
use solana_vote_program::vote_transaction;

pub fn setup_bank_and_vote_pubkeys_for_tests(
    num_vote_accounts: usize,
    stake: u64,
) -> (Bank, Vec<Pubkey>) {
    // Create some voters at genesis
    let validator_voting_keypairs: Vec<_> = (0..num_vote_accounts)
        .map(|_| ValidatorVoteKeypairs::new_rand())
        .collect();

    let vote_pubkeys: Vec<_> = validator_voting_keypairs
        .iter()
        .map(|k| k.vote_keypair.pubkey())
        .collect();
    let GenesisConfigInfo { genesis_config, .. } =
        genesis_utils::create_genesis_config_with_vote_accounts(
            10_000,
            &validator_voting_keypairs,
            vec![stake; validator_voting_keypairs.len()],
        );
    let bank = Bank::new_for_tests(&genesis_config);
    (bank, vote_pubkeys)
}

pub fn find_and_send_votes(
    txs: &TransactionBatch,
    tx_results: &TransactionResults,
    vote_sender: Option<&ReplayVoteSender>,
) {
    let TransactionResults {
        overwritten_vote_accounts,
        ..
    } = tx_results;
    if let Some(vote_sender) = vote_sender {
        for old_account in overwritten_vote_accounts {
            let (tx, execution_result) =
                match txs.get_executed_tx(old_account.transaction_result_index) {
                    Some(executed_tx) => executed_tx,
                    None => continue,
                };

            assert!(execution_result.process_result.is_ok());
            if let Some(parsed_vote) = vote_transaction::parse_sanitized_vote_transaction(tx) {
                if parsed_vote.1.slots.last().is_some() {
                    let _ = vote_sender.send(parsed_vote);
                }
            }
        }
    }
}
