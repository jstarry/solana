#![allow(clippy::integer_arithmetic)]
//! A library for generating a message from a sequence of instructions

use crate::sanitize::{Sanitize, SanitizeError};
use crate::serialize_utils::{
    append_slice, append_u16, append_u8, read_pubkey, read_slice, read_u16, read_u8,
};
use crate::{
    bpf_loader, bpf_loader_deprecated, bpf_loader_upgradeable,
    hash::Hash,
    instruction::{AccountMeta, CompiledInstruction, Instruction},
    message::MessageHeader,
    pubkey::Pubkey,
    short_vec, system_instruction, system_program, sysvar,
};
use itertools::Itertools;
use lazy_static::lazy_static;
use std::{convert::TryFrom, str::FromStr};

/// An instruction to execute a program
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CompiledInstruction<'a> {
    /// Index into the transaction keys array indicating the program account that executes this instruction
    pub program_id_index: u8,
    /// Ordered indices into the transaction keys array indicating which accounts to pass to the program
    #[serde(with = "short_bytes")]
    pub accounts: &'a [u8],
    /// The program input data
    #[serde(with = "short_bytes")]
    pub data: &'a [u8],
}

// NOTE: Serialization-related changes must be paired with the custom serialization
// for versioned messages in the `RemainingLegacyMessage` struct.
#[derive(Serialize, Deserialize, Default, Debug, PartialEq, Eq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Message<'a> {
    /// The message header, identifying signed and read-only `account_keys`
    /// NOTE: Serialization-related changes must be paired with the direct read at sigverify.
    pub header: MessageHeader,

    /// All the account keys used by this transaction
    #[serde(with = "short_bytes")]
    pub account_keys: &'a [Pubkey],

    /// The id of a recent ledger entry.
    pub recent_blockhash: &'a Hash,

    /// Programs that will be executed in sequence and committed in one atomic transaction if all
    /// succeed.
    #[serde(with = "short_vec")]
    pub instructions: Vec<CompiledInstruction<'a>>,
}
