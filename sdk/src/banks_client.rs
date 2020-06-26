use crate::{
    clock::Slot,
    fee_calculator::FeeCalculator,
    hash::Hash,
    message::Message,
    pubkey::Pubkey,
    signature::{Keypair, Signature, Signer},
    signers::Signers,
    system_instruction,
    transaction::{self, Transaction},
};
use std::io;
use tarpc::context;

#[tarpc::service]
pub trait BanksRpc {
    async fn get_recent_blockhash() -> (Hash, FeeCalculator, Slot);
    async fn send_transaction(transaction: Transaction);
    async fn get_signature_status(signature: Signature) -> Option<transaction::Result<()>>;
    async fn get_root_slot() -> Slot;
    async fn send_and_confirm_transaction(
        transaction: Transaction,
    ) -> Option<transaction::Result<()>>;
    async fn get_balance(pubkey: Pubkey) -> u64;
}

pub struct BanksClient {
    rpc_client: BanksRpcClient,
}

impl BanksClient {
    pub fn new(rpc_client: BanksRpcClient) -> Self {
        Self { rpc_client }
    }

    pub async fn send_message<S: Signers>(
        &mut self,
        signers: &S,
        message: Message,
    ) -> io::Result<(Transaction, u64)> {
        let (recent_blockhash, _fee_calculator, last_valid_slot) = self
            .rpc_client
            .get_recent_blockhash(context::current())
            .await?;
        let transaction = Transaction::new(signers, message, recent_blockhash);
        self
            .rpc_client
            .send_transaction(context::current(), transaction.clone())
            .await?;
        Ok((transaction, last_valid_slot))
    }

    pub async fn send_and_confirm_message<S: Signers>(
        &mut self,
        signers: &S,
        message: Message,
    ) -> io::Result<(Transaction, Option<transaction::Result<()>>)> {
        let (recent_blockhash, _fee_calculator, _last_valid_slot) = self
            .rpc_client
            .get_recent_blockhash(context::current())
            .await?;
        let transaction = Transaction::new(signers, message, recent_blockhash);
        let result = self.rpc_client
            .send_and_confirm_transaction(context::current(), transaction.clone())
            .await?;
        Ok((transaction, result))
    }

    pub async fn transfer_and_confirm(
        &mut self,
        from_keypair: &Keypair,
        to_pubkey: &Pubkey,
        lamports: u64,
    ) -> io::Result<(Transaction, Option<transaction::Result<()>>)> {
        let from_pubkey = from_keypair.pubkey();
        let instruction = system_instruction::transfer(&from_pubkey, &to_pubkey, lamports);
        let message = Message::new(&[instruction], Some(&from_pubkey));
        self.send_and_confirm_message(&[from_keypair], message)
            .await
    }

    pub async fn get_balance(&mut self, pubkey: &Pubkey) -> io::Result<u64> {
        self.rpc_client
            .get_balance(context::current(), *pubkey)
            .await
    }

    pub async fn get_signature_status(
        &mut self,
        signature: &Signature,
    ) -> io::Result<Option<transaction::Result<()>>> {
        self.rpc_client
            .get_signature_status(context::current(), *signature)
            .await
    }

    pub async fn get_root_slot(&mut self) -> io::Result<u64> {
        self.rpc_client.get_root_slot(context::current()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tarpc::{client, transport};

    #[test]
    fn test_banks_rpc_client_new() {
        let (client_transport, _server_transport) = transport::channel::unbounded();
        BanksRpcClient::new(client::Config::default(), client_transport);
    }
}
