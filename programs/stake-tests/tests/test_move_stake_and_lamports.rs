#![allow(clippy::arithmetic_side_effects)]

// NOTE this is temporarily ported from the bpf stake program repo so MoveStake and MoveLamports can be tested comprehensively
// in the future we will either port *all* instruction tests from bpf stake program and remove existing stakeinstruction tests
// or we will develop a text fixture system that allows fuzzing and obsoletes both existing test suites
// in other words the utility functions in this file should not be broken out into modules or used elsewhere

use {
    agave_feature_set::stake_raise_minimum_delegation_to_1_sol,
    solana_account::{AccountSharedData, ReadableAccount},
    solana_instruction::Instruction,
    solana_keypair::Keypair,
    solana_program_error::{ProgramError, ProgramResult},
    solana_program_test::*,
    solana_pubkey::Pubkey,
    solana_signer::{signers::Signers, Signer},
    solana_stake_interface::{
        self as stake,
        error::StakeError,
        instruction as ixn, program as stake_program,
        state::{Authorized, Lockup, Meta, Stake, StakeStateV2},
    },
    solana_system_interface::{instruction as system_instruction, program as system_program},
    solana_sysvar::stake_history::{self, StakeHistory},
    solana_transaction::Transaction,
    solana_transaction_error::TransactionError,
    solana_vote_program::{
        self, vote_instruction,
        vote_state::{VoteInit, VoteStateV3, VoteStateVersions},
    },
    test_case::test_matrix,
};

const NO_SIGNERS: &[Keypair] = &[];

fn program_test() -> ProgramTest {
    program_test_without_features(&[])
}

fn program_test_without_features(feature_ids: &[Pubkey]) -> ProgramTest {
    let mut program_test = ProgramTest::default();
    for feature_id in feature_ids {
        program_test.deactivate_feature(*feature_id);
    }

    program_test
}

#[derive(Debug, PartialEq)]
struct Accounts {
    validator: Keypair,
    voter: Keypair,
    withdrawer: Keypair,
    vote_account: Keypair,
}

impl Accounts {
    async fn initialize(&self, context: &mut ProgramTestContext) {
        let slot = context.genesis_config().epoch_schedule.first_normal_slot + 1;
        context.warp_to_slot(slot).unwrap();

        create_vote(
            context,
            &self.validator,
            &self.voter.pubkey(),
            &self.withdrawer.pubkey(),
            &self.vote_account,
        );
    }
}

impl Default for Accounts {
    fn default() -> Self {
        Self {
            validator: Keypair::new(),
            voter: Keypair::new(),
            withdrawer: Keypair::new(),
            vote_account: Keypair::new(),
        }
    }
}

fn create_vote(
    context: &mut ProgramTestContext,
    validator: &Keypair,
    voter: &Pubkey,
    withdrawer: &Pubkey,
    vote_account: &Keypair,
) {
    let rent = &context.bank.rent_collector().rent;
    let rent_voter = rent.minimum_balance(VoteStateV3::size_of());

    let mut instructions = vec![system_instruction::create_account(
        &context.payer.pubkey(),
        &validator.pubkey(),
        rent.minimum_balance(0),
        0,
        &system_program::id(),
    )];
    instructions.append(&mut vote_instruction::create_account_with_config(
        &context.payer.pubkey(),
        &vote_account.pubkey(),
        &VoteInit {
            node_pubkey: validator.pubkey(),
            authorized_voter: *voter,
            authorized_withdrawer: *withdrawer,
            ..VoteInit::default()
        },
        rent_voter,
        vote_instruction::CreateVoteAccountConfig {
            space: VoteStateVersions::vote_state_size_of(true) as u64,
            ..Default::default()
        },
    ));

    let transaction = Transaction::new_signed_with_payer(
        &instructions,
        Some(&context.payer.pubkey()),
        &[validator, vote_account, &context.payer],
        context.bank.last_blockhash(),
    );

    // ignore errors for idempotency
    let _ = context.bank.process_transaction(&transaction);
}

fn transfer(context: &mut ProgramTestContext, recipient: &Pubkey, amount: u64) {
    let transaction = Transaction::new_signed_with_payer(
        &[system_instruction::transfer(
            &context.payer.pubkey(),
            recipient,
            amount,
        )],
        Some(&context.payer.pubkey()),
        &[&context.payer],
        context.bank.last_blockhash(),
    );
    context.bank.process_transaction(&transaction).unwrap();
}

fn advance_epoch(context: &mut ProgramTestContext) {
    refresh_blockhash(context);

    let root_slot = context.bank.slot();
    let slots_per_epoch = context.bank.epoch_schedule().slots_per_epoch;
    context.warp_to_slot(root_slot + slots_per_epoch).unwrap();
}

fn refresh_blockhash(_context: &mut ProgramTestContext) {
    // Bank's last_blockhash() doesn't return Result, so no unwrap needed
}

fn get_account(context: &ProgramTestContext, pubkey: &Pubkey) -> AccountSharedData {
    context.bank.get_account(pubkey).expect("account not found")
}

fn get_stake_account(context: &ProgramTestContext, pubkey: &Pubkey) -> (Meta, Option<Stake>, u64) {
    let stake_account = get_account(context, pubkey);
    let lamports = stake_account.lamports();
    match bincode::deserialize::<StakeStateV2>(stake_account.data()).unwrap() {
        StakeStateV2::Initialized(meta) => (meta, None, lamports),
        StakeStateV2::Stake(meta, stake, _) => (meta, Some(stake), lamports),
        StakeStateV2::Uninitialized => panic!("panic: uninitialized"),
        _ => unimplemented!(),
    }
}

fn get_stake_account_rent(context: &ProgramTestContext) -> u64 {
    let rent = &context.bank.rent_collector().rent;
    rent.minimum_balance(std::mem::size_of::<stake::state::StakeStateV2>())
}

fn get_effective_stake(context: &ProgramTestContext, pubkey: &Pubkey) -> u64 {
    let clock = context.bank.clock();
    let stake_history_account = context.bank.get_account(&stake_history::id()).unwrap();
    let stake_history: StakeHistory = bincode::deserialize(stake_history_account.data()).unwrap();
    let stake_account = get_account(context, pubkey);
    match bincode::deserialize::<StakeStateV2>(stake_account.data()).unwrap() {
        StakeStateV2::Stake(_, stake, _) => {
            stake
                .delegation
                .stake_activating_and_deactivating(clock.epoch, &stake_history, Some(0))
                .effective
        }
        _ => 0,
    }
}

fn get_minimum_delegation(context: &mut ProgramTestContext) -> u64 {
    const LAMPORTS_PER_SOL: u64 = 1_000_000_000;
    if context
        .bank
        .feature_set
        .is_active(&stake_raise_minimum_delegation_to_1_sol::id())
    {
        LAMPORTS_PER_SOL // 1 SOL
    } else {
        1 // 1 lamport
    }
}

fn create_blank_stake_account_from_keypair(
    context: &mut ProgramTestContext,
    stake: &Keypair,
) -> Pubkey {
    let lamports = get_stake_account_rent(context);

    let transaction = Transaction::new_signed_with_payer(
        &[system_instruction::create_account(
            &context.payer.pubkey(),
            &stake.pubkey(),
            lamports,
            StakeStateV2::size_of() as u64,
            &stake_program::id(),
        )],
        Some(&context.payer.pubkey()),
        &[&context.payer, stake],
        context.bank.last_blockhash(),
    );

    context.bank.process_transaction(&transaction).unwrap();

    stake.pubkey()
}

async fn process_instruction<T: Signers + ?Sized>(
    context: &mut ProgramTestContext,
    instruction: &Instruction,
    additional_signers: &T,
) -> ProgramResult {
    let mut transaction =
        Transaction::new_with_payer(&[instruction.clone()], Some(&context.payer.pubkey()));

    transaction.partial_sign(&[&context.payer], context.bank.last_blockhash());
    transaction.sign(additional_signers, context.bank.last_blockhash());

    match context.bank.process_transaction(&transaction) {
        Ok(_) => Ok(()),
        Err(e) => {
            // banks client error -> transaction error -> instruction error -> program error
            match e {
                TransactionError::InstructionError(_, e) => Err(e.try_into().unwrap()),
                TransactionError::InsufficientFundsForRent { .. } => {
                    Err(ProgramError::InsufficientFunds)
                }
                _ => panic!("couldnt convert {e:?} to ProgramError"),
            }
        }
    }
}

async fn test_instruction_with_missing_signers(
    context: &mut ProgramTestContext,
    instruction: &Instruction,
    additional_signers: &Vec<&Keypair>,
) {
    // remove every signer one by one and ensure we always fail
    for i in 0..instruction.accounts.len() {
        if instruction.accounts[i].is_signer {
            let mut instruction = instruction.clone();
            instruction.accounts[i].is_signer = false;
            let reduced_signers: Vec<_> = additional_signers
                .iter()
                .filter(|s| s.pubkey() != instruction.accounts[i].pubkey)
                .collect();

            let e = process_instruction(context, &instruction, &reduced_signers)
                .await
                .unwrap_err();
            assert_eq!(e, ProgramError::MissingRequiredSignature);
        }
    }

    // now make sure the instruction succeeds
    process_instruction(context, instruction, additional_signers)
        .await
        .unwrap();
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum StakeLifecycle {
    Uninitialized = 0,
    Initialized,
    Activating,
    Active,
    Deactivating,
    Deactive,
}
impl StakeLifecycle {
    // (stake, staker, withdrawer)
    async fn new_stake_account(
        self,
        context: &mut ProgramTestContext,
        vote_account: &Pubkey,
        staked_amount: u64,
    ) -> (Keypair, Keypair, Keypair) {
        let stake_keypair = Keypair::new();
        let staker_keypair = Keypair::new();
        let withdrawer_keypair = Keypair::new();

        self.new_stake_account_fully_specified(
            context,
            vote_account,
            staked_amount,
            &stake_keypair,
            &staker_keypair,
            &withdrawer_keypair,
            &Lockup::default(),
        )
        .await;

        (stake_keypair, staker_keypair, withdrawer_keypair)
    }

    #[allow(clippy::too_many_arguments)]
    async fn new_stake_account_fully_specified(
        self,
        context: &mut ProgramTestContext,
        vote_account: &Pubkey,
        staked_amount: u64,
        stake_keypair: &Keypair,
        staker_keypair: &Keypair,
        withdrawer_keypair: &Keypair,
        lockup: &Lockup,
    ) {
        let authorized = Authorized {
            staker: staker_keypair.pubkey(),
            withdrawer: withdrawer_keypair.pubkey(),
        };

        let stake = create_blank_stake_account_from_keypair(context, stake_keypair);
        if staked_amount > 0 {
            transfer(context, &stake, staked_amount);
        }

        if self >= StakeLifecycle::Initialized {
            let instruction = ixn::initialize(&stake, &authorized, lockup);
            process_instruction(context, &instruction, NO_SIGNERS)
                .await
                .unwrap();
        }

        if self >= StakeLifecycle::Activating {
            let instruction = ixn::delegate_stake(&stake, &staker_keypair.pubkey(), vote_account);
            process_instruction(context, &instruction, &vec![staker_keypair])
                .await
                .unwrap();
        }

        if self >= StakeLifecycle::Active {
            advance_epoch(context);
            assert_eq!(get_effective_stake(context, &stake), staked_amount,);
        }

        if self >= StakeLifecycle::Deactivating {
            let instruction = ixn::deactivate_stake(&stake, &staker_keypair.pubkey());
            process_instruction(context, &instruction, &vec![staker_keypair])
                .await
                .unwrap();
        }

        if self == StakeLifecycle::Deactive {
            advance_epoch(context);
            assert_eq!(get_effective_stake(context, &stake), 0,);
        }
    }
}

#[test_matrix(
    [program_test(),  program_test_without_features(&[stake_raise_minimum_delegation_to_1_sol::id()])],
    [StakeLifecycle::Initialized, StakeLifecycle::Activating, StakeLifecycle::Active,
     StakeLifecycle::Deactivating, StakeLifecycle::Deactive],
    [StakeLifecycle::Initialized, StakeLifecycle::Activating, StakeLifecycle::Active,
     StakeLifecycle::Deactivating, StakeLifecycle::Deactive],
    [false, true],
    [false, true]
)]
#[tokio::test]
async fn test_move_stake(
    program_test: ProgramTest,
    move_source_type: StakeLifecycle,
    move_dest_type: StakeLifecycle,
    full_move: bool,
    has_lockup: bool,
) {
    let mut context = program_test.start_with_context().await;
    let accounts = Accounts::default();
    accounts.initialize(&mut context).await;

    let rent_exempt_reserve = get_stake_account_rent(&context);
    let minimum_delegation = get_minimum_delegation(&mut context);

    // source has 2x minimum so we can easily test an unfunded destination
    let source_staked_amount = minimum_delegation * 2;

    // this is the amount of *staked* lamports for test checks
    // destinations may have excess lamports but these are *never* activated by move
    let dest_staked_amount = if move_dest_type == StakeLifecycle::Active {
        minimum_delegation
    } else {
        0
    };

    // test with and without lockup. both of these cases pass, we test failures elsewhere
    let lockup = if has_lockup {
        let clock = context.bank.clock();
        let lockup = Lockup {
            unix_timestamp: 0,
            epoch: clock.epoch + 100,
            custodian: Pubkey::new_unique(),
        };

        assert!(lockup.is_in_force(&clock, None));
        lockup
    } else {
        Lockup::default()
    };

    // we put an extra minimum in every account, unstaked, to test that no new lamports activate
    // name them here so our asserts are readable
    let source_excess = minimum_delegation;
    let dest_excess = minimum_delegation;

    let move_source_keypair = Keypair::new();
    let move_dest_keypair = Keypair::new();
    let staker_keypair = Keypair::new();
    let withdrawer_keypair = Keypair::new();

    // create source stake
    move_source_type
        .new_stake_account_fully_specified(
            &mut context,
            &accounts.vote_account.pubkey(),
            source_staked_amount,
            &move_source_keypair,
            &staker_keypair,
            &withdrawer_keypair,
            &lockup,
        )
        .await;
    let move_source = move_source_keypair.pubkey();
    let mut source_account = get_account(&context, &move_source);
    let mut source_stake_state: StakeStateV2 = bincode::deserialize(source_account.data()).unwrap();

    // create dest stake with same authorities
    move_dest_type
        .new_stake_account_fully_specified(
            &mut context,
            &accounts.vote_account.pubkey(),
            minimum_delegation,
            &move_dest_keypair,
            &staker_keypair,
            &withdrawer_keypair,
            &lockup,
        )
        .await;
    let move_dest = move_dest_keypair.pubkey();

    // true up source epoch if transient
    if move_source_type == StakeLifecycle::Activating
        || move_source_type == StakeLifecycle::Deactivating
    {
        let clock = context.bank.clock();
        if let StakeStateV2::Stake(_, ref mut stake, _) = &mut source_stake_state {
            match move_source_type {
                StakeLifecycle::Activating => stake.delegation.activation_epoch = clock.epoch,
                StakeLifecycle::Deactivating => stake.delegation.deactivation_epoch = clock.epoch,
                _ => (),
            }
        }

        source_account.set_data(bincode::serialize(&source_stake_state).unwrap());
        context.set_account(&move_source, &source_account);
    }

    // our inactive accounts have extra lamports, lets not let active feel left out
    if move_dest_type == StakeLifecycle::Active {
        transfer(&mut context, &move_dest, dest_excess);
    }

    // hey why not spread the love around to everyone
    transfer(&mut context, &move_source, source_excess);

    // alright first things first, clear out all the state failures
    match (move_source_type, move_dest_type) {
        // valid
        (StakeLifecycle::Active, StakeLifecycle::Initialized)
        | (StakeLifecycle::Active, StakeLifecycle::Active)
        | (StakeLifecycle::Active, StakeLifecycle::Deactive) => (),
        // invalid! get outta my test
        _ => {
            let instruction = ixn::move_stake(
                &move_source,
                &move_dest,
                &staker_keypair.pubkey(),
                if full_move {
                    source_staked_amount
                } else {
                    minimum_delegation
                },
            );

            // this is InvalidAccountData sometimes and Custom(5) sometimes but i dont care
            process_instruction(&mut context, &instruction, &vec![&staker_keypair])
                .await
                .unwrap_err();
            return;
        }
    }

    // the below checks are conceptually incoherent with a 1 lamport minimum
    // the undershoot fails successfully (but because its a zero move, not because the destination ends underfunded)
    // then the second one succeeds failedly (because its a full move, so the "underfunded" source is actually closed)
    if minimum_delegation > 1 {
        // source has 2x minimum (always 2 sol because these tests dont have featuresets)
        // so first for inactive accounts lets undershoot and fail for underfunded dest
        if move_dest_type != StakeLifecycle::Active {
            let instruction = ixn::move_stake(
                &move_source,
                &move_dest,
                &staker_keypair.pubkey(),
                minimum_delegation - 1,
            );

            let e = process_instruction(&mut context, &instruction, &vec![&staker_keypair])
                .await
                .unwrap_err();
            assert_eq!(e, ProgramError::InvalidArgument);
        }

        // now lets overshoot and fail for underfunded source
        let instruction = ixn::move_stake(
            &move_source,
            &move_dest,
            &staker_keypair.pubkey(),
            minimum_delegation + 1,
        );

        let e = process_instruction(&mut context, &instruction, &vec![&staker_keypair])
            .await
            .unwrap_err();
        assert_eq!(e, ProgramError::InvalidArgument);
    }

    // now we do it juuust right
    let instruction = ixn::move_stake(
        &move_source,
        &move_dest,
        &staker_keypair.pubkey(),
        if full_move {
            source_staked_amount
        } else {
            minimum_delegation
        },
    );

    test_instruction_with_missing_signers(&mut context, &instruction, &vec![&staker_keypair]).await;

    if full_move {
        let (_, option_source_stake, source_lamports) = get_stake_account(&context, &move_source);

        // source is deactivated and rent/excess stay behind
        assert!(option_source_stake.is_none());
        assert_eq!(source_lamports, source_excess + rent_exempt_reserve);

        let (_, Some(dest_stake), dest_lamports) = get_stake_account(&context, &move_dest) else {
            panic!("dest should be active")
        };
        let dest_effective_stake = get_effective_stake(&context, &move_dest);

        // dest captured the entire source delegation, kept its rent/excess, didnt activate its excess
        assert_eq!(
            dest_stake.delegation.stake,
            source_staked_amount + dest_staked_amount
        );
        assert_eq!(dest_effective_stake, dest_stake.delegation.stake);
        assert_eq!(
            dest_lamports,
            dest_effective_stake + dest_excess + rent_exempt_reserve
        );
    } else {
        let (_, Some(source_stake), source_lamports) = get_stake_account(&context, &move_source)
        else {
            panic!("source should be active")
        };
        let source_effective_stake = get_effective_stake(&context, &move_source);

        // half of source delegation moved over, excess stayed behind
        assert_eq!(source_stake.delegation.stake, source_staked_amount / 2);
        assert_eq!(source_effective_stake, source_stake.delegation.stake);
        assert_eq!(
            source_lamports,
            source_effective_stake + source_excess + rent_exempt_reserve
        );

        let (_, Some(dest_stake), dest_lamports) = get_stake_account(&context, &move_dest) else {
            panic!("dest should be active")
        };
        let dest_effective_stake = get_effective_stake(&context, &move_dest);

        // dest mirrors our observations
        assert_eq!(
            dest_stake.delegation.stake,
            source_staked_amount / 2 + dest_staked_amount
        );
        assert_eq!(dest_effective_stake, dest_stake.delegation.stake);
        assert_eq!(
            dest_lamports,
            dest_effective_stake + dest_excess + rent_exempt_reserve
        );
    }
}

#[test_matrix(
    [program_test(),  program_test_without_features(&[stake_raise_minimum_delegation_to_1_sol::id()])],
    [StakeLifecycle::Initialized, StakeLifecycle::Activating, StakeLifecycle::Active,
     StakeLifecycle::Deactivating, StakeLifecycle::Deactive],
    [StakeLifecycle::Initialized, StakeLifecycle::Activating, StakeLifecycle::Active,
     StakeLifecycle::Deactivating, StakeLifecycle::Deactive],
    [false, true],
    [false, true]
)]
#[tokio::test]
async fn test_move_lamports(
    program_test: ProgramTest,
    move_source_type: StakeLifecycle,
    move_dest_type: StakeLifecycle,
    different_votes: bool,
    has_lockup: bool,
) {
    let mut context = program_test.start_with_context().await;
    let accounts = Accounts::default();
    accounts.initialize(&mut context).await;

    let rent_exempt_reserve = get_stake_account_rent(&context);
    let minimum_delegation = get_minimum_delegation(&mut context);

    // put minimum in both accounts if theyre active
    let source_staked_amount = if move_source_type == StakeLifecycle::Active {
        minimum_delegation
    } else {
        0
    };

    let dest_staked_amount = if move_dest_type == StakeLifecycle::Active {
        minimum_delegation
    } else {
        0
    };

    // test with and without lockup. both of these cases pass, we test failures elsewhere
    let lockup = if has_lockup {
        let clock = context.bank.clock();
        let lockup = Lockup {
            unix_timestamp: 0,
            epoch: clock.epoch + 100,
            custodian: Pubkey::new_unique(),
        };

        assert!(lockup.is_in_force(&clock, None));
        lockup
    } else {
        Lockup::default()
    };

    // we put an extra minimum in every account, unstaked, to test moving them
    let source_excess = minimum_delegation;
    let dest_excess = minimum_delegation;

    let move_source_keypair = Keypair::new();
    let move_dest_keypair = Keypair::new();
    let staker_keypair = Keypair::new();
    let withdrawer_keypair = Keypair::new();

    // make a separate vote account if needed
    let dest_vote_account = if different_votes {
        let vote_account = Keypair::new();
        create_vote(
            &mut context,
            &Keypair::new(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &vote_account,
        );

        vote_account.pubkey()
    } else {
        accounts.vote_account.pubkey()
    };

    // create source stake
    move_source_type
        .new_stake_account_fully_specified(
            &mut context,
            &accounts.vote_account.pubkey(),
            minimum_delegation,
            &move_source_keypair,
            &staker_keypair,
            &withdrawer_keypair,
            &lockup,
        )
        .await;
    let move_source = move_source_keypair.pubkey();
    let mut source_account = get_account(&context, &move_source);
    let mut source_stake_state: StakeStateV2 = bincode::deserialize(source_account.data()).unwrap();

    // create dest stake with same authorities
    move_dest_type
        .new_stake_account_fully_specified(
            &mut context,
            &dest_vote_account,
            minimum_delegation,
            &move_dest_keypair,
            &staker_keypair,
            &withdrawer_keypair,
            &lockup,
        )
        .await;
    let move_dest = move_dest_keypair.pubkey();

    // true up source epoch if transient
    if move_source_type == StakeLifecycle::Activating
        || move_source_type == StakeLifecycle::Deactivating
    {
        let clock = context.bank.clock();
        if let StakeStateV2::Stake(_, ref mut stake, _) = &mut source_stake_state {
            match move_source_type {
                StakeLifecycle::Activating => stake.delegation.activation_epoch = clock.epoch,
                StakeLifecycle::Deactivating => stake.delegation.deactivation_epoch = clock.epoch,
                _ => (),
            }
        }

        source_account.set_data(bincode::serialize(&source_stake_state).unwrap());
        context.set_account(&move_source, &source_account);
    }

    // if we activated the initial amount we need to top up with the test lamports
    if move_source_type == StakeLifecycle::Active {
        transfer(&mut context, &move_source, source_excess);
    }
    if move_dest_type == StakeLifecycle::Active {
        transfer(&mut context, &move_dest, dest_excess);
    }

    // clear out state failures
    if move_source_type == StakeLifecycle::Activating
        || move_source_type == StakeLifecycle::Deactivating
        || move_dest_type == StakeLifecycle::Deactivating
    {
        let instruction = ixn::move_lamports(
            &move_source,
            &move_dest,
            &staker_keypair.pubkey(),
            source_excess,
        );

        process_instruction(&mut context, &instruction, &vec![&staker_keypair])
            .await
            .unwrap_err();
        return;
    }

    // overshoot and fail for underfunded source
    let instruction = ixn::move_lamports(
        &move_source,
        &move_dest,
        &staker_keypair.pubkey(),
        source_excess + 1,
    );

    let e = process_instruction(&mut context, &instruction, &vec![&staker_keypair])
        .await
        .unwrap_err();
    assert_eq!(e, ProgramError::InvalidArgument);

    let (_, _, before_source_lamports) = get_stake_account(&context, &move_source);
    let (_, _, before_dest_lamports) = get_stake_account(&context, &move_dest);

    // now properly move the full excess
    let instruction = ixn::move_lamports(
        &move_source,
        &move_dest,
        &staker_keypair.pubkey(),
        source_excess,
    );

    test_instruction_with_missing_signers(&mut context, &instruction, &vec![&staker_keypair]).await;

    let (_, _, after_source_lamports) = get_stake_account(&context, &move_source);
    let source_effective_stake = get_effective_stake(&context, &move_source);

    // source activation didnt change
    assert_eq!(source_effective_stake, source_staked_amount);

    // source lamports are right
    assert_eq!(
        after_source_lamports,
        before_source_lamports - minimum_delegation
    );
    assert_eq!(
        after_source_lamports,
        source_effective_stake + rent_exempt_reserve
    );

    let (_, _, after_dest_lamports) = get_stake_account(&context, &move_dest);
    let dest_effective_stake = get_effective_stake(&context, &move_dest);

    // dest activation didnt change
    assert_eq!(dest_effective_stake, dest_staked_amount);

    // dest lamports are right
    assert_eq!(
        after_dest_lamports,
        before_dest_lamports + minimum_delegation
    );
    assert_eq!(
        after_dest_lamports,
        dest_effective_stake + rent_exempt_reserve + source_excess + dest_excess
    );
}

#[test_matrix(
    [program_test(),  program_test_without_features(&[stake_raise_minimum_delegation_to_1_sol::id()])],
    [(StakeLifecycle::Active, StakeLifecycle::Uninitialized),
     (StakeLifecycle::Uninitialized, StakeLifecycle::Initialized),
     (StakeLifecycle::Uninitialized, StakeLifecycle::Uninitialized)],
    [false, true]
)]
#[tokio::test]
async fn test_move_uninitialized_fail(
    program_test: ProgramTest,
    move_types: (StakeLifecycle, StakeLifecycle),
    move_lamports: bool,
) {
    let mut context = program_test.start_with_context().await;
    let accounts = Accounts::default();
    accounts.initialize(&mut context).await;

    let minimum_delegation = get_minimum_delegation(&mut context);
    let source_staked_amount = minimum_delegation * 2;

    let (move_source_type, move_dest_type) = move_types;

    let (move_source_keypair, staker_keypair, withdrawer_keypair) = move_source_type
        .new_stake_account(
            &mut context,
            &accounts.vote_account.pubkey(),
            source_staked_amount,
        )
        .await;
    let move_source = move_source_keypair.pubkey();

    let move_dest_keypair = Keypair::new();
    move_dest_type
        .new_stake_account_fully_specified(
            &mut context,
            &accounts.vote_account.pubkey(),
            0,
            &move_dest_keypair,
            &staker_keypair,
            &withdrawer_keypair,
            &Lockup::default(),
        )
        .await;
    let move_dest = move_dest_keypair.pubkey();

    let source_signer = if move_source_type == StakeLifecycle::Uninitialized {
        &move_source_keypair
    } else {
        &staker_keypair
    };

    let instruction = if move_lamports {
        ixn::move_lamports(
            &move_source,
            &move_dest,
            &source_signer.pubkey(),
            minimum_delegation,
        )
    } else {
        ixn::move_stake(
            &move_source,
            &move_dest,
            &source_signer.pubkey(),
            minimum_delegation,
        )
    };

    let e = process_instruction(&mut context, &instruction, &vec![source_signer])
        .await
        .unwrap_err();
    assert_eq!(e, ProgramError::InvalidAccountData);
}

#[test_matrix(
    [program_test(),  program_test_without_features(&[stake_raise_minimum_delegation_to_1_sol::id()])],
    [StakeLifecycle::Initialized, StakeLifecycle::Active, StakeLifecycle::Deactive],
    [StakeLifecycle::Initialized, StakeLifecycle::Activating, StakeLifecycle::Active, StakeLifecycle::Deactive],
    [false, true]
)]
#[tokio::test]
async fn test_move_general_fail(
    program_test: ProgramTest,
    move_source_type: StakeLifecycle,
    move_dest_type: StakeLifecycle,
    move_lamports: bool,
) {
    // the test_matrix includes all valid source/dest combinations for MoveLamports
    // we dont test invalid combinations because they would fail regardless of the fail cases we test here
    // valid source/dest for MoveStake are a strict subset of MoveLamports
    // source must be active, and dest must be active or inactive. so we skip the additional invalid MoveStake cases
    if !move_lamports
        && (move_source_type != StakeLifecycle::Active
            || move_dest_type == StakeLifecycle::Activating)
    {
        return;
    }

    let mut context = program_test.start_with_context().await;
    let accounts = Accounts::default();
    accounts.initialize(&mut context).await;

    let minimum_delegation = get_minimum_delegation(&mut context);
    let source_staked_amount = minimum_delegation * 2;

    let in_force_lockup = {
        let clock = context.bank.clock();
        Lockup {
            unix_timestamp: 0,
            epoch: clock.epoch + 1_000_000,
            custodian: Pubkey::new_unique(),
        }
    };

    let mk_ixn = if move_lamports {
        ixn::move_lamports
    } else {
        ixn::move_stake
    };

    // we can reuse source but will need a lot of dest
    let (move_source_keypair, staker_keypair, withdrawer_keypair) = move_source_type
        .new_stake_account(
            &mut context,
            &accounts.vote_account.pubkey(),
            source_staked_amount,
        )
        .await;
    let move_source = move_source_keypair.pubkey();
    transfer(&mut context, &move_source, minimum_delegation);

    // self-move fails
    // NOTE this error type is an artifact of the native program interface
    // when we move to bpf, it should actually hit the processor error
    let instruction = mk_ixn(
        &move_source,
        &move_source,
        &staker_keypair.pubkey(),
        minimum_delegation,
    );
    let e = process_instruction(&mut context, &instruction, &vec![&staker_keypair])
        .await
        .unwrap_err();
    assert_eq!(e, ProgramError::AccountBorrowFailed);

    // first we make a "normal" move dest
    {
        let move_dest_keypair = Keypair::new();
        move_dest_type
            .new_stake_account_fully_specified(
                &mut context,
                &accounts.vote_account.pubkey(),
                minimum_delegation,
                &move_dest_keypair,
                &staker_keypair,
                &withdrawer_keypair,
                &Lockup::default(),
            )
            .await;
        let move_dest = move_dest_keypair.pubkey();

        // zero move fails
        let instruction = mk_ixn(&move_source, &move_dest, &staker_keypair.pubkey(), 0);
        let e = process_instruction(&mut context, &instruction, &vec![&staker_keypair])
            .await
            .unwrap_err();
        assert_eq!(e, ProgramError::InvalidArgument);

        // sign with withdrawer fails
        let instruction = mk_ixn(
            &move_source,
            &move_dest,
            &withdrawer_keypair.pubkey(),
            minimum_delegation,
        );
        let e = process_instruction(&mut context, &instruction, &vec![&withdrawer_keypair])
            .await
            .unwrap_err();
        assert_eq!(e, ProgramError::MissingRequiredSignature);

        // good place to test source lockup
        let move_locked_source_keypair = Keypair::new();
        move_source_type
            .new_stake_account_fully_specified(
                &mut context,
                &accounts.vote_account.pubkey(),
                source_staked_amount,
                &move_locked_source_keypair,
                &staker_keypair,
                &withdrawer_keypair,
                &in_force_lockup,
            )
            .await;
        let move_locked_source = move_locked_source_keypair.pubkey();
        transfer(&mut context, &move_locked_source, minimum_delegation);

        let instruction = mk_ixn(
            &move_locked_source,
            &move_dest,
            &staker_keypair.pubkey(),
            minimum_delegation,
        );
        let e = process_instruction(&mut context, &instruction, &vec![&staker_keypair])
            .await
            .unwrap_err();
        assert_eq!(e, StakeError::MergeMismatch.into());
    }

    // staker mismatch
    {
        let move_dest_keypair = Keypair::new();
        let throwaway = Keypair::new();
        move_dest_type
            .new_stake_account_fully_specified(
                &mut context,
                &accounts.vote_account.pubkey(),
                minimum_delegation,
                &move_dest_keypair,
                &throwaway,
                &withdrawer_keypair,
                &Lockup::default(),
            )
            .await;
        let move_dest = move_dest_keypair.pubkey();

        let instruction = mk_ixn(
            &move_source,
            &move_dest,
            &staker_keypair.pubkey(),
            minimum_delegation,
        );
        let e = process_instruction(&mut context, &instruction, &vec![&staker_keypair])
            .await
            .unwrap_err();
        assert_eq!(e, StakeError::MergeMismatch.into());

        let instruction = mk_ixn(
            &move_source,
            &move_dest,
            &throwaway.pubkey(),
            minimum_delegation,
        );
        let e = process_instruction(&mut context, &instruction, &vec![&throwaway])
            .await
            .unwrap_err();
        assert_eq!(e, ProgramError::MissingRequiredSignature);
    }

    // withdrawer mismatch
    {
        let move_dest_keypair = Keypair::new();
        let throwaway = Keypair::new();
        move_dest_type
            .new_stake_account_fully_specified(
                &mut context,
                &accounts.vote_account.pubkey(),
                minimum_delegation,
                &move_dest_keypair,
                &staker_keypair,
                &throwaway,
                &Lockup::default(),
            )
            .await;
        let move_dest = move_dest_keypair.pubkey();

        let instruction = mk_ixn(
            &move_source,
            &move_dest,
            &staker_keypair.pubkey(),
            minimum_delegation,
        );
        let e = process_instruction(&mut context, &instruction, &vec![&staker_keypair])
            .await
            .unwrap_err();
        assert_eq!(e, StakeError::MergeMismatch.into());

        let instruction = mk_ixn(
            &move_source,
            &move_dest,
            &throwaway.pubkey(),
            minimum_delegation,
        );
        let e = process_instruction(&mut context, &instruction, &vec![&throwaway])
            .await
            .unwrap_err();
        assert_eq!(e, ProgramError::MissingRequiredSignature);
    }

    // dest lockup
    {
        let move_dest_keypair = Keypair::new();
        move_dest_type
            .new_stake_account_fully_specified(
                &mut context,
                &accounts.vote_account.pubkey(),
                minimum_delegation,
                &move_dest_keypair,
                &staker_keypair,
                &withdrawer_keypair,
                &in_force_lockup,
            )
            .await;
        let move_dest = move_dest_keypair.pubkey();

        let instruction = mk_ixn(
            &move_source,
            &move_dest,
            &staker_keypair.pubkey(),
            minimum_delegation,
        );
        let e = process_instruction(&mut context, &instruction, &vec![&staker_keypair])
            .await
            .unwrap_err();
        assert_eq!(e, StakeError::MergeMismatch.into());
    }

    // lastly we test different vote accounts for move_stake
    if !move_lamports && move_dest_type == StakeLifecycle::Active {
        let dest_vote_account_keypair = Keypair::new();
        create_vote(
            &mut context,
            &Keypair::new(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &dest_vote_account_keypair,
        );

        let move_dest_keypair = Keypair::new();
        move_dest_type
            .new_stake_account_fully_specified(
                &mut context,
                &dest_vote_account_keypair.pubkey(),
                minimum_delegation,
                &move_dest_keypair,
                &staker_keypair,
                &withdrawer_keypair,
                &Lockup::default(),
            )
            .await;
        let move_dest = move_dest_keypair.pubkey();

        let instruction = mk_ixn(
            &move_source,
            &move_dest,
            &staker_keypair.pubkey(),
            minimum_delegation,
        );
        let e = process_instruction(&mut context, &instruction, &vec![&staker_keypair])
            .await
            .unwrap_err();
        assert_eq!(e, StakeError::VoteAddressMismatch.into());
    }
}
