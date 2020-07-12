use crate::{
    bank::Bank,
    bank_forks::BankForks,
    send_transaction_service::{SendTransactionService, TransactionInfo},
};
use async_tungstenite::{accept_async, tokio::TokioAdapter};
use bincode::{deserialize, serialize};
use futures::{
    future,
    prelude::stream::{self, StreamExt},
};
use solana_sdk::{
    account::Account,
    banks_client::{Banks, BanksClient},
    clock::Slot,
    commitment_config::CommitmentLevel,
    fee_calculator::FeeCalculator,
    hash::Hash,
    pubkey::Pubkey,
    signature::Signature,
    transaction::{self, Transaction},
};
use std::{
    io,
    net::SocketAddr,
    sync::{
        atomic::AtomicBool,
        mpsc::{channel, Receiver, Sender},
        Arc, RwLock,
    },
    thread::Builder,
    time::Duration,
};
use tarpc::{
    client,
    context::Context,
    serde_transport::{tcp, Transport},
    server::{self, Channel, Handler},
    transport,
};
use tokio::time::delay_for;
use tokio_serde::formats::Bincode;
use ws_stream_tungstenite::WsStream;

#[derive(Clone)]
struct BanksService {
    bank_forks: Arc<RwLock<BankForks>>,
    transaction_sender: Sender<TransactionInfo>,
}

impl BanksService {
    /// Return a BanksService that forwards transactions to the
    /// given sender. If unit-testing, those transactions can go to
    /// a bank in the given BankForks. Otherwise, the receiver should
    /// forward them to a validator in the leader schedule.
    fn new_with_sender(
        bank_forks: Arc<RwLock<BankForks>>,
        transaction_sender: Sender<TransactionInfo>,
    ) -> Self {
        Self {
            bank_forks,
            transaction_sender,
        }
    }

    fn run(bank: &Bank, transaction_receiver: Receiver<TransactionInfo>) {
        while let Ok(info) = transaction_receiver.recv() {
            let mut transaction_infos = vec![info];
            while let Ok(info) = transaction_receiver.try_recv() {
                transaction_infos.push(info);
            }
            let transactions: Vec<_> = transaction_infos
                .into_iter()
                .map(|info| deserialize(&info.wire_transaction).unwrap())
                .collect();
            let _ = bank.process_transactions(&transactions);
        }
    }

    /// Useful for unit-testing
    fn new(bank_forks: Arc<RwLock<BankForks>>) -> Self {
        let (transaction_sender, transaction_receiver) = channel();
        let bank = bank_forks.read().unwrap().working_bank();
        Builder::new()
            .name("solana-bank-forks-client".to_string())
            .spawn(move || Self::run(&bank, transaction_receiver))
            .unwrap();
        Self::new_with_sender(bank_forks, transaction_sender)
    }

    fn slot(&self, commitment: CommitmentLevel) -> Slot {
        match commitment {
            CommitmentLevel::Recent => self.bank_forks.read().unwrap().highest_slot(),
            CommitmentLevel::Root => self.bank_forks.read().unwrap().root(),
            CommitmentLevel::Single | CommitmentLevel::SingleGossip => {
                //TODO: self.block_commitment_cache.highest_confirmed_slot()
                todo!();
            }
            CommitmentLevel::Max => {
                //TODO: self.block_commitment_cache.largest_confirmed_root()
                self.bank_forks.read().unwrap().root()
            }
        }
    }

    fn bank(&self, commitment: CommitmentLevel) -> Arc<Bank> {
        self.bank_forks.read().unwrap()[self.slot(commitment)].clone()
    }
}

async fn poll_transaction_status(
    bank: Arc<Bank>,
    signature: Signature,
    last_valid_slot: Slot,
) -> Option<transaction::Result<()>> {
    let mut status = bank.get_signature_status(&signature);
    while status.is_none() {
        if bank.slot() > last_valid_slot {
            break;
        }
        delay_for(Duration::from_millis(100)).await;
        status = bank.get_signature_status(&signature);
    }
    status
}

#[tarpc::server]
impl Banks for BanksService {
    async fn get_fees_with_commitment(
        self,
        _: Context,
        commitment: CommitmentLevel,
    ) -> (FeeCalculator, Hash, Slot) {
        let bank = self.bank(commitment);
        let (blockhash, fee_calculator) = bank.last_blockhash_with_fee_calculator();
        let last_valid_slot = bank.get_blockhash_last_valid_slot(&blockhash).unwrap();
        (fee_calculator, blockhash, last_valid_slot)
    }

    async fn send_transaction(self, _: Context, transaction: Transaction) {
        let blockhash = &transaction.message.recent_blockhash;
        let last_valid_slot = self
            .bank_forks
            .read()
            .unwrap()
            .root_bank()
            .get_blockhash_last_valid_slot(&blockhash)
            .unwrap();
        let signature = transaction.signatures.get(0).cloned().unwrap_or_default();
        let info =
            TransactionInfo::new(signature, serialize(&transaction).unwrap(), last_valid_slot);
        self.transaction_sender.send(info).unwrap();
    }

    async fn get_signature_status_with_commitment(
        self,
        _: Context,
        signature: Signature,
        commitment: CommitmentLevel,
    ) -> Option<transaction::Result<()>> {
        let bank = self.bank(commitment);
        bank.get_signature_status(&signature)
    }

    async fn get_signature_statuses_with_commitment(
        self,
        _: Context,
        signatures: Vec<Signature>,
        commitment: CommitmentLevel,
    ) -> Vec<Option<transaction::Result<()>>> {
        let bank = self.bank(commitment);
        signatures
            .iter()
            .map(|x| bank.get_signature_status(x))
            .collect()
    }

    async fn get_slot(self, _: Context, commitment: CommitmentLevel) -> Slot {
        self.slot(commitment)
    }

    async fn send_and_confirm_transaction(
        self,
        _: Context,
        transaction: Transaction,
        commitment: CommitmentLevel,
    ) -> Option<transaction::Result<()>> {
        let blockhash = &transaction.message.recent_blockhash;
        let last_valid_slot = self
            .bank_forks
            .read()
            .unwrap()
            .root_bank()
            .get_blockhash_last_valid_slot(&blockhash)
            .unwrap();
        let signature = transaction.signatures.get(0).cloned().unwrap_or_default();
        let info =
            TransactionInfo::new(signature, serialize(&transaction).unwrap(), last_valid_slot);
        self.transaction_sender.send(info).unwrap();
        let bank = self.bank(commitment);
        poll_transaction_status(bank, signature, last_valid_slot).await
    }

    async fn get_account_with_commitment(
        self,
        _: Context,
        pubkey: Pubkey,
        _commitment: CommitmentLevel,
    ) -> Option<Account> {
        self.bank_forks
            .read()
            .unwrap()
            .root_bank()
            .get_account(&pubkey)
    }
}

pub async fn start_local_service(bank_forks: &Arc<RwLock<BankForks>>) -> io::Result<BanksClient> {
    let banks_service = BanksService::new(bank_forks.clone());
    let (client_transport, server_transport) = transport::channel::unbounded();
    let server = server::new(server::Config::default())
        .incoming(stream::once(future::ready(server_transport)))
        .respond_with(banks_service.serve());
    tokio::spawn(server);

    let banks_client = BanksClient::new(client::Config::default(), client_transport);
    banks_client.spawn()
}

mod ws {
    use {
        futures::{prelude::*, ready, task::*},
        pin_project::pin_project,
        std::{io, net::SocketAddr, pin::Pin},
        tokio::net::{TcpListener, TcpStream, ToSocketAddrs},
    };

    /// Listens on `addr`, wrapping accepted connections in JSON transports.
    pub async fn listen<A>(addr: A) -> io::Result<Incoming>
    where
        A: ToSocketAddrs,
    {
        let listener = TcpListener::bind(addr).await?;
        let local_addr = listener.local_addr()?;
        Ok(Incoming {
            listener,
            local_addr,
        })
    }

    /// A [`TcpListener`] that wraps connections in JSON transports.
    #[pin_project]
    #[derive(Debug)]
    pub struct Incoming {
        listener: TcpListener,
        local_addr: SocketAddr,
    }

    impl Stream for Incoming {
        type Item = io::Result<TcpStream>;

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let next =
                ready!(Pin::new(&mut self.as_mut().project().listener.incoming()).poll_next(cx)?);
            Poll::Ready(next.map(|conn| Ok(conn)))
        }
    }
}

pub async fn start_ws_service(
    listen_addr: SocketAddr,
    tpu_addr: SocketAddr,
    bank_forks: Arc<RwLock<BankForks>>,
) -> io::Result<()> {
    // Note: These settings are copied straight from the tarpc example.
    let service = ws::listen(listen_addr)
        .await?
        // Ignore accept errors.
        .filter_map(|r| future::ready(r.ok()))
        .then(|tcp_stream| accept_async(TokioAdapter(tcp_stream)))
        .filter_map(|r| future::ready(r.ok()))
        .map(|stream| Transport::from((WsStream::new(stream), Bincode::default())))
        .map(server::BaseChannel::with_defaults)
        // Limit channels to 1 per IP.
        // .max_channels_per_key(1, |t| t.as_ref().peer_addr().unwrap().ip())
        // serve is generated by the service attribute. It takes as input any type implementing
        // the generated Banks trait.
        .map(move |chan| {
            let (sender, receiver) = channel();
            let exit_send_transaction_service = Arc::new(AtomicBool::new(false));

            SendTransactionService::new(
                tpu_addr,
                &bank_forks,
                &exit_send_transaction_service,
                receiver,
            );

            let service = BanksService::new_with_sender(bank_forks.clone(), sender);
            chan.respond_with(service.serve()).execute()
        })
        // Max 10 channels.
        .buffer_unordered(10)
        .for_each(|_| async {});

    service.await;
    Ok(())
}

pub async fn start_tcp_service(
    listen_addr: SocketAddr,
    tpu_addr: SocketAddr,
    bank_forks: Arc<RwLock<BankForks>>,
) -> io::Result<()> {
    // Note: These settings are copied straight from the tarpc example.
    let service = tcp::listen(listen_addr, Bincode::default)
        .await?
        // Ignore accept errors.
        .filter_map(|r| future::ready(r.ok()))
        .map(server::BaseChannel::with_defaults)
        // Limit channels to 1 per IP.
        .max_channels_per_key(1, |t| t.as_ref().peer_addr().unwrap().ip())
        // serve is generated by the service attribute. It takes as input any type implementing
        // the generated Banks trait.
        .map(move |chan| {
            let (sender, receiver) = channel();
            let exit_send_transaction_service = Arc::new(AtomicBool::new(false));

            SendTransactionService::new(
                tpu_addr,
                &bank_forks,
                &exit_send_transaction_service,
                receiver,
            );

            let service = BanksService::new_with_sender(bank_forks.clone(), sender);
            chan.respond_with(service.serve()).execute()
        })
        // Max 10 channels.
        .buffer_unordered(10)
        .for_each(|_| async {});

    service.await;
    Ok(())
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
        let bank_forks = Arc::new(RwLock::new(BankForks::new(Bank::new(
            &genesis.genesis_config,
        ))));

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
        let bank_forks = Arc::new(RwLock::new(BankForks::new(Bank::new(
            &genesis.genesis_config,
        ))));

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
