use crate::{bank::Bank, bank_forks::BankForks};
use futures::future::{self, Ready};
use solana_sdk::{
    clock::Slot,
    fee_calculator::FeeCalculator,
    hash::Hash,
    signature::Signature,
    transaction::{self, Transaction},
};
use std::{
    sync::{
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
    thread::Builder,
};
use tarpc::context::Context;

#[tarpc::service]
trait BankForksRpc {
    async fn get_recent_blockhash() -> (Hash, FeeCalculator, Slot);
    async fn send_transaction(transaction: Transaction) -> Signature;
    async fn get_signature_status(signature: Signature) -> Option<transaction::Result<()>>;
    async fn get_root_slot() -> Slot;
}

#[derive(Clone)]
pub struct BankForksServer {
    bank_forks: Arc<BankForks>,
    transaction_sender: Sender<Transaction>,
}

impl BankForksServer {
    /// Return a BankForksServer that forwards transactions to the
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

impl BankForksRpc for BankForksServer {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genesis_utils::create_genesis_config;
    use futures::prelude::*;
    use solana_sdk::{message::Message, pubkey::Pubkey, signature::Signer, system_instruction};
    use std::{io, time::Duration};
    use tarpc::{
        client, context,
        server::{self, Handler},
    };

    #[tokio::test]
    async fn test_bank_forks_rpc_client_transfer() -> io::Result<()> {
        let (client_transport, server_transport) = tarpc::transport::channel::unbounded();

        let genesis = create_genesis_config(10);
        let bank = Bank::new(&genesis.genesis_config);
        let bank_forks = Arc::new(BankForks::new(bank));
        let bank_forks_server = BankForksServer::new(bank_forks);
        let server = server::new(server::Config::default())
            .incoming(stream::once(future::ready(server_transport)))
            .respond_with(bank_forks_server.serve());
        tokio::spawn(server);

        let mut client =
            BankForksRpcClient::new(client::Config::default(), client_transport).spawn()?;

        let (recent_blockhash, _fee_calculator, last_valid_slot) =
            client.get_recent_blockhash(context::current()).await?;

        let mint_pubkey = &genesis.mint_keypair.pubkey();
        let bob_pubkey = Pubkey::new_rand();
        let instruction = system_instruction::transfer(&mint_pubkey, &bob_pubkey, 1);
        let message = Message::new_with_payer(&[instruction], Some(&mint_pubkey));
        let transaction = Transaction::new(&[&genesis.mint_keypair], message, recent_blockhash);
        let signature = client
            .send_transaction(context::current(), transaction)
            .await?;

        let mut status = client
            .get_signature_status(context::current(), signature)
            .await?;
        while status.is_none() {
            let root_slot = client.get_root_slot(context::current()).await?;
            if root_slot > last_valid_slot {
                break;
            }
            tokio::time::delay_for(Duration::from_millis(100u64)).await;
            status = client
                .get_signature_status(context::current(), signature)
                .await?;
        }

        assert_eq!(status, Some(Ok(())));

        Ok(())
    }
}
