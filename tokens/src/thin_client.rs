use solana_sdk::{
    account::Account,
    banks_client::{get_balance, BanksClient},
    clock::Slot,
    fee_calculator::FeeCalculator,
    hash::Hash,
    message::Message,
    pubkey::Pubkey,
    signature::{Signature, Signer},
    signers::Signers,
    system_instruction,
    transaction::Transaction,
    transport::Result,
};
use solana_transaction_status::TransactionStatus;
use tarpc::context;

pub struct ThinClient {
    client: BanksClient,
    dry_run: bool,
}

impl ThinClient {
    pub fn new(client: BanksClient, dry_run: bool) -> Self {
        Self { client, dry_run }
    }

    pub async fn send_transaction(&mut self, transaction: Transaction) -> Result<Signature> {
        if self.dry_run {
            return Ok(Signature::default());
        }

        let signature = transaction.signatures[0];
        self.client
            .send_transaction(context::current(), transaction)
            .await?;
        Ok(signature)
    }

    pub async fn poll_for_confirmation(&mut self, signature: &Signature) -> Result<()> {
        while self.get_signature_statuses(&[*signature]).await?[0].is_none() {
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        Ok(())
    }

    pub async fn get_signature_statuses(
        &mut self,
        signatures: &[Signature],
    ) -> Result<Vec<Option<TransactionStatus>>> {
        let statuses = self
            .client
            .get_signature_statuses(context::current(), signatures.to_vec())
            .await?;
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

    pub async fn send_and_confirm_message<S: Signers>(
        &mut self,
        message: Message,
        signers: &S,
    ) -> Result<(Transaction, Slot)> {
        if self.dry_run {
            return Ok((Transaction::new_unsigned(message), std::u64::MAX));
        }
        let (blockhash, _fee_caluclator, last_valid_slot) = self.get_fees().await?;

        let transaction = Transaction::new(signers, message, blockhash);
        self.send_transaction(transaction.clone()).await?;
        Ok((transaction, last_valid_slot))
    }

    pub async fn transfer<S: Signer>(
        &mut self,
        lamports: u64,
        sender_keypair: &S,
        to_pubkey: &Pubkey,
    ) -> Result<(Transaction, u64)> {
        let create_instruction =
            system_instruction::transfer(&sender_keypair.pubkey(), &to_pubkey, lamports);
        let message = Message::new(&[create_instruction], Some(&sender_keypair.pubkey()));
        self.send_and_confirm_message(message, &[sender_keypair])
            .await
    }

    pub async fn get_fees(&mut self) -> Result<(Hash, FeeCalculator, Slot)> {
        let (fee_calculator, recent_blockhash, last_valid_slot) =
            self.client.get_fees(context::current()).await?;
        Ok((recent_blockhash, fee_calculator, last_valid_slot))
    }

    pub async fn get_slot(&mut self) -> Result<Slot> {
        Ok(self.client.get_root_slot(context::current()).await?)
    }

    pub async fn get_balance(&mut self, pubkey: &Pubkey) -> Result<u64> {
        Ok(get_balance(&mut self.client, *pubkey).await?)
    }

    pub async fn get_account(&mut self, pubkey: &Pubkey) -> Result<Option<Account>> {
        Ok(self.client.get_account(context::current(), *pubkey).await?)
    }
}
