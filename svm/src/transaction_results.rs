// Re-exported since these have moved to `solana_sdk`.
#[deprecated(
    since = "1.18.0",
    note = "Please use `solana_sdk::inner_instruction` types instead"
)]
pub use solana_sdk::inner_instruction::{InnerInstruction, InnerInstructionsList};
use {
    crate::{
        account_loader::LoadedTransaction, nonce_info::NonceInfo,
        rollback_accounts::RollbackAccounts,
    },
    serde::{Deserialize, Serialize},
    solana_program_runtime::loaded_programs::ProgramCacheEntry,
    solana_sdk::{
        account::ReadableAccount,
        fee::FeeDetails,
        pubkey::Pubkey,
        transaction::{self, TransactionError},
        transaction_context::TransactionReturnData,
    },
    std::{collections::HashMap, sync::Arc},
};

#[derive(Debug, Default, Clone)]
pub struct TransactionLoadedAccountsStats {
    pub loaded_accounts_data_size: usize,
    pub loaded_accounts_count: usize,
}

/// Type safe representation of a transaction execution attempt which
/// differentiates between a transaction that was executed (will be
/// committed to the ledger) and a transaction which wasn't executed
/// and will be dropped.
///
/// Note: `Result<TransactionExecutionDetails, TransactionError>` is not
/// used because it's easy to forget that the inner `details.status` field
/// is what should be checked to detect a successful transaction. This
/// enum provides a convenience method `Self::was_executed_successfully` to
/// make such checks hard to do incorrectly.
#[derive(Debug, Clone)]
pub enum TransactionExecutionResult {
    Executed(Box<ExecutedTransaction>),
    NotExecuted(TransactionLoadFailure),
}

impl TransactionExecutionResult {
    pub fn executed_transaction(&self) -> Option<&ExecutedTransaction> {
        match self {
            Self::Executed(executed_tx) => Some(executed_tx.as_ref()),
            Self::NotExecuted { .. } => None,
        }
    }

    pub fn executed_transaction_mut(&mut self) -> Option<&mut ExecutedTransaction> {
        match self {
            Self::Executed(executed_tx) => Some(executed_tx.as_mut()),
            Self::NotExecuted { .. } => None,
        }
    }

    pub fn was_executed_successfully(&self) -> bool {
        self.executed_transaction()
            .map(|executed_tx| executed_tx.was_successful())
            .unwrap_or(false)
    }

    pub fn was_executed(&self) -> bool {
        self.executed_transaction().is_some()
    }

    pub fn execution_details(&self) -> Option<&TransactionExecutionDetails> {
        self.executed_transaction()
            .map(|executed_tx| &executed_tx.execution_details)
    }

    pub fn flattened_result(&self) -> transaction::Result<()> {
        match self {
            Self::Executed(executed_tx) => executed_tx.execution_details.status.clone(),
            Self::NotExecuted(failure) => Err(failure.clone().into_err()),
        }
    }
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum TransactionLoadFailure {
    Discard(TransactionError),
    CollectFees {
        err: TransactionError,
        details: Box<CollectFeesDetails>,
    },
}

impl TransactionLoadFailure {
    pub fn into_err(self) -> TransactionError {
        match self {
            Self::Discard(err) | Self::CollectFees { err, .. } => err,
        }
    }
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub struct CollectFeesDetails {
    pub rollback_accounts: RollbackAccounts,
    pub fee_details: FeeDetails,
}

impl CollectFeesDetails {
    pub fn loaded_account_stats(&self) -> TransactionLoadedAccountsStats {
        match &self.rollback_accounts {
            RollbackAccounts::FeePayerOnly { fee_payer_account } => {
                TransactionLoadedAccountsStats {
                    loaded_accounts_count: 1,
                    loaded_accounts_data_size: fee_payer_account.data().len(),
                }
            }
            RollbackAccounts::SameNonceAndFeePayer { nonce } => TransactionLoadedAccountsStats {
                loaded_accounts_count: 1,
                loaded_accounts_data_size: nonce.account().data().len(),
            },
            RollbackAccounts::SeparateNonceAndFeePayer {
                nonce,
                fee_payer_account,
            } => TransactionLoadedAccountsStats {
                loaded_accounts_count: 2,
                loaded_accounts_data_size: fee_payer_account.data().len()
                    + nonce.account().data().len(),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExecutedTransaction {
    pub loaded_transaction: LoadedTransaction,
    pub execution_details: TransactionExecutionDetails,
    pub programs_modified_by_tx: HashMap<Pubkey, Arc<ProgramCacheEntry>>,
}

impl ExecutedTransaction {
    pub fn was_successful(&self) -> bool {
        self.execution_details.status.is_ok()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TransactionExecutionDetails {
    pub status: transaction::Result<()>,
    pub log_messages: Option<Vec<String>>,
    pub inner_instructions: Option<InnerInstructionsList>,
    pub return_data: Option<TransactionReturnData>,
    pub executed_units: u64,
    /// The change in accounts data len for this transaction.
    /// NOTE: This value is valid IFF `status` is `Ok`.
    pub accounts_data_len_delta: i64,
}
