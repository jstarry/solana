#![allow(clippy::integer_arithmetic)]
//! This account contains the serialized transaction instructions

use crate::serialize_utils::{read_pubkey, read_slice, read_u16, read_u8};
use crate::{
    instruction::{AccountMeta, Instruction},
    sanitize::SanitizeError,
};

// Instructions Sysvar, dummy type, use the associated helpers instead of the Sysvar trait
pub struct Instructions();

crate::declare_sysvar_id!("Sysvar1nstructions1111111111111111111111111", Instructions);

/// Load the current instruction's index from the Instructions Sysvar data
pub fn load_current_index(data: &[u8]) -> u16 {
    let mut instr_fixed_data = [0u8; 2];
    let len = data.len();
    instr_fixed_data.copy_from_slice(&data[len - 2..len]);
    u16::from_le_bytes(instr_fixed_data)
}

/// Store the current instruction's index in the Instructions Sysvar data
pub fn store_current_index(data: &mut [u8], instruction_index: u16) {
    let last_index = data.len() - 2;
    data[last_index..last_index + 2].copy_from_slice(&instruction_index.to_le_bytes());
}

/// The bit encoding whether an account is a signer
pub const IS_SIGNER_BIT: usize = 0;

/// The bit encoding whether an account is a writable
pub const IS_WRITABLE_BIT: usize = 1;

/// Load an instruction at the specified index
pub fn load_instruction_at(index: usize, data: &[u8]) -> Result<Instruction, SanitizeError> {
    let mut current = 0;
    let num_instructions = read_u16(&mut current, data)?;
    if index >= num_instructions as usize {
        return Err(SanitizeError::IndexOutOfBounds);
    }

    // index into the instruction byte-offset table.
    current += index * 2;
    let start = read_u16(&mut current, data)?;

    current = start as usize;
    let num_accounts = read_u16(&mut current, data)?;
    let mut accounts = Vec::with_capacity(num_accounts as usize);
    for _ in 0..num_accounts {
        let meta_byte = read_u8(&mut current, data)?;
        let mut is_signer = false;
        let mut is_writable = false;
        if meta_byte & (1 << IS_SIGNER_BIT) != 0 {
            is_signer = true;
        }
        if meta_byte & (1 << IS_WRITABLE_BIT) != 0 {
            is_writable = true;
        }
        let pubkey = read_pubkey(&mut current, data)?;
        accounts.push(AccountMeta {
            pubkey,
            is_signer,
            is_writable,
        });
    }
    let program_id = read_pubkey(&mut current, data)?;
    let data_len = read_u16(&mut current, data)?;
    let data = read_slice(&mut current, data, data_len as usize)?;
    Ok(Instruction {
        program_id,
        accounts,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_store_instruction() {
        let mut data = [4u8; 10];
        store_current_index(&mut data, 3);
        assert_eq!(load_current_index(&data), 3);
        assert_eq!([4u8; 8], data[0..8]);
    }
}
