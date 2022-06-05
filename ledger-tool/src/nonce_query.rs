#![allow(clippy::integer_arithmetic)]
use {
    log::*,
    serde::Serialize,
    solana_measure::measure::Measure,
    solana_runtime::bank::Bank,
    solana_sdk::{
        account::ReadableAccount, account_utils::StateMut, hash::Hash, message::SanitizedMessage,
        nonce, pubkey::Pubkey, system_program, transaction::TransactionError,
    },
    solana_storage_bigtable::LedgerStorage,
    solana_transaction_status::ConfirmedTransactionStatusWithSignature,
    std::{
        collections::{BTreeMap, HashMap},
        sync::Arc,
    },
};

pub fn process_command(bank: Arc<Bank>) {
    println!("Finding nonces");
    let mut measure = Measure::start("getting accounts");
    let nonce_blockhashes: BTreeMap<_, _> = bank
        .get_all_accounts_with_modified_slots()
        .unwrap()
        .into_iter()
        .filter_map(|(pubkey, account, _slot)| {
            if !system_program::check_id(account.owner()) {
                return None;
            }

            let blockhash = match StateMut::<nonce::state::Versions>::state(&account)
                .map(|v| v.convert_to_current())
            {
                Ok(nonce::state::State::Initialized(ref data)) => data.blockhash(),
                _ => return None,
            };

            Some((pubkey, blockhash))
        })
        .collect();
    measure.stop();
    info!("{}", measure);

    println!("Found {} nonces", nonce_blockhashes.len());

    let mut measure = Measure::start("printing nonce accounts");
    for (pubkey, blockhash) in nonce_blockhashes.iter() {
        println!("{},{}", pubkey, blockhash);
    }
    measure.stop();
    info!("{}", measure);

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let nonce_transactions = runtime
        .block_on(find_most_recent_durable_transactions(
            nonce_blockhashes.keys(),
        ))
        .unwrap();

    println!("{}", serde_json::to_value(&nonce_transactions).unwrap());
}

#[derive(Serialize)]
struct RecentDurableTransaction {
    blockhash: Hash,
    err: Option<TransactionError>,
}

async fn find_most_recent_durable_transactions(
    nonces: impl Iterator<Item = &Pubkey>,
) -> Result<HashMap<Pubkey, RecentDurableTransaction>, String> {
    let bigtable = LedgerStorage::new(false, None, None).await.unwrap();
    let mut results = HashMap::default();
    for nonce in nonces {
        let most_recent_durable_tx =
            match find_most_recent_durable_transaction(nonce, &bigtable).await {
                Ok(tx) => tx,
                Err(_err) => find_most_recent_durable_transaction(nonce, &bigtable).await?,
            };
        if let Some(most_recent_durable_tx) = most_recent_durable_tx {
            results.insert(*nonce, most_recent_durable_tx);
        }
    }
    Ok(results)
}

async fn find_most_recent_durable_transaction(
    address: &Pubkey,
    bigtable: &LedgerStorage,
) -> Result<Option<RecentDurableTransaction>, String> {
    let results = bigtable
        .get_confirmed_signatures_for_address(address, None, None, 100)
        .await
        .map_err(|_| format!("failed to fetch recent signatures for {}", address))?;

    for (ConfirmedTransactionStatusWithSignature { signature, err, .. }, ..) in results {
        let tx_message = match bigtable.get_confirmed_transaction(&signature).await {
            Ok(Some(confirmed_tx)) => {
                SanitizedMessage::try_from(confirmed_tx.transaction.transaction.message)
                    .map_err(|_| format!("failed to sanitize tx {}", signature))
            }
            _ => Err(format!("failed to find tx {}", signature)),
        }?;

        if Some(address) == tx_message.get_durable_nonce(false) {
            return Ok(Some(RecentDurableTransaction {
                blockhash: *tx_message.recent_blockhash(),
                err,
            }));
        }
    }

    Ok(None)
}
