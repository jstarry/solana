use crate::{bank::Bank, bank_forks::BankForks};
use futures::{
    future,
    prelude::stream::{self, StreamExt},
};
use solana_sdk::{
    account::Account,
    banks_client::{start_tcp_client, Banks, BanksClient},
    clock::Slot,
    fee_calculator::FeeCalculator,
    hash::Hash,
    pubkey::Pubkey,
    signature::Signature,
    transaction::{self, Transaction},
};
use std::{
    io,
    sync::{
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
    thread::Builder,
    time::Duration,
};
use tarpc::{
    client,
    context::Context,
    serde_transport::tcp,
    server::{self, Channel, Handler},
    transport,
};
use tokio::time::delay_for;
use tokio_serde::formats::Bincode;

#[derive(Clone)]
struct BanksService {
    bank_forks: Arc<BankForks>,
    transaction_sender: Sender<Transaction>,
}

impl BanksService {
    /// Return a BanksService that forwards transactions to the
    /// given sender. If unit-testing, those transactions can go to
    /// a bank in the given BankForks. Otherwise, the receiver should
    /// forward them to a validator in the leader schedule.
    fn new_with_sender(
        bank_forks: Arc<BankForks>,
        transaction_sender: Sender<Transaction>,
    ) -> Self {
        Self {
            bank_forks,
            transaction_sender,
        }
    }

    fn run(bank: &Bank, transaction_receiver: Receiver<Transaction>) {
        while let Ok(tx) = transaction_receiver.recv() {
            let mut transactions = vec![tx];
            while let Ok(tx) = transaction_receiver.try_recv() {
                transactions.push(tx);
            }
            let _ = bank.process_transactions(&transactions);
        }
    }

    /// Useful for unit-testing
    fn new(bank_forks: Arc<BankForks>) -> Self {
        let (transaction_sender, transaction_receiver) = channel();
        let bank = bank_forks.working_bank();
        Builder::new()
            .name("solana-bank-forks-client".to_string())
            .spawn(move || Self::run(&bank, transaction_receiver))
            .unwrap();
        Self::new_with_sender(bank_forks, transaction_sender)
    }
}

async fn poll_transaction_status(
    root_bank: Arc<Bank>,
    signature: Signature,
    last_valid_slot: Slot,
) -> Option<transaction::Result<()>> {
    let mut status = root_bank.get_signature_status(&signature);
    while status.is_none() {
        let root_slot = root_bank.slot();
        if root_slot > last_valid_slot {
            break;
        }
        delay_for(Duration::from_millis(100)).await;
        status = root_bank.get_signature_status(&signature);
    }
    status
}

#[tarpc::server]
impl Banks for BanksService {
    async fn get_fees(self, _: Context) -> (FeeCalculator, Hash, Slot) {
        let bank = self.bank_forks.root_bank();
        let (blockhash, fee_calculator) = bank.last_blockhash_with_fee_calculator();
        let last_valid_slot = bank.get_blockhash_last_valid_slot(&blockhash).unwrap();
        (fee_calculator, blockhash, last_valid_slot)
    }

    async fn send_transaction(self, _: Context, transaction: Transaction) {
        self.transaction_sender.send(transaction).unwrap();
    }

    async fn get_signature_status(
        self,
        _: Context,
        signature: Signature,
    ) -> Option<transaction::Result<()>> {
        let bank = self.bank_forks.root_bank();
        bank.get_signature_status(&signature)
    }

    async fn get_signature_statuses(
        self,
        _: Context,
        signatures: Vec<Signature>,
    ) -> Vec<Option<transaction::Result<()>>> {
        let bank = self.bank_forks.root_bank();
        signatures
            .iter()
            .map(|x| bank.get_signature_status(x))
            .collect()
    }

    async fn get_root_slot(self, _: Context) -> Slot {
        self.bank_forks.root()
    }

    async fn send_and_confirm_transaction(
        self,
        _: Context,
        transaction: Transaction,
    ) -> Option<transaction::Result<()>> {
        let blockhash = &transaction.message.recent_blockhash;
        let root_bank = self.bank_forks.root_bank();
        let last_valid_slot = root_bank.get_blockhash_last_valid_slot(&blockhash).unwrap();
        let signature = transaction.signatures.get(0).cloned().unwrap_or_default();
        self.transaction_sender.send(transaction).unwrap();
        poll_transaction_status(root_bank.clone(), signature, last_valid_slot).await
    }

    async fn get_account(self, _: Context, pubkey: Pubkey) -> Option<Account> {
        let bank = self.bank_forks.root_bank();
        bank.get_account(&pubkey)
    }
}

pub async fn start_local_service(bank_forks: &Arc<BankForks>) -> io::Result<BanksClient> {
    let banks_service = BanksService::new(bank_forks.clone());
    let (client_transport, server_transport) = transport::channel::unbounded();
    let server = server::new(server::Config::default())
        .incoming(stream::once(future::ready(server_transport)))
        .respond_with(banks_service.serve());
    tokio::spawn(server);

    let banks_client = BanksClient::new(client::Config::default(), client_transport);
    banks_client.spawn()
}

pub async fn start_local_tcp_service(bank_forks: Arc<BankForks>) -> io::Result<BanksClient> {
    let incoming = tcp::listen(&"localhost:0", Bincode::default)
        .await?
        // Ignore accept errors.
        .filter_map(|r| future::ready(r.ok()));

    let addr = incoming.get_ref().local_addr();

    // Note: These settings are copied straight from the tarpc example.
    let service = incoming
        .map(server::BaseChannel::with_defaults)
        // Limit channels to 1 per IP.
        .max_channels_per_key(1, |t| t.as_ref().peer_addr().unwrap().ip())
        // serve is generated by the service attribute. It takes as input any type implementing
        // the generated Banks trait.
        .map(move |channel| {
            let service = BanksService::new(bank_forks.clone());
            channel.respond_with(service.serve()).execute()
        })
        // Max 10 channels.
        .buffer_unordered(10)
        .for_each(|_| async {});

    tokio::spawn(service);

    start_tcp_client(addr).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genesis_utils::create_genesis_config;
    use solana_sdk::{
        banks_client::BanksClientExt, message::Message, pubkey::Pubkey, signature::Signer,
        system_instruction,
    };
    use tarpc::context;
    use tokio::runtime::Runtime;

    #[test]
    fn test_banks_service_transfer_via_service() -> io::Result<()> {
        // This test shows the preferred way to interact with BanksService.
        // It creates a runtime explicitly (no globals via tokio macros) and calls
        // `runtime.block_on()` just once, to run all the async code.

        let genesis = create_genesis_config(10);
        let bank_forks = Arc::new(BankForks::new(Bank::new(&genesis.genesis_config)));

        let bob_pubkey = Pubkey::new_rand();
        let mint_pubkey = genesis.mint_keypair.pubkey();
        let instruction = system_instruction::transfer(&mint_pubkey, &bob_pubkey, 1);
        let message = Message::new(&[instruction], Some(&mint_pubkey));

        Runtime::new()?.block_on(async {
            let mut banks_client = start_local_service(&bank_forks).await?;
            let recent_blockhash = banks_client
                .get_recent_blockhash(context::current())
                .await?;
            let transaction = Transaction::new(&[&genesis.mint_keypair], message, recent_blockhash);
            banks_client
                .process_transaction(context::current(), transaction)
                .await
                .unwrap();
            assert_eq!(
                banks_client
                    .get_balance(context::current(), bob_pubkey)
                    .await?,
                1
            );
            Ok(())
        })
    }

    #[test]
    fn test_banks_service_transfer_via_client() -> io::Result<()> {
        // The caller may not want to hold the connection open until the transaction
        // is processed (or blockhash expires). In this test, we verify the
        // server-side functionality is available to the client.

        let genesis = create_genesis_config(10);
        let bank_forks = Arc::new(BankForks::new(Bank::new(&genesis.genesis_config)));

        let mint_pubkey = &genesis.mint_keypair.pubkey();
        let bob_pubkey = Pubkey::new_rand();
        let instruction = system_instruction::transfer(&mint_pubkey, &bob_pubkey, 1);
        let message = Message::new(&[instruction], Some(&mint_pubkey));

        Runtime::new()?.block_on(async {
            let mut banks_client = start_local_service(&bank_forks).await?;
            let (_, recent_blockhash, last_valid_slot) =
                banks_client.get_fees(context::current()).await?;
            let transaction = Transaction::new(&[&genesis.mint_keypair], message, recent_blockhash);
            let signature = transaction.signatures[0];
            banks_client
                .send_transaction(context::current(), transaction)
                .await?;

            let mut status = banks_client
                .get_signature_status(context::current(), signature)
                .await?;
            assert_eq!(status, None, "process_transaction() called synchronously");

            while status.is_none() {
                let root_slot = banks_client.get_root_slot(context::current()).await?;
                if root_slot > last_valid_slot {
                    break;
                }
                delay_for(Duration::from_millis(100)).await;
                status = banks_client
                    .get_signature_status(context::current(), signature)
                    .await?;
            }
            assert_eq!(status, Some(Ok(())));
            assert_eq!(
                banks_client
                    .get_balance(context::current(), bob_pubkey)
                    .await?,
                1
            );
            Ok(())
        })
    }
}
