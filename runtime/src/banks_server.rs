use crate::{bank::Bank, bank_forks::BankForks};
use futures::{
    future::{self, Future, Ready},
    prelude::stream,
};
use solana_sdk::{
    account::Account,
    banks_client::{Banks, BanksClient},
    clock::Slot,
    fee_calculator::FeeCalculator,
    hash::Hash,
    pubkey::Pubkey,
    signature::Signature,
    transaction::{self, Transaction},
};
use std::{
    io,
    pin::Pin,
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
    server::{self, Handler},
    transport,
};
use tokio::{runtime::Runtime, time::delay_for};

#[derive(Clone)]
pub struct BanksServer {
    bank_forks: Arc<BankForks>,
    transaction_sender: Sender<Transaction>,
}

impl BanksServer {
    /// Return a BanksServer that forwards transactions to the
    /// given sender. If unit-testing, those transactions can go to
    /// a bank in the given BankForks. Otherwise, the receiver should
    /// forward them to a validator in the leader schedule.
    pub fn new_with_sender(
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
    pub fn new(bank_forks: Arc<BankForks>) -> Self {
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

impl Banks for BanksServer {
    type GetFeesFut = Ready<(FeeCalculator, Hash, Slot)>;
    fn get_fees(self, _: Context) -> Self::GetFeesFut {
        let bank = self.bank_forks.root_bank();
        let (blockhash, fee_calculator) = bank.last_blockhash_with_fee_calculator();
        let last_valid_slot = bank.get_blockhash_last_valid_slot(&blockhash).unwrap();
        future::ready((fee_calculator, blockhash, last_valid_slot))
    }

    type SendTransactionFut = Ready<()>;
    fn send_transaction(self, _: Context, transaction: Transaction) -> Self::SendTransactionFut {
        self.transaction_sender.send(transaction).unwrap();
        future::ready(())
    }

    type GetSignatureStatusFut = Ready<Option<transaction::Result<()>>>;
    fn get_signature_status(self, _: Context, signature: Signature) -> Self::GetSignatureStatusFut {
        let bank = self.bank_forks.root_bank();
        future::ready(bank.get_signature_status(&signature))
    }

    type GetRootSlotFut = Ready<Slot>;
    fn get_root_slot(self, _: Context) -> Self::GetRootSlotFut {
        future::ready(self.bank_forks.root())
    }

    type SendAndConfirmTransactionFut =
        Pin<Box<dyn Future<Output = Option<transaction::Result<()>>> + Send>>;
    fn send_and_confirm_transaction(
        self,
        _: Context,
        transaction: Transaction,
    ) -> Self::SendAndConfirmTransactionFut {
        let blockhash = &transaction.message.recent_blockhash;
        let root_bank = self.bank_forks.root_bank();
        let last_valid_slot = root_bank.get_blockhash_last_valid_slot(&blockhash).unwrap();
        let signature = transaction.signatures.get(0).cloned().unwrap_or_default();
        self.transaction_sender.send(transaction).unwrap();
        let status = poll_transaction_status(root_bank.clone(), signature, last_valid_slot);
        Box::pin(status)
    }

    type GetAccountFut = Ready<Option<Account>>;
    fn get_account(self, _: Context, pubkey: Pubkey) -> Self::GetAccountFut {
        let bank = self.bank_forks.root_bank();
        future::ready(bank.get_account(&pubkey))
    }
}

pub fn start_local_server(
    runtime: &mut Runtime,
    bank_forks: &Arc<BankForks>,
) -> io::Result<BanksClient> {
    let banks_server = BanksServer::new(bank_forks.clone());
    let (client_transport, server_transport) = transport::channel::unbounded();
    let server = server::new(server::Config::default())
        .incoming(stream::once(future::ready(server_transport)))
        .respond_with(banks_server.serve());
    runtime.spawn(server);

    let banks_client = BanksClient::new(client::Config::default(), client_transport);
    runtime.enter(|| banks_client.spawn())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genesis_utils::create_genesis_config;
    use solana_sdk::{
        banks_client::get_balance, message::Message, pubkey::Pubkey, signature::Signer,
        system_instruction,
    };
    use tarpc::context;

    #[test]
    fn test_banks_server_transfer_via_server() -> io::Result<()> {
        // This test shows the preferred way to interact with BanksServer.
        // It creates a runtime explicitly (no globals via tokio macros) and calls
        // `runtime.block_on()` just once, to run all the async code.

        let genesis = create_genesis_config(10);
        let bank_forks = Arc::new(BankForks::new(Bank::new(&genesis.genesis_config)));
        let mut runtime = Runtime::new()?;
        let mut banks_client = start_local_server(&mut runtime, &bank_forks)?;

        let bob_pubkey = Pubkey::new_rand();
        let mint_pubkey = genesis.mint_keypair.pubkey();
        let instruction = system_instruction::transfer(&mint_pubkey, &bob_pubkey, 1);
        let message = Message::new(&[instruction], Some(&mint_pubkey));

        runtime.block_on(async {
            let recent_blockhash = banks_client.get_fees(context::current()).await?.1;
            let transaction = Transaction::new(&[&genesis.mint_keypair], message, recent_blockhash);
            let status = banks_client
                .send_and_confirm_transaction(context::current(), transaction)
                .await?;
            assert_eq!(status, Some(Ok(())));
            assert_eq!(get_balance(&mut banks_client, bob_pubkey).await?, 1);
            Ok(())
        })
    }

    #[test]
    fn test_banks_server_transfer_via_client() -> io::Result<()> {
        // The caller may not want to hold the connection open until the transaction
        // is processed (or blockhash expires). In this test, we verify the
        // server-side functionality is available to the client.

        let genesis = create_genesis_config(10);
        let bank_forks = Arc::new(BankForks::new(Bank::new(&genesis.genesis_config)));
        let mut runtime = Runtime::new()?;
        let mut banks_client = start_local_server(&mut runtime, &bank_forks)?;

        let mint_pubkey = &genesis.mint_keypair.pubkey();
        let bob_pubkey = Pubkey::new_rand();
        let instruction = system_instruction::transfer(&mint_pubkey, &bob_pubkey, 1);
        let message = Message::new(&[instruction], Some(&mint_pubkey));

        runtime.block_on(async {
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
            assert_eq!(get_balance(&mut banks_client, bob_pubkey).await?, 1);
            Ok(())
        })
    }
}
