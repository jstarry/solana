use {
    crate::{
        rollback_accounts::RollbackAccounts,
        transaction_results::{TransactionExecutionDetails, TransactionLoadedAccountsStats},
    },
    solana_program_runtime::loaded_programs::ProgramCacheEntry,
    solana_sdk::{
        fee::FeeDetails,
        pubkey::Pubkey,
        rent_debits::RentDebits,
        transaction::{Result as TransactionResult, TransactionError},
        transaction_context::TransactionAccount,
    },
    std::{collections::HashMap, sync::Arc},
};

pub type TransactionProcessingResult = TransactionResult<ProcessedTransaction>;

pub struct ProcessedTransaction {
    pub fee_details: FeeDetails,
    pub rollback_accounts: RollbackAccounts,
    pub outcome: TransactionProcessingOutcome,
    pub loaded_account_stats: TransactionLoadedAccountsStats,
}

impl ProcessedTransaction {
    pub fn is_failure(&self) -> bool {
        !self.outcome.was_executed_successfully()
    }

    pub fn program_modifications(&self) -> Option<&HashMap<Pubkey, Arc<ProgramCacheEntry>>> {
        if let TransactionProcessingOutcome::Executed(executed_tx) = &self.outcome {
            let details = &executed_tx.execution_details;
            let programs_modified_by_tx = &executed_tx.programs_modified_by_tx;
            if details.status.is_ok() && !programs_modified_by_tx.is_empty() {
                return Some(programs_modified_by_tx);
            }
        }
        None
    }

    pub fn accounts_data_len_delta(&self) -> i64 {
        if let Some(details) = self.outcome.execution_details() {
            if details.status.is_ok() {
                return details.accounts_data_len_delta;
            }
        }
        0
    }

    pub fn status(&self) -> TransactionResult<()> {
        match &self.outcome {
            TransactionProcessingOutcome::Executed(executed_tx) => {
                executed_tx.execution_details.status.clone()
            }
            TransactionProcessingOutcome::FeesOnly { err, .. } => Err(TransactionError::clone(err)),
        }
    }
}

pub struct TransactionPostExecutionContext {
    pub execution_details: TransactionExecutionDetails,
    pub collected_rent: u64,
    pub rent_debits: RentDebits,
    pub accounts: Vec<TransactionAccount>,
    pub programs_modified_by_tx: HashMap<Pubkey, Arc<ProgramCacheEntry>>,
}

pub enum TransactionProcessingOutcome {
    Executed(Box<TransactionPostExecutionContext>),
    FeesOnly { err: TransactionError },
}

impl TransactionProcessingOutcome {
    pub fn execution_context(&self) -> Option<&TransactionPostExecutionContext> {
        match self {
            Self::Executed(context) => Some(context),
            Self::FeesOnly { .. } => None,
        }
    }

    pub fn execution_details(&self) -> Option<&TransactionExecutionDetails> {
        match self {
            Self::Executed(context) => Some(&context.execution_details),
            Self::FeesOnly { .. } => None,
        }
    }

    pub fn flattened_result(&self) -> TransactionResult<()> {
        match self {
            Self::Executed(context) => context.execution_details.status.clone(),
            Self::FeesOnly { err, .. } => Err(err.clone()),
        }
    }

    pub fn was_executed(&self) -> bool {
        match self {
            Self::Executed { .. } => true,
            Self::FeesOnly { .. } => false,
        }
    }

    pub fn was_executed_successfully(&self) -> bool {
        match self {
            Self::Executed(context) => context.execution_details.status.is_ok(),
            Self::FeesOnly { .. } => false,
        }
    }
}
