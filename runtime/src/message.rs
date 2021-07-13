//! Verified and sanitized runtime message
use itertools::Itertools;
use solana_sdk::{
    bpf_loader, bpf_loader_deprecated, bpf_loader_upgradeable,
    hash::Hash,
    message::Message,
    pubkey::Pubkey,
    sanitize::Sanitize,
    signature::Signature,
    system_program, sysvar,
    transaction::{Result, Transaction, TransactionError},
};
use std::{borrow::Cow, ops::Deref, str::FromStr};

use crate::accounts::Accounts;

#[derive(Debug, PartialEq, Clone)]
pub struct RuntimeTransaction<'a> {
    pub tx: Cow<'a, Transaction>,
    pub hash: Hash,
    pub accounts: Vec<AccountMeta<'a>>,
    pub instructions: Vec<RuntimeInstruction<'a>>,
}

/// Account metadata used to define Instructions
#[derive(Debug, PartialEq, Clone)]
pub struct AccountMeta<'a> {
    /// Index in message.accounts
    pub index: usize,
    /// An account's public key
    pub pubkey: &'a Pubkey,
    /// True if an Instruction requires a Transaction signature matching `pubkey`.
    pub is_signer: bool,
    /// True if the `pubkey` can be loaded as a read-write account.
    pub is_writable: bool,
}

#[derive(Debug, PartialEq, Clone)]
pub struct RuntimeInstruction<'a> {
    /// Index in message.accounts
    pub program_id_index: usize,
    /// Pubkey of the instruction processor that executes this instruction
    pub program_id: &'a Pubkey,
    /// Metadata for what accounts should be passed to the instruction processor
    pub accounts: Vec<AccountMeta<'a>>,
    /// Unique account indices
    pub unique_account_indices: Vec<usize>,
    /// Instruction data
    pub data: &'a [u8],
}

pub struct AccountDetails<'a> {
    pub pubkey: &'a Pubkey,
    pub index: usize,

    pub program_signer: Option<bool>,
    pub is_executable: bool,

    pub caller_writable: bool,
    pub caller_signer: bool,

    pub callee_writable: bool,
    pub callee_signer: bool,
}

use lazy_static::lazy_static;
lazy_static! {
// Copied keys over since direct references create cyclical dependency.
static ref BUILTIN_PROGRAMS_KEYS: [Pubkey; 10] = {
    let parse = |s| Pubkey::from_str(s).unwrap();
    [
    parse("Config1111111111111111111111111111111111111"),
    parse("Feature111111111111111111111111111111111111"),
    parse("NativeLoader1111111111111111111111111111111"),
    parse("Stake11111111111111111111111111111111111111"),
    parse("StakeConfig11111111111111111111111111111111"),
    parse("Vote111111111111111111111111111111111111111"),
    system_program::id(),
    bpf_loader::id(),
    bpf_loader_deprecated::id(),
    bpf_loader_upgradeable::id(),
    ]
};
}

impl Deref for RuntimeTransaction<'_> {
    type Target = Transaction;
    fn deref(&self) -> &Self::Target {
        &self.tx
    }
}

impl <'a> RuntimeTransaction<'a> {
    pub fn try_build(transaction: Cow<'a, Transaction>, message_hash: Hash) -> Result<RuntimeTransaction<'a>> {
        transaction.verify_precompiles()?;
        transaction.sanitize()?;

        if Accounts::has_duplicates(&transaction.message.account_keys) {
            return Err(TransactionError::AccountLoadedTwice);
        }

        let accounts: Vec<_> = transaction.message
            .account_keys
            .iter()
            .enumerate()
            .map(|(i, key)| AccountMeta {
                index: i,
                pubkey: key,
                is_signer: transaction.message.is_signer(i),
                is_writable: transaction.message.is_writable(i),
            })
            .collect();

        let instructions = transaction.message
            .instructions
            .iter()
            .map(|compiled_ix| {
                let program_id_index = compiled_ix.program_id_index as usize;
                let unique_account_indices = compiled_ix
                    .accounts
                    .iter()
                    .unique()
                    .map(|index| *index as usize)
                    .collect();
                RuntimeInstruction {
                    data: &compiled_ix.data,
                    program_id_index,
                    program_id: &accounts[program_id_index].pubkey,
                    unique_account_indices,
                    accounts: compiled_ix
                        .accounts
                        .iter()
                        .map(|account_index| accounts[*account_index as usize].clone())
                        .collect(),
                }
            })
            .collect();

        Ok(Self {
            tx: transaction,
            hash: message_hash,
            accounts,
            instructions,
        })
    }

    pub fn is_writable(&self, i: usize) -> bool {
        let account = &self.accounts[i];
        if sysvar::is_sysvar_id(account.pubkey) || BUILTIN_PROGRAMS_KEYS.contains(&account.pubkey) {
            false
        } else {
            account.is_writable
        }
    }

    pub fn get_account_keys_by_lock_type(&self) -> (Vec<&Pubkey>, Vec<&Pubkey>) {
        let mut writable_keys = vec![];
        let mut readonly_keys = vec![];
        for account in self.accounts.iter() {
            if account.is_writable {
                writable_keys.push(account.pubkey);
            } else {
                readonly_keys.push(account.pubkey);
            }
        }
        (writable_keys, readonly_keys)
    }

    pub fn is_key_passed_to_program(&self, key_index: usize) -> bool {
        self.instructions
            .iter()
            .any(|ix| ix.accounts.iter().any(|account| account.index == key_index))
    }

    pub fn is_key_called_as_program(&self, key_index: usize) -> bool {
        self.instructions
            .iter()
            .any(|ix| ix.program_id_index == key_index)
    }

    pub fn is_non_loader_key(&self, key_index: usize) -> bool {
        !self.is_key_called_as_program(key_index) || self.is_key_passed_to_program(key_index)
    }
}
