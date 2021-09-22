use {
    crate::{
        accounts::LoadedTransaction,
        bank::{Bank, ExecutedTransaction, NonceRollbackPartial},
    },
    solana_sdk::transaction::{
        Result, SanitizedTransaction, TransactionAccountLocks, TransactionError,
    },
    std::{borrow::Cow, ops::Index},
};

// Represents the results of trying to lock a set of accounts
pub struct TransactionBatch<'a, 'b> {
    bank: &'a Bank,
    items: Vec<TransactionBatchItem<'b>>,
}

pub enum TransactionExecutionStatus {
    Ready,
    Loaded(Box<LoadedTransaction>),
    Executed(Box<ExecutedTransaction>),
    Retryable,
    Discarded(TransactionError),
}

pub struct TransactionBatchItem<'a> {
    pub(crate) tx: Cow<'a, SanitizedTransaction>,
    pub(crate) status: TransactionExecutionStatus,
    locked: bool,
    pub(crate) nonce_rollback: Option<NonceRollbackPartial>,
}

impl<'a> TransactionBatchItem<'a> {
    pub fn is_ready(&self) -> bool {
        matches!(self.status, TransactionExecutionStatus::Ready)
    }

    pub fn mark_loaded(&mut self, loaded_tx: Box<LoadedTransaction>) {
        self.status = TransactionExecutionStatus::Loaded(loaded_tx);
    }

    pub fn mark_discarded(&mut self, err: TransactionError) {
        self.status = TransactionExecutionStatus::Discarded(err);
        self.nonce_rollback = None;
    }

    pub fn mark_executed(&mut self, executed_transaction: Box<ExecutedTransaction>) {
        self.status = TransactionExecutionStatus::Executed(executed_transaction);
    }

    pub fn tx(&self) -> &SanitizedTransaction {
        self.tx.as_ref()
    }

    pub fn new(tx: Cow<'a, SanitizedTransaction>, locked: bool) -> Self {
        Self {
            tx,
            status: if locked {
                TransactionExecutionStatus::Ready
            } else {
                TransactionExecutionStatus::Retryable
            },
            locked,
            nonce_rollback: None,
        }
    }

    pub fn executed(&self) -> Option<(&SanitizedTransaction, &ExecutedTransaction)> {
        if let TransactionExecutionStatus::Executed(executed) = &self.status {
            Some((self.tx.as_ref(), executed))
        } else {
            None
        }
    }

    pub fn processed_tx(&self) -> Option<&SanitizedTransaction> {
        if self.can_commit() {
            Some(&self.tx)
        } else {
            None
        }
    }

    pub fn can_commit(&self) -> bool {
        if let TransactionExecutionStatus::Executed(executed) = &self.status {
            true
        } else {
            false
        }
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }
}

impl<'a, 'b> Index<usize> for TransactionBatch<'a, 'b> {
    type Output = SanitizedTransaction;
    fn index(&self, index: usize) -> &Self::Output {
        &self.items[index].tx
    }
}

impl<'a, 'b> TransactionBatch<'a, 'b> {
    pub fn new(bank: &'a Bank, items: Vec<TransactionBatchItem<'b>>) -> Self {
        Self { bank, items }
    }

    pub fn take_transaction_account_locks(
        &mut self,
        demote_program_write_locks: bool,
    ) -> Vec<TransactionAccountLocks> {
        self.items
            .iter_mut()
            .filter_map(move |item| {
                if item.locked {
                    item.locked = false;
                    Some(item.tx.get_account_locks(demote_program_write_locks))
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn get_executed_tx(
        &self,
        index: usize,
    ) -> Option<(&SanitizedTransaction, &ExecutedTransaction)> {
        self.items.get(index).and_then(|item| {
            if let TransactionExecutionStatus::Executed(executed) = &item.status {
                Some((item.tx(), executed.as_ref()))
            } else {
                None
            }
        })
    }

    pub fn loaded_txs_iter(
        &'b mut self,
    ) -> impl Iterator<Item = (&'b mut TransactionBatchItem, &'b LoadedTransaction)> {
        self.items.iter_mut().filter_map(|item| {
            if let TransactionExecutionStatus::Loaded(loaded) = &item.status {
                Some((item, loaded.as_ref()))
            } else {
                None
            }
        })
    }

    pub fn executed_txs_iter(
        &self,
    ) -> impl Iterator<Item = (&SanitizedTransaction, &ExecutedTransaction)> {
        self.items.iter().filter_map(|item| {
            if let TransactionExecutionStatus::Executed(executed) = &item.status {
                Some((item.tx.as_ref(), executed.as_ref()))
            } else {
                None
            }
        })
    }

    // pub fn sanitized_transactions(&self) -> Vec<&SanitizedTransaction> {
    //     self.sanitized_txs_iter().collect()
    // }

    // pub fn sanitized_txs_iter(&self) -> impl Iterator<Item = &SanitizedTransaction> {
    //     self.items().iter().map(|item| item.tx.as_ref())
    // }

    pub fn items_mut(&mut self) -> &mut [TransactionBatchItem<'b>] {
        &mut self.items
    }

    // pub fn items(&self) -> &[TransactionBatchItem] {
    //     &self.items
    // }

    pub fn bank(&self) -> &Bank {
        self.bank
    }
}

// Unlock all locked accounts in destructor.
impl<'a, 'b> Drop for TransactionBatch<'a, 'b> {
    fn drop(&mut self) {
        self.bank.unlock_accounts(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genesis_utils::{create_genesis_config_with_leader, GenesisConfigInfo};
    use solana_sdk::{signature::Keypair, system_transaction};
    use std::convert::TryInto;

    #[test]
    fn test_transaction_batch() {
        let (bank, txs) = setup();

        // Test getting locked accounts
        let batch = bank.prepare_sanitized_batch(&txs);

        // Grab locks
        assert!(batch.processing_results().iter().all(|x| x.is_ok()));

        // Trying to grab locks again should fail
        let batch2 = bank.prepare_sanitized_batch(&txs);
        assert!(batch2.processing_results().iter().all(|x| x.is_err()));

        // Drop the first set of locks
        drop(batch);

        // Now grabbing locks should work again
        let batch2 = bank.prepare_sanitized_batch(&txs);
        assert!(batch2.processing_results().iter().all(|x| x.is_ok()));
    }

    #[test]
    fn test_simulation_batch() {
        let (bank, txs) = setup();

        // Prepare batch without locks
        let batch = bank.prepare_simulation_batch(txs[0].clone());
        assert!(batch.processing_results().iter().all(|x| x.is_ok()));

        // Grab locks
        let batch2 = bank.prepare_sanitized_batch(&txs);
        assert!(batch2.processing_results().iter().all(|x| x.is_ok()));

        // Prepare another batch without locks
        let batch3 = bank.prepare_simulation_batch(txs[0].clone());
        assert!(batch3.processing_results().iter().all(|x| x.is_ok()));
    }

    fn setup() -> (Bank, Vec<SanitizedTransaction>) {
        let dummy_leader_pubkey = solana_sdk::pubkey::new_rand();
        let GenesisConfigInfo {
            genesis_config,
            mint_keypair,
            ..
        } = create_genesis_config_with_leader(500, &dummy_leader_pubkey, 100);
        let bank = Bank::new_for_tests(&genesis_config);

        let pubkey = solana_sdk::pubkey::new_rand();
        let keypair2 = Keypair::new();
        let pubkey2 = solana_sdk::pubkey::new_rand();

        let txs = vec![
            system_transaction::transfer(&mint_keypair, &pubkey, 1, genesis_config.hash())
                .try_into()
                .unwrap(),
            system_transaction::transfer(&keypair2, &pubkey2, 1, genesis_config.hash())
                .try_into()
                .unwrap(),
        ];

        (bank, txs)
    }
}
