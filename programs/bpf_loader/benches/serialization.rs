use {
    criterion::{criterion_group, criterion_main, Criterion},
    solana_account::AccountSharedData,
    solana_program_runtime::serialization::serialize_parameters,
    solana_pubkey::Pubkey,
    solana_rent::Rent,
    solana_sdk_ids::{bpf_loader, bpf_loader_deprecated},
    solana_transaction_context::{IndexOfAccount, InstructionAccount, TransactionContext},
};

fn create_account(lamports: u64, data: Vec<u8>, owner: &Pubkey, executable: bool, rent_epoch: u64) -> AccountSharedData {
    let mut account = AccountSharedData::new(lamports, data.len(), owner);
    account.set_data_from_slice(&data);
    account.set_executable(executable);
    account.set_rent_epoch(rent_epoch);
    account
}

fn create_inputs(owner: Pubkey, num_instruction_accounts: usize) -> TransactionContext {
    let program_id = solana_pubkey::new_rand();
    let transaction_accounts = vec![
        (
            program_id,
            create_account(0, vec![], &owner, true, 0),
        ),
        (
            solana_pubkey::new_rand(),
            create_account(1, vec![1u8; 100000], &owner, false, 100),
        ),
        (
            solana_pubkey::new_rand(),
            create_account(2, vec![11u8; 100000], &owner, true, 200),
        ),
        (
            solana_pubkey::new_rand(),
            create_account(3, vec![], &owner, false, 3100),
        ),
        (
            solana_pubkey::new_rand(),
            create_account(4, vec![1u8; 100000], &owner, false, 100),
        ),
        (
            solana_pubkey::new_rand(),
            create_account(5, vec![11u8; 10000], &owner, true, 200),
        ),
        (
            solana_pubkey::new_rand(),
            create_account(6, vec![], &owner, false, 3100),
        ),
    ];
    let mut instruction_accounts: Vec<InstructionAccount> = Vec::new();
    for (instruction_account_index, index_in_transaction) in [1, 1, 2, 3, 4, 4, 5, 6]
        .into_iter()
        .cycle()
        .take(num_instruction_accounts)
        .enumerate()
    {
        let index_in_callee = instruction_accounts
            .iter()
            .position(|account| account.index_in_transaction == index_in_transaction)
            .unwrap_or(instruction_account_index) as IndexOfAccount;
        instruction_accounts.push(InstructionAccount {
            index_in_caller: instruction_account_index as IndexOfAccount,
            index_in_transaction,
            index_in_callee,
            is_signer: false,
            is_writable: instruction_account_index >= 4,
        });
    }

    let mut transaction_context =
        TransactionContext::new(transaction_accounts, Rent::default(), 1, 1);
    let instruction_data = vec![1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
    transaction_context
        .get_next_instruction_context()
        .unwrap()
        .configure(&[0], &instruction_accounts, &instruction_data);
    transaction_context.push().unwrap();
    transaction_context
}

fn bench_serialize_unaligned(c: &mut Criterion) {
    let transaction_context = create_inputs(bpf_loader_deprecated::id(), 7);
    let instruction_context = transaction_context
        .get_current_instruction_context()
        .unwrap();

    c.bench_function("serialize_unaligned", |b| {
        b.iter(|| {
            let _ = serialize_parameters(
                &transaction_context,
                instruction_context,
                false,
                true, // mask_out_rent_epoch_in_vm_serialization
            )
            .unwrap();
        });
    });
}

fn bench_serialize_unaligned_copy_account_data(c: &mut Criterion) {
    let transaction_context = create_inputs(bpf_loader_deprecated::id(), 7);
    let instruction_context = transaction_context
        .get_current_instruction_context()
        .unwrap();
    c.bench_function("serialize_unaligned_copy_account_data", |b| {
        b.iter(|| {
            let _ = serialize_parameters(
                &transaction_context,
                instruction_context,
                true,
                true, // mask_out_rent_epoch_in_vm_serialization
            )
            .unwrap();
        });
    });
}

fn bench_serialize_aligned(c: &mut Criterion) {
    let transaction_context = create_inputs(bpf_loader::id(), 7);
    let instruction_context = transaction_context
        .get_current_instruction_context()
        .unwrap();

    c.bench_function("serialize_aligned", |b| {
        b.iter(|| {
            let _ = serialize_parameters(
                &transaction_context,
                instruction_context,
                false,
                true, // mask_out_rent_epoch_in_vm_serialization
            )
            .unwrap();
        });
    });
}

fn bench_serialize_aligned_copy_account_data(c: &mut Criterion) {
    let transaction_context = create_inputs(bpf_loader::id(), 7);
    let instruction_context = transaction_context
        .get_current_instruction_context()
        .unwrap();

    c.bench_function("serialize_aligned_copy_account_data", |b| {
        b.iter(|| {
            let _ = serialize_parameters(
                &transaction_context,
                instruction_context,
                true,
                true, // mask_out_rent_epoch_in_vm_serialization
            )
            .unwrap();
        });
    });
}

fn bench_serialize_unaligned_max_accounts(c: &mut Criterion) {
    let transaction_context = create_inputs(bpf_loader_deprecated::id(), 255);
    let instruction_context = transaction_context
        .get_current_instruction_context()
        .unwrap();

    c.bench_function("serialize_unaligned_max_accounts", |b| {
        b.iter(|| {
            let _ = serialize_parameters(
                &transaction_context,
                instruction_context,
                false,
                true, // mask_out_rent_epoch_in_vm_serialization
            )
            .unwrap();
        });
    });
}

fn bench_serialize_aligned_max_accounts(c: &mut Criterion) {
    let transaction_context = create_inputs(bpf_loader::id(), 255);
    let instruction_context = transaction_context
        .get_current_instruction_context()
        .unwrap();

    c.bench_function("serialize_aligned_max_accounts", |b| {
        b.iter(|| {
            let _ = serialize_parameters(
                &transaction_context,
                instruction_context,
                false,
                true, // mask_out_rent_epoch_in_vm_serialization
            )
            .unwrap();
        });
    });
}

criterion_group!(
    benches,
    bench_serialize_unaligned,
    bench_serialize_unaligned_copy_account_data,
    bench_serialize_aligned,
    bench_serialize_aligned_copy_account_data,
    bench_serialize_unaligned_max_accounts,
    bench_serialize_aligned_max_accounts
);
criterion_main!(benches);
