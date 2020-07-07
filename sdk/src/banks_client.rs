use crate::{
    account::Account,
    clock::Slot,
    commitment_config::CommitmentLevel,
    fee_calculator::FeeCalculator,
    hash::Hash,
    pubkey::Pubkey,
    signature::Signature,
    transaction::{self, Transaction},
    transport,
};
use async_trait::async_trait;
use std::io::{self, Error, ErrorKind};
use tarpc::{client, context::Context, serde_transport::tcp};
use tokio::net::ToSocketAddrs;
use tokio_serde::formats::Bincode;

#[tarpc::service]
pub trait Banks {
    async fn get_fees_with_commitment(commitement: CommitmentLevel) -> (FeeCalculator, Hash, Slot);
    async fn send_transaction(transaction: Transaction);
    async fn get_signature_status_with_commitment(
        signature: Signature,
        commitement: CommitmentLevel,
    ) -> Option<transaction::Result<()>>;
    async fn get_signature_statuses_with_commitment(
        signatures: Vec<Signature>,
        commitment: CommitmentLevel,
    ) -> Vec<Option<transaction::Result<()>>>;
    async fn get_slot(commitment: CommitmentLevel) -> Slot;
    async fn send_and_confirm_transaction(
        transaction: Transaction,
        commitment: CommitmentLevel,
    ) -> Option<transaction::Result<()>>;
    async fn get_account_with_commitment(
        pubkey: Pubkey,
        commitment: CommitmentLevel,
    ) -> Option<Account>;
}

#[async_trait]
pub trait BanksClientExt {
    async fn get_recent_blockhash(&mut self, _: Context) -> io::Result<Hash>;
    async fn process_transaction_with_commitment(
        &mut self,
        _: Context,
        transaction: Transaction,
        commitment: CommitmentLevel,
    ) -> transport::Result<()>;
    async fn process_transaction(
        &mut self,
        _: Context,
        transaction: Transaction,
    ) -> transport::Result<()>;
    async fn get_fees(&mut self, _: Context) -> io::Result<(FeeCalculator, Hash, Slot)>;
    async fn get_signature_status(
        &mut self,
        _: Context,
        signature: Signature,
    ) -> io::Result<Option<transaction::Result<()>>>;
    async fn get_signature_statuses(
        &mut self,
        _: Context,
        signatures: Vec<Signature>,
    ) -> io::Result<Vec<Option<transaction::Result<()>>>>;
    async fn get_root_slot(&mut self, _: Context) -> io::Result<Slot>;
    async fn get_account(&mut self, _: Context, pubkey: Pubkey) -> io::Result<Option<Account>>;
    async fn get_balance_with_commitment(
        &mut self,
        _: Context,
        pubkey: Pubkey,
        commitment: CommitmentLevel,
    ) -> io::Result<u64>;
    async fn get_balance(&mut self, _: Context, pubkey: Pubkey) -> io::Result<u64>;
}

#[async_trait]
impl BanksClientExt for BanksClient {
    async fn get_fees(&mut self, context: Context) -> io::Result<(FeeCalculator, Hash, Slot)> {
        self.get_fees_with_commitment(context, CommitmentLevel::Root)
            .await
    }

    async fn get_recent_blockhash(&mut self, context: Context) -> io::Result<Hash> {
        Ok(self.get_fees(context).await?.1)
    }

    async fn process_transaction_with_commitment(
        &mut self,
        context: Context,
        transaction: Transaction,
        commitment: CommitmentLevel,
    ) -> transport::Result<()> {
        let result = self
            .send_and_confirm_transaction(context, transaction, commitment)
            .await?;
        match result {
            None => Err(Error::new(ErrorKind::TimedOut, "invalid blockhash or fee-payer").into()),
            Some(transaction_result) => Ok(transaction_result?),
        }
    }

    async fn process_transaction(
        &mut self,
        context: Context,
        transaction: Transaction,
    ) -> transport::Result<()> {
        self.process_transaction_with_commitment(context, transaction, CommitmentLevel::default())
            .await
    }

    async fn get_root_slot(&mut self, context: Context) -> io::Result<Slot> {
        self.get_slot(context, CommitmentLevel::Root).await
    }

    async fn get_account(
        &mut self,
        context: Context,
        pubkey: Pubkey,
    ) -> io::Result<Option<Account>> {
        self.get_account_with_commitment(context, pubkey, CommitmentLevel::default())
            .await
    }

    async fn get_balance_with_commitment(
        &mut self,
        context: Context,
        pubkey: Pubkey,
        commitment: CommitmentLevel,
    ) -> io::Result<u64> {
        let account = self
            .get_account_with_commitment(context, pubkey, commitment)
            .await?;
        Ok(account.map(|x| x.lamports).unwrap_or(0))
    }

    async fn get_balance(&mut self, context: Context, pubkey: Pubkey) -> io::Result<u64> {
        self.get_balance_with_commitment(context, pubkey, CommitmentLevel::default())
            .await
    }

    async fn get_signature_status(
        &mut self,
        context: Context,
        signature: Signature,
    ) -> io::Result<Option<transaction::Result<()>>> {
        self.get_signature_status_with_commitment(context, signature, CommitmentLevel::default())
            .await
    }

    async fn get_signature_statuses(
        &mut self,
        context: Context,
        signatures: Vec<Signature>,
    ) -> io::Result<Vec<Option<transaction::Result<()>>>> {
        self.get_signature_statuses_with_commitment(context, signatures, CommitmentLevel::default())
            .await
    }
}

pub async fn start_tcp_client<T: ToSocketAddrs>(addr: T) -> io::Result<BanksClient> {
    let transport = tcp::connect(addr, Bincode::default()).await?;
    BanksClient::new(client::Config::default(), transport).spawn()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tarpc::{client, transport};

    #[test]
    fn test_banks_client_new() {
        let (client_transport, _server_transport) = transport::channel::unbounded();
        BanksClient::new(client::Config::default(), client_transport);
    }
}
