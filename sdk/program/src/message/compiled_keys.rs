use {
    super::v0::{LoadedAddresses, MessageAddressTableLookup},
    crate::{
        instruction::{CompiledInstruction, Instruction},
        message::MessageHeader,
        pubkey::Pubkey,
    },
    std::{borrow::Cow, collections::BTreeMap},
};

/// A helper struct to collect pubkeys referenced by a set of instructions
#[derive(Debug, PartialEq, Eq)]
pub struct CompiledKeys {
    pub writable_signer_keys: Vec<Pubkey>,
    pub readonly_signer_keys: Vec<Pubkey>,
    pub writable_non_signer_keys: Vec<Pubkey>,
    pub readonly_non_signer_keys: Vec<Pubkey>,
}

impl CompiledKeys {
    pub fn into_header_and_account_keys(self) -> (MessageHeader, Vec<Pubkey>) {
        let header = MessageHeader {
            num_required_signatures: self.writable_signer_keys.len() as u8
                + self.readonly_signer_keys.len() as u8,
            num_readonly_signed_accounts: self.readonly_signer_keys.len() as u8,
            num_readonly_unsigned_accounts: self.readonly_non_signer_keys.len() as u8,
        };

        let account_keys: Vec<Pubkey> = self
            .writable_signer_keys
            .into_iter()
            .chain(self.readonly_signer_keys)
            .chain(self.writable_non_signer_keys)
            .chain(self.readonly_non_signer_keys)
            .collect();

        (header, account_keys)
    }
}

#[derive(Default, Debug)]
struct InstructionAccountMeta {
    is_signer: bool,
    is_writable: bool,
}

fn drain_filter(
    keys: &mut Vec<Pubkey>,
    lookup_table: &AddressLookupTable,
) -> (Vec<u8>, Vec<Pubkey>) {
    let mut indexes = Vec::new();
    let mut drained_addresses = Vec::new();
    let mut i = 0;
    while i < keys.len() {
        let search_key = &keys[i];
        let found_key = lookup_table
            .addresses
            .iter()
            .enumerate()
            .find_map(|(key_index, key)| {
                if *key == search_key {
                    Some(key_index as u8)
                } else {
                    None
                }
            });

        if let Some(index) = found_key {
            indexes.push(index);
            drained_addresses.push(keys.remove(i));
        } else {
            i += 1;
        }
    }
    (indexes, drained_addresses)
}

pub struct AddressLookupTable<'a> {
    pub key: &'a Pubkey,
    pub addresses: Cow<'a, [&'a Pubkey]>,
}

pub(crate) fn compile_instructions<'a>(
    instructions: &[Instruction],
    ordered_keys: impl Iterator<Item = &'a Pubkey>,
) -> Vec<CompiledInstruction> {
    let account_index_map: BTreeMap<&Pubkey, u8> = BTreeMap::from_iter(
        ordered_keys
            .enumerate()
            .map(|(index, key)| (key, index as u8)),
    );

    instructions
        .iter()
        .map(|ix| {
            let accounts: Vec<u8> = ix
                .accounts
                .iter()
                .map(|account_meta| *account_index_map.get(&account_meta.pubkey).unwrap())
                .collect();

            CompiledInstruction {
                program_id_index: *account_index_map.get(&ix.program_id).unwrap(),
                data: ix.data.clone(),
                accounts,
            }
        })
        .collect()
}

impl CompiledKeys {
    /// Compiles the pubkeys referenced by a list of instructions and organizes by
    /// signer/non-signer and writable/readonly.
    pub fn compile(payer: Option<&Pubkey>, instructions: &[Instruction]) -> Self {
        let mut account_meta_map = BTreeMap::<&Pubkey, InstructionAccountMeta>::new();
        for ix in instructions {
            account_meta_map.entry(&ix.program_id).or_default();
            for account_meta in &ix.accounts {
                let meta = account_meta_map.entry(&account_meta.pubkey).or_default();
                meta.is_signer |= account_meta.is_signer;
                meta.is_writable |= account_meta.is_writable;
            }
        }

        let mut writable_signer_keys = vec![];
        if let Some(payer) = payer {
            account_meta_map.remove(payer);
            writable_signer_keys.push(*payer);
        }

        writable_signer_keys.extend(
            account_meta_map
                .iter()
                .filter_map(|(key, meta)| (meta.is_signer && meta.is_writable).then(|| **key)),
        );

        let readonly_signer_keys = account_meta_map
            .iter()
            .filter_map(|(key, meta)| (meta.is_signer && !meta.is_writable).then(|| **key))
            .collect();

        let writable_non_signer_keys = account_meta_map
            .iter()
            .filter_map(|(key, meta)| (!meta.is_signer && meta.is_writable).then(|| **key))
            .collect();
        let readonly_non_signer_keys = account_meta_map
            .iter()
            .filter_map(|(key, meta)| (!meta.is_signer && !meta.is_writable).then(|| **key))
            .collect();

        CompiledKeys {
            writable_signer_keys,
            readonly_signer_keys,
            writable_non_signer_keys,
            readonly_non_signer_keys,
        }
    }

    pub fn drain_lookups(
        &mut self,
        lookup_table: &AddressLookupTable,
    ) -> Option<(MessageAddressTableLookup, LoadedAddresses)> {
        let (writable_indexes, writable) =
            drain_filter(&mut self.writable_non_signer_keys, lookup_table);
        let (readonly_indexes, readonly) =
            drain_filter(&mut self.readonly_non_signer_keys, lookup_table);
        if !writable_indexes.is_empty() || !readonly_indexes.is_empty() {
            Some((
                MessageAddressTableLookup {
                    account_key: *lookup_table.key,
                    writable_indexes,
                    readonly_indexes,
                },
                LoadedAddresses { writable, readonly },
            ))
        } else {
            None
        }
    }
}

// #[cfg(test)]
// mod tests {
//     #![allow(deprecated)]
//     use {
//         super::*,
//         crate::{hash, instruction::AccountMeta, message::MESSAGE_HEADER_LENGTH},
//         std::collections::HashSet,
//     };

//     #[test]
//     fn test_message_unique_program_ids() {
//         let program_id0 = Pubkey::default();
//         let program_ids = get_program_ids(&[
//             Instruction::new_with_bincode(program_id0, &0, vec![]),
//             Instruction::new_with_bincode(program_id0, &0, vec![]),
//         ]);
//         assert_eq!(program_ids, vec![program_id0]);
//     }

//     #[test]
//     fn test_builtin_program_keys() {
//         let keys: HashSet<Pubkey> = BUILTIN_PROGRAMS_KEYS.iter().copied().collect();
//         assert_eq!(keys.len(), 10);
//         for k in keys {
//             let k = format!("{}", k);
//             assert!(k.ends_with("11111111111111111111111"));
//         }
//     }

//     #[test]
//     fn test_builtin_program_keys_abi_freeze() {
//         // Once the feature is flipped on, we can't further modify
//         // BUILTIN_PROGRAMS_KEYS without the risk of breaking consensus.
//         let builtins = format!("{:?}", *BUILTIN_PROGRAMS_KEYS);
//         assert_eq!(
//             format!("{}", hash::hash(builtins.as_bytes())),
//             "ACqmMkYbo9eqK6QrRSrB3HLyR6uHhLf31SCfGUAJjiWj"
//         );
//     }

//     #[test]
//     fn test_message_unique_program_ids_not_adjacent() {
//         let program_id0 = Pubkey::default();
//         let program_id1 = Pubkey::new_unique();
//         let program_ids = get_program_ids(&[
//             Instruction::new_with_bincode(program_id0, &0, vec![]),
//             Instruction::new_with_bincode(program_id1, &0, vec![]),
//             Instruction::new_with_bincode(program_id0, &0, vec![]),
//         ]);
//         assert_eq!(program_ids, vec![program_id0, program_id1]);
//     }

//     #[test]
//     fn test_message_unique_program_ids_order_preserved() {
//         let program_id0 = Pubkey::new_unique();
//         let program_id1 = Pubkey::default(); // Key less than program_id0
//         let program_ids = get_program_ids(&[
//             Instruction::new_with_bincode(program_id0, &0, vec![]),
//             Instruction::new_with_bincode(program_id1, &0, vec![]),
//             Instruction::new_with_bincode(program_id0, &0, vec![]),
//         ]);
//         assert_eq!(program_ids, vec![program_id0, program_id1]);
//     }

//     #[test]
//     fn test_message_unique_keys_both_signed() {
//         let program_id = Pubkey::default();
//         let id0 = Pubkey::default();
//         let keys = get_keys(
//             &[
//                 Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, true)]),
//                 Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, true)]),
//             ],
//             None,
//         );
//         assert_eq!(keys, InstructionKeys::new(vec![id0], vec![], 0, 0));
//     }

//     #[test]
//     fn test_message_unique_keys_signed_and_payer() {
//         let program_id = Pubkey::default();
//         let id0 = Pubkey::default();
//         let keys = get_keys(
//             &[Instruction::new_with_bincode(
//                 program_id,
//                 &0,
//                 vec![AccountMeta::new(id0, true)],
//             )],
//             Some(&id0),
//         );
//         assert_eq!(keys, InstructionKeys::new(vec![id0], vec![], 0, 0));
//     }

//     #[test]
//     fn test_message_unique_keys_unsigned_and_payer() {
//         let program_id = Pubkey::default();
//         let id0 = Pubkey::default();
//         let keys = get_keys(
//             &[Instruction::new_with_bincode(
//                 program_id,
//                 &0,
//                 vec![AccountMeta::new(id0, false)],
//             )],
//             Some(&id0),
//         );
//         assert_eq!(keys, InstructionKeys::new(vec![id0], vec![], 0, 0));
//     }

//     #[test]
//     fn test_message_unique_keys_one_signed() {
//         let program_id = Pubkey::default();
//         let id0 = Pubkey::default();
//         let keys = get_keys(
//             &[
//                 Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, false)]),
//                 Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, true)]),
//             ],
//             None,
//         );
//         assert_eq!(keys, InstructionKeys::new(vec![id0], vec![], 0, 0));
//     }

//     #[test]
//     fn test_message_unique_keys_one_readonly_signed() {
//         let program_id = Pubkey::default();
//         let id0 = Pubkey::default();
//         let keys = get_keys(
//             &[
//                 Instruction::new_with_bincode(
//                     program_id,
//                     &0,
//                     vec![AccountMeta::new_readonly(id0, true)],
//                 ),
//                 Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, true)]),
//             ],
//             None,
//         );

//         // Ensure the key is no longer readonly
//         assert_eq!(keys, InstructionKeys::new(vec![id0], vec![], 0, 0));
//     }

//     #[test]
//     fn test_message_unique_keys_one_readonly_unsigned() {
//         let program_id = Pubkey::default();
//         let id0 = Pubkey::default();
//         let keys = get_keys(
//             &[
//                 Instruction::new_with_bincode(
//                     program_id,
//                     &0,
//                     vec![AccountMeta::new_readonly(id0, false)],
//                 ),
//                 Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, false)]),
//             ],
//             None,
//         );

//         // Ensure the key is no longer readonly
//         assert_eq!(keys, InstructionKeys::new(vec![], vec![id0], 0, 0));
//     }

//     #[test]
//     fn test_message_unique_keys_order_preserved() {
//         let program_id = Pubkey::default();
//         let id0 = Pubkey::new_unique();
//         let id1 = Pubkey::default(); // Key less than id0
//         let keys = get_keys(
//             &[
//                 Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, false)]),
//                 Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id1, false)]),
//             ],
//             None,
//         );
//         assert_eq!(keys, InstructionKeys::new(vec![], vec![id0, id1], 0, 0));
//     }

//     #[test]
//     fn test_message_unique_keys_not_adjacent() {
//         let program_id = Pubkey::default();
//         let id0 = Pubkey::default();
//         let id1 = Pubkey::new_unique();
//         let keys = get_keys(
//             &[
//                 Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, false)]),
//                 Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id1, false)]),
//                 Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, true)]),
//             ],
//             None,
//         );
//         assert_eq!(keys, InstructionKeys::new(vec![id0], vec![id1], 0, 0));
//     }

//     #[test]
//     fn test_message_signed_keys_first() {
//         let program_id = Pubkey::default();
//         let id0 = Pubkey::default();
//         let id1 = Pubkey::new_unique();
//         let keys = get_keys(
//             &[
//                 Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, false)]),
//                 Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id1, true)]),
//             ],
//             None,
//         );
//         assert_eq!(keys, InstructionKeys::new(vec![id1], vec![id0], 0, 0));
//     }

//     #[test]
//     // Ensure there's a way to calculate the number of required signatures.
//     fn test_message_signed_keys_len() {
//         let program_id = Pubkey::default();
//         let id0 = Pubkey::default();
//         let ix = Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, false)]);
//         let message = Message::new(&[ix], None);
//         assert_eq!(message.header.num_required_signatures, 0);

//         let ix = Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, true)]);
//         let message = Message::new(&[ix], Some(&id0));
//         assert_eq!(message.header.num_required_signatures, 1);
//     }

//     #[test]
//     fn test_message_readonly_keys_last() {
//         let program_id = Pubkey::default();
//         let id0 = Pubkey::default(); // Identical key/program_id should be de-duped
//         let id1 = Pubkey::new_unique();
//         let id2 = Pubkey::new_unique();
//         let id3 = Pubkey::new_unique();
//         let keys = get_keys(
//             &[
//                 Instruction::new_with_bincode(
//                     program_id,
//                     &0,
//                     vec![AccountMeta::new_readonly(id0, false)],
//                 ),
//                 Instruction::new_with_bincode(
//                     program_id,
//                     &0,
//                     vec![AccountMeta::new_readonly(id1, true)],
//                 ),
//                 Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id2, false)]),
//                 Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id3, true)]),
//             ],
//             None,
//         );
//         assert_eq!(
//             keys,
//             InstructionKeys::new(vec![id3, id1], vec![id2, id0], 1, 1)
//         );
//     }

//     #[test]
//     fn test_message_kitchen_sink() {
//         let program_id0 = Pubkey::new_unique();
//         let program_id1 = Pubkey::new_unique();
//         let id0 = Pubkey::default();
//         let id1 = Pubkey::new_unique();
//         let message = Message::new(
//             &[
//                 Instruction::new_with_bincode(program_id0, &0, vec![AccountMeta::new(id0, false)]),
//                 Instruction::new_with_bincode(program_id1, &0, vec![AccountMeta::new(id1, true)]),
//                 Instruction::new_with_bincode(program_id0, &0, vec![AccountMeta::new(id1, false)]),
//             ],
//             Some(&id1),
//         );
//         assert_eq!(
//             message.instructions[0],
//             CompiledInstruction::new(2, &0, vec![1])
//         );
//         assert_eq!(
//             message.instructions[1],
//             CompiledInstruction::new(3, &0, vec![0])
//         );
//         assert_eq!(
//             message.instructions[2],
//             CompiledInstruction::new(2, &0, vec![0])
//         );
//     }

//     #[test]
//     fn test_message_payer_first() {
//         let program_id = Pubkey::default();
//         let payer = Pubkey::new_unique();
//         let id0 = Pubkey::default();

//         let ix = Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, false)]);
//         let message = Message::new(&[ix], Some(&payer));
//         assert_eq!(message.header.num_required_signatures, 1);

//         let ix = Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, true)]);
//         let message = Message::new(&[ix], Some(&payer));
//         assert_eq!(message.header.num_required_signatures, 2);

//         let ix = Instruction::new_with_bincode(
//             program_id,
//             &0,
//             vec![AccountMeta::new(payer, true), AccountMeta::new(id0, true)],
//         );
//         let message = Message::new(&[ix], Some(&payer));
//         assert_eq!(message.header.num_required_signatures, 2);
//     }

//     #[test]
//     fn test_message_program_last() {
//         let program_id = Pubkey::default();
//         let id0 = Pubkey::new_unique();
//         let id1 = Pubkey::new_unique();
//         let keys = get_keys(
//             &[
//                 Instruction::new_with_bincode(
//                     program_id,
//                     &0,
//                     vec![AccountMeta::new_readonly(id0, false)],
//                 ),
//                 Instruction::new_with_bincode(
//                     program_id,
//                     &0,
//                     vec![AccountMeta::new_readonly(id1, true)],
//                 ),
//             ],
//             None,
//         );
//         assert_eq!(
//             keys,
//             InstructionKeys::new(vec![id1], vec![id0, program_id], 1, 2)
//         );
//     }

//     #[test]
//     fn test_program_position() {
//         let program_id0 = Pubkey::default();
//         let program_id1 = Pubkey::new_unique();
//         let id = Pubkey::new_unique();
//         let message = Message::new(
//             &[
//                 Instruction::new_with_bincode(program_id0, &0, vec![AccountMeta::new(id, false)]),
//                 Instruction::new_with_bincode(program_id1, &0, vec![AccountMeta::new(id, true)]),
//             ],
//             Some(&id),
//         );
//         assert_eq!(message.program_position(0), None);
//         assert_eq!(message.program_position(1), Some(0));
//         assert_eq!(message.program_position(2), Some(1));
//     }

//     #[test]
//     fn test_is_writable() {
//         let key0 = Pubkey::new_unique();
//         let key1 = Pubkey::new_unique();
//         let key2 = Pubkey::new_unique();
//         let key3 = Pubkey::new_unique();
//         let key4 = Pubkey::new_unique();
//         let key5 = Pubkey::new_unique();

//         let message = Message {
//             header: MessageHeader {
//                 num_required_signatures: 3,
//                 num_readonly_signed_accounts: 2,
//                 num_readonly_unsigned_accounts: 1,
//             },
//             account_keys: vec![key0, key1, key2, key3, key4, key5],
//             recent_blockhash: Hash::default(),
//             instructions: vec![],
//         };
//         assert!(message.is_writable(0));
//         assert!(!message.is_writable(1));
//         assert!(!message.is_writable(2));
//         assert!(message.is_writable(3));
//         assert!(message.is_writable(4));
//         assert!(!message.is_writable(5));
//     }

//     #[test]
//     fn test_get_account_keys_by_lock_type() {
//         let program_id = Pubkey::default();
//         let id0 = Pubkey::new_unique();
//         let id1 = Pubkey::new_unique();
//         let id2 = Pubkey::new_unique();
//         let id3 = Pubkey::new_unique();
//         let message = Message::new(
//             &[
//                 Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, false)]),
//                 Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id1, true)]),
//                 Instruction::new_with_bincode(
//                     program_id,
//                     &0,
//                     vec![AccountMeta::new_readonly(id2, false)],
//                 ),
//                 Instruction::new_with_bincode(
//                     program_id,
//                     &0,
//                     vec![AccountMeta::new_readonly(id3, true)],
//                 ),
//             ],
//             Some(&id1),
//         );
//         assert_eq!(
//             message.get_account_keys_by_lock_type(),
//             (vec![&id1, &id0], vec![&id3, &id2, &program_id])
//         );
//     }

//     #[test]
//     fn test_program_ids() {
//         let key0 = Pubkey::new_unique();
//         let key1 = Pubkey::new_unique();
//         let loader2 = Pubkey::new_unique();
//         let instructions = vec![CompiledInstruction::new(2, &(), vec![0, 1])];
//         let message = Message::new_with_compiled_instructions(
//             1,
//             0,
//             2,
//             vec![key0, key1, loader2],
//             Hash::default(),
//             instructions,
//         );
//         assert_eq!(message.program_ids(), vec![&loader2]);
//     }

//     #[test]
//     fn test_is_key_passed_to_program() {
//         let key0 = Pubkey::new_unique();
//         let key1 = Pubkey::new_unique();
//         let loader2 = Pubkey::new_unique();
//         let instructions = vec![CompiledInstruction::new(2, &(), vec![0, 1])];
//         let message = Message::new_with_compiled_instructions(
//             1,
//             0,
//             2,
//             vec![key0, key1, loader2],
//             Hash::default(),
//             instructions,
//         );

//         assert!(message.is_key_passed_to_program(0));
//         assert!(message.is_key_passed_to_program(1));
//         assert!(!message.is_key_passed_to_program(2));
//     }

//     #[test]
//     fn test_is_non_loader_key() {
//         let key0 = Pubkey::new_unique();
//         let key1 = Pubkey::new_unique();
//         let loader2 = Pubkey::new_unique();
//         let instructions = vec![CompiledInstruction::new(2, &(), vec![0, 1])];
//         let message = Message::new_with_compiled_instructions(
//             1,
//             0,
//             2,
//             vec![key0, key1, loader2],
//             Hash::default(),
//             instructions,
//         );
//         assert!(message.is_non_loader_key(0));
//         assert!(message.is_non_loader_key(1));
//         assert!(!message.is_non_loader_key(2));
//     }

//     #[test]
//     fn test_message_header_len_constant() {
//         assert_eq!(
//             bincode::serialized_size(&MessageHeader::default()).unwrap() as usize,
//             MESSAGE_HEADER_LENGTH
//         );
//     }

//     #[test]
//     fn test_message_hash() {
//         // when this test fails, it's most likely due to a new serialized format of a message.
//         // in this case, the domain prefix `solana-tx-message-v1` should be updated.
//         let program_id0 = Pubkey::from_str("4uQeVj5tqViQh7yWWGStvkEG1Zmhx6uasJtWCJziofM").unwrap();
//         let program_id1 = Pubkey::from_str("8opHzTAnfzRpPEx21XtnrVTX28YQuCpAjcn1PczScKh").unwrap();
//         let id0 = Pubkey::from_str("CiDwVBFgWV9E5MvXWoLgnEgn2hK7rJikbvfWavzAQz3").unwrap();
//         let id1 = Pubkey::from_str("GcdayuLaLyrdmUu324nahyv33G5poQdLUEZ1nEytDeP").unwrap();
//         let id2 = Pubkey::from_str("LX3EUdRUBUa3TbsYXLEUdj9J3prXkWXvLYSWyYyc2Jj").unwrap();
//         let id3 = Pubkey::from_str("QRSsyMWN1yHT9ir42bgNZUNZ4PdEhcSWCrL2AryKpy5").unwrap();
//         let instructions = vec![
//             Instruction::new_with_bincode(program_id0, &0, vec![AccountMeta::new(id0, false)]),
//             Instruction::new_with_bincode(program_id0, &0, vec![AccountMeta::new(id1, true)]),
//             Instruction::new_with_bincode(
//                 program_id1,
//                 &0,
//                 vec![AccountMeta::new_readonly(id2, false)],
//             ),
//             Instruction::new_with_bincode(
//                 program_id1,
//                 &0,
//                 vec![AccountMeta::new_readonly(id3, true)],
//             ),
//         ];

//         let message = Message::new(&instructions, Some(&id1));
//         assert_eq!(
//             message.hash(),
//             Hash::from_str("CXRH7GHLieaQZRUjH1mpnNnUZQtU4V4RpJpAFgy77i3z").unwrap()
//         )
//     }
// }
