use {
    crate::{
        transaction_processing_result::{ProcessedTransaction, TransactionProcessingOutcome},
        transaction_results::{TransactionExecutionDetails, TransactionLoadedAccountsStats},
    },
    solana_sdk::{
        fee::FeeDetails,
        rent_debits::RentDebits,
        transaction::{Result as TransactionResult, TransactionError},
    },
};

pub type TransactionCommitResult = TransactionResult<CommittedTransaction>;

#[derive(Clone, Debug)]
pub struct CommittedTransaction {
    pub loaded_account_stats: TransactionLoadedAccountsStats,
    pub execution_outcome: CommittedExecutionOutcome,
    pub fee_details: FeeDetails,
}

pub trait TransactionCommitResultExtensions {
    fn was_successful_execution(&self) -> bool;
    fn into_transaction_result(self) -> TransactionResult<()>;
}

impl TransactionCommitResultExtensions for TransactionCommitResult {
    fn was_successful_execution(&self) -> bool {
        match self {
            Ok(committed_tx) => committed_tx.execution_outcome.was_executed_successfully(),
            Err(_) => false,
        }
    }

    fn into_transaction_result(self) -> TransactionResult<()> {
        self.and_then(|committed_tx| committed_tx.execution_outcome.into_transaction_result())
    }
}

impl From<ProcessedTransaction> for CommittedTransaction {
    fn from(processed_tx: ProcessedTransaction) -> Self {
        Self {
            fee_details: processed_tx.fee_details,
            execution_outcome: processed_tx.outcome.into(),
            loaded_account_stats: processed_tx.loaded_account_stats,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommittedExecutionOutcome {
    Executed {
        details: Box<TransactionExecutionDetails>,
        rent_debits: RentDebits,
    },
    FeesOnly {
        err: TransactionError,
    },
}

impl From<TransactionProcessingOutcome> for CommittedExecutionOutcome {
    fn from(outcome: TransactionProcessingOutcome) -> Self {
        match outcome {
            TransactionProcessingOutcome::Executed(context) => Self::Executed {
                details: Box::new(context.execution_details),
                rent_debits: context.rent_debits,
            },
            TransactionProcessingOutcome::FeesOnly { err } => Self::FeesOnly { err },
        }
    }
}

impl CommittedExecutionOutcome {
    pub fn executed_units(&self) -> u64 {
        match self {
            Self::Executed { details, .. } => details.executed_units,
            Self::FeesOnly { .. } => 0,
        }
    }

    pub fn into_execution_details(self) -> Option<TransactionExecutionDetails> {
        if let Self::Executed { details, .. } = self {
            Some(*details)
        } else {
            None
        }
    }

    pub fn into_transaction_result(self) -> TransactionResult<()> {
        match self {
            Self::Executed { details, .. } => details.status,
            Self::FeesOnly { err } => Err(err),
        }
    }

    pub fn was_executed_successfully(&self) -> bool {
        match self {
            Self::Executed { details, .. } => details.status.is_ok(),
            Self::FeesOnly { .. } => false,
        }
    }
}
