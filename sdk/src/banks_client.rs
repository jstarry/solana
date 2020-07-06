use crate::{
    account::Account,
    clock::Slot,
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
    async fn get_fees() -> (FeeCalculator, Hash, Slot);
    async fn send_transaction(transaction: Transaction);
    async fn get_signature_status(signature: Signature) -> Option<transaction::Result<()>>;
    async fn get_signature_statuses(
        signatures: Vec<Signature>,
    ) -> Vec<Option<transaction::Result<()>>>;
    async fn get_root_slot() -> Slot;
    async fn send_and_confirm_transaction(
        transaction: Transaction,
    ) -> Option<transaction::Result<()>>;
    async fn get_account(pubkey: Pubkey) -> Option<Account>;
}

#[async_trait]
pub trait BanksClientExt {
    async fn get_recent_blockhash(&mut self, _: Context) -> io::Result<Hash>;
    async fn process_transaction(
        &mut self,
        _: Context,
        transaction: Transaction,
    ) -> transport::Result<()>;
    async fn get_balance(&mut self, _: Context, pubkey: Pubkey) -> io::Result<u64>;
}

#[async_trait]
impl BanksClientExt for BanksClient {
    async fn get_recent_blockhash(&mut self, context: Context) -> io::Result<Hash> {
        Ok(self.get_fees(context).await?.1)
    }

    async fn process_transaction(
        &mut self,
        context: Context,
        transaction: Transaction,
    ) -> transport::Result<()> {
        let result = self
            .send_and_confirm_transaction(context, transaction)
            .await?;
        match result {
            None => Err(Error::new(ErrorKind::TimedOut, "invalid blockhash or fee-payer").into()),
            Some(transaction_result) => Ok(transaction_result?),
        }
    }

    async fn get_balance(&mut self, context: Context, pubkey: Pubkey) -> io::Result<u64> {
        let account = self.get_account(context, pubkey).await?;
        Ok(account.map(|x| x.lamports).unwrap_or(0))
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
