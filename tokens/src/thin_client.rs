use solana_client::{rpc_client::RpcClient, rpc_config::RpcSendTransactionConfig};
use solana_runtime::bank_client::BankClient;
use solana_sdk::{
    account::Account,
    banks_client::{get_balance, BanksClient},
    client::{AsyncClient, SyncClient},
    clock::Slot,
    commitment_config::CommitmentConfig,
    fee_calculator::FeeCalculator,
    hash::Hash,
    message::Message,
    pubkey::Pubkey,
    signature::{Signature, Signer},
    signers::Signers,
    system_instruction,
    transaction::Transaction,
    transport::{Result, TransportError},
};
use solana_transaction_status::TransactionStatus;
use tarpc::context;
use tokio::runtime::Runtime;

pub trait Client {
    fn send_transaction1(&mut self, transaction: Transaction) -> Result<Signature>;
    fn get_signature_statuses1(
        &mut self,
        signatures: &[Signature],
    ) -> Result<Vec<Option<TransactionStatus>>>;
    fn get_balance1(&mut self, pubkey: &Pubkey) -> Result<u64>;
    fn get_fees1(&mut self) -> Result<(Hash, FeeCalculator, Slot)>;
    fn get_slot1(&mut self) -> Result<Slot>;
    fn get_account1(&mut self, pubkey: &Pubkey) -> Result<Option<Account>>;
}

impl Client for RpcClient {
    fn send_transaction1(&mut self, transaction: Transaction) -> Result<Signature> {
        self.send_transaction_with_config(
            &transaction,
            RpcSendTransactionConfig {
                skip_preflight: true,
            },
        )
        .map_err(|e| TransportError::Custom(e.to_string()))
    }

    fn get_signature_statuses1(
        &mut self,
        signatures: &[Signature],
    ) -> Result<Vec<Option<TransactionStatus>>> {
        self.get_signature_statuses(signatures)
            .map(|response| response.value)
            .map_err(|e| TransportError::Custom(e.to_string()))
    }

    fn get_balance1(&mut self, pubkey: &Pubkey) -> Result<u64> {
        self.get_balance(pubkey)
            .map_err(|e| TransportError::Custom(e.to_string()))
    }

    fn get_fees1(&mut self) -> Result<(Hash, FeeCalculator, Slot)> {
        let result = self
            .get_recent_blockhash_with_commitment(CommitmentConfig::default())
            .map_err(|e| TransportError::Custom(e.to_string()))?;
        Ok(result.value)
    }

    fn get_slot1(&mut self) -> Result<Slot> {
        self.get_slot()
            .map_err(|e| TransportError::Custom(e.to_string()))
    }

    fn get_account1(&mut self, pubkey: &Pubkey) -> Result<Option<Account>> {
        self.get_account(pubkey)
            .map(Some)
            .map_err(|e| TransportError::Custom(e.to_string()))
    }
}

impl Client for BankClient {
    fn send_transaction1(&mut self, transaction: Transaction) -> Result<Signature> {
        self.async_send_transaction(transaction)
    }

    fn get_signature_statuses1(
        &mut self,
        signatures: &[Signature],
    ) -> Result<Vec<Option<TransactionStatus>>> {
        signatures
            .iter()
            .map(|signature| {
                self.get_signature_status(signature).map(|opt| {
                    opt.map(|status| TransactionStatus {
                        slot: 0,
                        confirmations: None,
                        status,
                        err: None,
                    })
                })
            })
            .collect()
    }

    fn get_balance1(&mut self, pubkey: &Pubkey) -> Result<u64> {
        self.get_balance(pubkey)
    }

    fn get_fees1(&mut self) -> Result<(Hash, FeeCalculator, Slot)> {
        self.get_recent_blockhash_with_commitment(CommitmentConfig::default())
    }

    fn get_slot1(&mut self) -> Result<Slot> {
        self.get_slot()
    }

    fn get_account1(&mut self, pubkey: &Pubkey) -> Result<Option<Account>> {
        self.get_account(pubkey)
    }
}

impl Client for (Runtime, BanksClient) {
    fn send_transaction1(&mut self, transaction: Transaction) -> Result<Signature> {
        let signature = transaction.signatures[0];
        let banks_client = &mut self.1;
        self.0.block_on(async move {
            banks_client
                .send_transaction(context::current(), transaction)
                .await
        })?;
        Ok(signature)
    }

    fn get_signature_statuses1(
        &mut self,
        signatures: &[Signature],
    ) -> Result<Vec<Option<TransactionStatus>>> {
        let banks_client = &mut self.1;
        let statuses = self.0.block_on(async move {
            banks_client
                .get_signature_statuses(context::current(), signatures.to_vec())
                .await
        })?;
        let transaction_statuses = statuses
            .into_iter()
            .map(|opt| {
                opt.map(|status| TransactionStatus {
                    slot: 0,
                    confirmations: None,
                    status,
                    err: None,
                })
            })
            .collect();
        Ok(transaction_statuses)
    }

    fn get_balance1(&mut self, pubkey: &Pubkey) -> Result<u64> {
        Ok(self.0.block_on(get_balance(&mut self.1, *pubkey))?)
    }

    fn get_fees1(&mut self) -> Result<(Hash, FeeCalculator, Slot)> {
        let banks_client = &mut self.1;
        let (fee_calculator, recent_blockhash, last_valid_slot) = self
            .0
            .block_on(async move { banks_client.get_fees(context::current()).await })?;
        Ok((recent_blockhash, fee_calculator, last_valid_slot))
    }

    fn get_slot1(&mut self) -> Result<Slot> {
        let banks_client = &mut self.1;
        let root_slot = self
            .0
            .block_on(async move { banks_client.get_root_slot(context::current()).await })?;
        Ok(root_slot)
    }

    fn get_account1(&mut self, pubkey: &Pubkey) -> Result<Option<Account>> {
        let banks_client = &mut self.1;
        let account = self
            .0
            .block_on(async move { banks_client.get_account(context::current(), *pubkey).await })?;
        Ok(account)
    }
}

pub struct ThinClient<'a> {
    client: Box<dyn Client + 'a>,
    dry_run: bool,
}

impl<'a> ThinClient<'a> {
    pub fn new<C: Client + 'a>(client: C, dry_run: bool) -> Self {
        Self {
            client: Box::new(client),
            dry_run,
        }
    }

    pub fn send_transaction(&mut self, transaction: Transaction) -> Result<Signature> {
        if self.dry_run {
            return Ok(Signature::default());
        }
        self.client.send_transaction1(transaction)
    }

    pub fn poll_for_confirmation(&mut self, signature: &Signature) -> Result<()> {
        while self.get_signature_statuses(&[*signature])?[0].is_none() {
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        Ok(())
    }

    pub fn get_signature_statuses(
        &mut self,
        signatures: &[Signature],
    ) -> Result<Vec<Option<TransactionStatus>>> {
        self.client.get_signature_statuses1(signatures)
    }

    pub fn send_and_confirm_message<S: Signers>(
        &mut self,
        message: Message,
        signers: &S,
    ) -> Result<(Transaction, Slot)> {
        if self.dry_run {
            return Ok((Transaction::new_unsigned(message), std::u64::MAX));
        }
        let (blockhash, _fee_caluclator, last_valid_slot) = self.get_fees()?;

        let transaction = Transaction::new(signers, message, blockhash);
        self.send_transaction(transaction.clone())?;
        Ok((transaction, last_valid_slot))
    }

    pub fn transfer<S: Signer>(
        &mut self,
        lamports: u64,
        sender_keypair: &S,
        to_pubkey: &Pubkey,
    ) -> Result<(Transaction, u64)> {
        let create_instruction =
            system_instruction::transfer(&sender_keypair.pubkey(), &to_pubkey, lamports);
        let message = Message::new(&[create_instruction], Some(&sender_keypair.pubkey()));
        self.send_and_confirm_message(message, &[sender_keypair])
    }

    pub fn get_fees(&mut self) -> Result<(Hash, FeeCalculator, Slot)> {
        self.client.get_fees1()
    }

    pub fn get_slot(&mut self) -> Result<Slot> {
        self.client.get_slot1()
    }

    pub fn get_balance(&mut self, pubkey: &Pubkey) -> Result<u64> {
        self.client.get_balance1(pubkey)
    }

    pub fn get_account(&mut self, pubkey: &Pubkey) -> Result<Option<Account>> {
        self.client.get_account1(pubkey)
    }
}
