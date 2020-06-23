use crate::{bank::Bank, bank_forks::BankForks};
use futures::{
    future::{self, Future, Ready},
    prelude::stream,
};
use solana_sdk::{
    banks_client::{BanksClient, BanksRpc, BanksRpcClient},
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

impl BanksRpc for BanksServer {
    type GetRecentBlockhashFut = Ready<(Hash, FeeCalculator, Slot)>;
    fn get_recent_blockhash(self, _: Context) -> Self::GetRecentBlockhashFut {
        let bank = self.bank_forks.root_bank();
        let (blockhash, fee_calculator) = bank.last_blockhash_with_fee_calculator();
        let last_valid_slot = bank.get_blockhash_last_valid_slot(&blockhash).unwrap();
        future::ready((blockhash, fee_calculator, last_valid_slot))
    }

    type SendTransactionFut = Ready<Signature>;
    fn send_transaction(self, _: Context, transaction: Transaction) -> Self::SendTransactionFut {
        let signature = transaction.signatures.get(0).cloned().unwrap_or_default();
        self.transaction_sender.send(transaction).unwrap();
        future::ready(signature)
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

    type GetBalanceFut = Ready<u64>;
    fn get_balance(self, _: Context, pubkey: Pubkey) -> Self::GetBalanceFut {
        let bank = self.bank_forks.root_bank();
        future::ready(bank.get_balance(&pubkey))
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

    let banks_rpc_client = BanksRpcClient::new(client::Config::default(), client_transport);
    let rpc_client = runtime.enter(|| banks_rpc_client.spawn())?;
    Ok(BanksClient::new(rpc_client))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genesis_utils::create_genesis_config;
    use solana_sdk::{message::Message, pubkey::Pubkey, signature::Signer, system_instruction};

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

        runtime.block_on(async {
            let status = banks_client
                .transfer(&genesis.mint_keypair, &bob_pubkey, 1)
                .await?;
            assert_eq!(status, Some(Ok(())));
            assert_eq!(banks_client.get_balance(&bob_pubkey).await?, 1);
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
        let message = Message::new_with_payer(&[instruction], Some(&mint_pubkey));

        runtime.block_on(async {
            let (signature, last_valid_slot) = banks_client
                .send_message(&[&genesis.mint_keypair], message)
                .await?;

            let mut status = banks_client.get_signature_status(&signature).await?;
            assert_eq!(status, None, "process_transaction() called synchronously");

            while status.is_none() {
                let root_slot = banks_client.get_root_slot().await?;
                if root_slot > last_valid_slot {
                    break;
                }
                delay_for(Duration::from_millis(100)).await;
                status = banks_client.get_signature_status(&signature).await?;
            }
            assert_eq!(status, Some(Ok(())));
            assert_eq!(banks_client.get_balance(&bob_pubkey).await?, 1);
            Ok(())
        })
    }

    #[test]
    fn test_banks_server_transfer_via_blocking_call() -> io::Result<()> {
        // This test shows how `runtime.block_on()` could be used to implement
        // a fully synchronous interface. Ideally, you'd only use this as a shim
        // to replace some existing synchronous interface.

        let genesis = create_genesis_config(10);
        let bank_forks = Arc::new(BankForks::new(Bank::new(&genesis.genesis_config)));

        let mut runtime = Runtime::new()?;
        let mut banks_client = start_local_server(&mut runtime, &bank_forks)?;

        let bob_pubkey = Pubkey::new_rand();
        let status =
            runtime.block_on(banks_client.transfer(&genesis.mint_keypair, &bob_pubkey, 1))?;
        assert_eq!(status, Some(Ok(())));
        assert_eq!(runtime.block_on(banks_client.get_balance(&bob_pubkey))?, 1);
        Ok(())
    }
}
