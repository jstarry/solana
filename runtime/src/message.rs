//! Verified and sanitized runtime message
use itertools::Itertools;
use solana_sdk::{bpf_loader, bpf_loader_deprecated, bpf_loader_upgradeable, hash::Hash, instruction::{CompiledInstruction, Instruction, InstructionError}, message::Message, pubkey::Pubkey, sanitize::{Sanitize, SanitizeError}, signature::Signature, system_program, sysvar, transaction::Transaction};
use std::{collections::HashMap, convert::TryFrom, str::FromStr};
use thiserror::Error;

use crate::accounts::Accounts;

#[derive(Debug, PartialEq, Clone)]
pub struct RuntimeTransaction<'a> {
    pub hash: Hash,
    pub message: &'a Message,
    pub signatures: &'a [Signature],
    pub accounts: Vec<AccountMeta<'a>>,
    pub recent_blockhash: Hash,
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

impl RuntimeTransaction<'_> {
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

#[derive(Debug, Error)]
pub enum MessageError {
    #[error("Duplicate account key")]
    DuplicateAccountKey,
    #[error("Message failed sanitize check")]
    SanitizeFailure,
}

impl From<SanitizeError> for MessageError {
    fn from(_: SanitizeError) -> Self {
        Self::SanitizeFailure
    }
}

impl<'a> TryFrom<&'a Transaction> for RuntimeTransaction<'a> {
    type Error = MessageError;
    fn try_from(transaction: &'a Transaction) -> Result<Self, Self::Error> {
        transaction.sanitize()?;

        let message = &transaction.message;
        if Accounts::has_duplicates(&message.account_keys) {
            return Err(MessageError::DuplicateAccountKey);
        }

        let accounts: Vec<_> = message
            .account_keys
            .iter()
            .enumerate()
            .map(|(i, key)| AccountMeta {
                index: i,
                pubkey: key,
                is_signer: message.is_signer(i),
                is_writable: message.is_writable(i),
            })
            .collect();

        let instructions = message
            .instructions
            .iter()
            .map(|compiled_ix| {
                let program_id_index = compiled_ix.program_id_index as usize;
                let unique_account_indices = compiled_ix.accounts.iter().unique().map(|index| *index as usize).collect();
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
            hash: message.hash(),
            message,
            signatures: &transaction.signatures,
            accounts,
            recent_blockhash: message.recent_blockhash,
            instructions,
        })
    }
}
