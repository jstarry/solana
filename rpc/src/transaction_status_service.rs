use {
    crossbeam_channel::{Receiver, RecvTimeoutError},
    itertools::izip,
    solana_ledger::{
        blockstore::Blockstore,
        blockstore_processor::{TransactionStatusBatch, TransactionStatusMessage},
    },
    solana_runtime::bank::{InnerInstructionsList, TransactionLogMessages},
    solana_transaction_status::{
        extract_and_fmt_memos, InnerInstructions, Reward, TransactionStatusMeta,
    },
    std::{
        sync::{
            atomic::{AtomicBool, AtomicU64, Ordering},
            Arc,
        },
        thread::{self, Builder, JoinHandle},
        time::Duration,
    },
};

pub struct TransactionStatusService {
    thread_hdl: JoinHandle<()>,
}

impl TransactionStatusService {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(
        write_transaction_status_receiver: Receiver<TransactionStatusMessage>,
        max_complete_transaction_status_slot: Arc<AtomicU64>,
        blockstore: Arc<Blockstore>,
        exit: &Arc<AtomicBool>,
    ) -> Self {
        let exit = exit.clone();
        let thread_hdl = Builder::new()
            .name("solana-transaction-status-writer".to_string())
            .spawn(move || loop {
                if exit.load(Ordering::Relaxed) {
                    break;
                }
                if let Err(RecvTimeoutError::Disconnected) = Self::write_transaction_status_batch(
                    &write_transaction_status_receiver,
                    &max_complete_transaction_status_slot,
                    &blockstore,
                ) {
                    break;
                }
            })
            .unwrap();
        Self { thread_hdl }
    }

    fn write_transaction_status_batch(
        write_transaction_status_receiver: &Receiver<TransactionStatusMessage>,
        max_complete_transaction_status_slot: &Arc<AtomicU64>,
        blockstore: &Arc<Blockstore>,
    ) -> Result<(), RecvTimeoutError> {
        match write_transaction_status_receiver.recv_timeout(Duration::from_secs(1))? {
            TransactionStatusMessage::Batch(TransactionStatusBatch { bank, items }) => {
                let slot = bank.slot();
                for item in items {
                    let TransactionStatusBatchItem { transaction, meta } = item;
                    let tx_account_locks =
                        transaction.get_account_locks(bank.demote_program_write_locks());

                    if let Some(memos) = extract_and_fmt_memos(transaction.message()) {
                        blockstore
                            .write_transaction_memos(transaction.signature(), memos)
                            .expect("Expect database write to succeed: TransactionMemos");
                    }

                    blockstore
                        .write_transaction_status(
                            slot,
                            *transaction.signature(),
                            tx_account_locks.writable,
                            tx_account_locks.readonly,
                            meta,
                        )
                        .expect("Expect database write to succeed: TransactionStatus");
                }
            }
            TransactionStatusMessage::Freeze(slot) => {
                max_complete_transaction_status_slot.fetch_max(slot, Ordering::SeqCst);
            }
        }
        Ok(())
    }

    pub fn join(self) -> thread::Result<()> {
        self.thread_hdl.join()
    }
}
