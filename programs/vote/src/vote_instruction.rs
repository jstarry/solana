//! Vote program
//! Receive and processes votes from validators

use {
    crate::{
        id,
        vote_state::{self, Vote, VoteAuthorize, VoteInit, VoteState, VoteStateUpdate},
    },
    log::*,
    num_derive::{FromPrimitive, ToPrimitive},
    serde_derive::{Deserialize, Serialize},
    solana_metrics::inc_new_counter_info,
    solana_program_runtime::invoke_context::InvokeContext,
    solana_sdk::{
        decode_error::DecodeError,
        feature_set,
        hash::Hash,
        instruction::{AccountMeta, Instruction, InstructionError},
        keyed_account::{
            check_sysvar_keyed_account, from_keyed_account, get_signers, keyed_account_at_index,
            KeyedAccount,
        },
        program_utils::limited_deserialize,
        pubkey::Pubkey,
        system_instruction,
        sysvar::{self, clock::Clock, rent::Rent, slot_hashes::SlotHashes},
    },
    std::{collections::HashSet, str::FromStr, sync::Arc},
    thiserror::Error,
};

/// Reasons the stake might have had an error
#[derive(Error, Debug, Clone, PartialEq, FromPrimitive, ToPrimitive)]
pub enum VoteError {
    #[error("vote already recorded or not in slot hashes history")]
    VoteTooOld,

    #[error("vote slots do not match bank history")]
    SlotsMismatch,

    #[error("vote hash does not match bank hash")]
    SlotHashMismatch,

    #[error("vote has no slots, invalid")]
    EmptySlots,

    #[error("vote timestamp not recent")]
    TimestampTooOld,

    #[error("authorized voter has already been changed this epoch")]
    TooSoonToReauthorize,

    // TODO: figure out how to migrate these new errors
    #[error("Old state had vote which should not have been popped off by vote in new state")]
    LockoutConflict,

    #[error("Proposed state had earlier slot which should have been popped off by later vote")]
    NewVoteStateLockoutMismatch,

    #[error("Vote slots are not ordered")]
    SlotsNotOrdered,

    #[error("Confirmations are not ordered")]
    ConfirmationsNotOrdered,

    #[error("Zero confirmations")]
    ZeroConfirmations,

    #[error("Confirmation exceeds limit")]
    ConfirmationTooLarge,

    #[error("Root rolled back")]
    RootRollBack,

    #[error("Confirmations for same vote were smaller in new proposed state")]
    ConfirmationRollBack,

    #[error("New state contained a vote slot smaller than the root")]
    SlotSmallerThanRoot,

    #[error("New state contained too many votes")]
    TooManyVotes,
}

impl<E> DecodeError<E> for VoteError {
    fn type_of() -> &'static str {
        "VoteError"
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub enum VoteInstruction {
    /// Initialize a vote account
    ///
    /// # Account references
    ///   0. `[WRITE]` Uninitialized vote account
    ///   1. `[]` Rent sysvar
    ///   2. `[]` Clock sysvar
    ///   3. `[SIGNER]` New validator identity (node_pubkey)
    InitializeAccount(VoteInit),

    /// Authorize a key to send votes or issue a withdrawal
    ///
    /// # Account references
    ///   0. `[WRITE]` Vote account to be updated with the Pubkey for authorization
    ///   1. `[]` Clock sysvar
    ///   2. `[SIGNER]` Vote or withdraw authority
    Authorize(Pubkey, VoteAuthorize),

    /// A Vote instruction with recent votes
    ///
    /// # Account references
    ///   0. `[WRITE]` Vote account to vote with
    ///   1. `[]` Slot hashes sysvar
    ///   2. `[]` Clock sysvar
    ///   3. `[SIGNER]` Vote authority
    Vote(Vote),

    /// Withdraw some amount of funds
    ///
    /// # Account references
    ///   0. `[WRITE]` Vote account to withdraw from
    ///   1. `[WRITE]` Recipient account
    ///   2. `[SIGNER]` Withdraw authority
    Withdraw(u64),

    /// Update the vote account's validator identity (node_pubkey)
    ///
    /// # Account references
    ///   0. `[WRITE]` Vote account to be updated with the given authority public key
    ///   1. `[SIGNER]` New validator identity (node_pubkey)
    ///   2. `[SIGNER]` Withdraw authority
    UpdateValidatorIdentity,

    /// Update the commission for the vote account
    ///
    /// # Account references
    ///   0. `[WRITE]` Vote account to be updated
    ///   1. `[SIGNER]` Withdraw authority
    UpdateCommission(u8),

    /// A Vote instruction with recent votes
    ///
    /// # Account references
    ///   0. `[WRITE]` Vote account to vote with
    ///   1. `[]` Slot hashes sysvar
    ///   2. `[]` Clock sysvar
    ///   3. `[SIGNER]` Vote authority
    VoteSwitch(Vote, Hash),

    /// Authorize a key to send votes or issue a withdrawal
    ///
    /// This instruction behaves like `Authorize` with the additional requirement that the new vote
    /// or withdraw authority must also be a signer.
    ///
    /// # Account references
    ///   0. `[WRITE]` Vote account to be updated with the Pubkey for authorization
    ///   1. `[]` Clock sysvar
    ///   2. `[SIGNER]` Vote or withdraw authority
    ///   3. `[SIGNER]` New vote or withdraw authority
    AuthorizeChecked(VoteAuthorize),

    /// Update the onchain vote state for the signer.
    ///
    /// # Account references
    ///   0. `[Write]` Vote account to vote with
    ///   1. `[SIGNER]` Vote authority
    UpdateVoteState(VoteStateUpdate),

    /// Update the onchain vote state for the signer along with a switching proof.
    ///
    /// # Account references
    ///   0. `[Write]` Vote account to vote with
    ///   1. `[SIGNER]` Vote authority
    UpdateVoteStateSwitch(VoteStateUpdate, Hash),
}

fn initialize_account(vote_pubkey: &Pubkey, vote_init: &VoteInit) -> Instruction {
    let account_metas = vec![
        AccountMeta::new(*vote_pubkey, false),
        AccountMeta::new_readonly(sysvar::rent::id(), false),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
        AccountMeta::new_readonly(vote_init.node_pubkey, true),
    ];

    Instruction::new_with_bincode(
        id(),
        &VoteInstruction::InitializeAccount(*vote_init),
        account_metas,
    )
}

pub fn create_account(
    from_pubkey: &Pubkey,
    vote_pubkey: &Pubkey,
    vote_init: &VoteInit,
    lamports: u64,
) -> Vec<Instruction> {
    let space = VoteState::size_of() as u64;
    let create_ix =
        system_instruction::create_account(from_pubkey, vote_pubkey, lamports, space, &id());
    let init_ix = initialize_account(vote_pubkey, vote_init);
    vec![create_ix, init_ix]
}

pub fn create_account_with_seed(
    from_pubkey: &Pubkey,
    vote_pubkey: &Pubkey,
    base: &Pubkey,
    seed: &str,
    vote_init: &VoteInit,
    lamports: u64,
) -> Vec<Instruction> {
    let space = VoteState::size_of() as u64;
    let create_ix = system_instruction::create_account_with_seed(
        from_pubkey,
        vote_pubkey,
        base,
        seed,
        lamports,
        space,
        &id(),
    );
    let init_ix = initialize_account(vote_pubkey, vote_init);
    vec![create_ix, init_ix]
}

pub fn authorize(
    vote_pubkey: &Pubkey,
    authorized_pubkey: &Pubkey, // currently authorized
    new_authorized_pubkey: &Pubkey,
    vote_authorize: VoteAuthorize,
) -> Instruction {
    let account_metas = vec![
        AccountMeta::new(*vote_pubkey, false),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
        AccountMeta::new_readonly(*authorized_pubkey, true),
    ];

    Instruction::new_with_bincode(
        id(),
        &VoteInstruction::Authorize(*new_authorized_pubkey, vote_authorize),
        account_metas,
    )
}

pub fn authorize_checked(
    vote_pubkey: &Pubkey,
    authorized_pubkey: &Pubkey, // currently authorized
    new_authorized_pubkey: &Pubkey,
    vote_authorize: VoteAuthorize,
) -> Instruction {
    let account_metas = vec![
        AccountMeta::new(*vote_pubkey, false),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
        AccountMeta::new_readonly(*authorized_pubkey, true),
        AccountMeta::new_readonly(*new_authorized_pubkey, true),
    ];

    Instruction::new_with_bincode(
        id(),
        &VoteInstruction::AuthorizeChecked(vote_authorize),
        account_metas,
    )
}

pub fn update_validator_identity(
    vote_pubkey: &Pubkey,
    authorized_withdrawer_pubkey: &Pubkey,
    node_pubkey: &Pubkey,
) -> Instruction {
    let account_metas = vec![
        AccountMeta::new(*vote_pubkey, false),
        AccountMeta::new_readonly(*node_pubkey, true),
        AccountMeta::new_readonly(*authorized_withdrawer_pubkey, true),
    ];

    Instruction::new_with_bincode(
        id(),
        &VoteInstruction::UpdateValidatorIdentity,
        account_metas,
    )
}

pub fn update_commission(
    vote_pubkey: &Pubkey,
    authorized_withdrawer_pubkey: &Pubkey,
    commission: u8,
) -> Instruction {
    let account_metas = vec![
        AccountMeta::new(*vote_pubkey, false),
        AccountMeta::new_readonly(*authorized_withdrawer_pubkey, true),
    ];

    Instruction::new_with_bincode(
        id(),
        &VoteInstruction::UpdateCommission(commission),
        account_metas,
    )
}

pub fn vote(vote_pubkey: &Pubkey, authorized_voter_pubkey: &Pubkey, vote: Vote) -> Instruction {
    let account_metas = vec![
        AccountMeta::new(*vote_pubkey, false),
        AccountMeta::new_readonly(sysvar::slot_hashes::id(), false),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
        AccountMeta::new_readonly(*authorized_voter_pubkey, true),
    ];

    Instruction::new_with_bincode(id(), &VoteInstruction::Vote(vote), account_metas)
}

pub fn vote_switch(
    vote_pubkey: &Pubkey,
    authorized_voter_pubkey: &Pubkey,
    vote: Vote,
    proof_hash: Hash,
) -> Instruction {
    let account_metas = vec![
        AccountMeta::new(*vote_pubkey, false),
        AccountMeta::new_readonly(sysvar::slot_hashes::id(), false),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
        AccountMeta::new_readonly(*authorized_voter_pubkey, true),
    ];

    Instruction::new_with_bincode(
        id(),
        &VoteInstruction::VoteSwitch(vote, proof_hash),
        account_metas,
    )
}

pub fn update_vote_state(
    vote_pubkey: &Pubkey,
    authorized_voter_pubkey: &Pubkey,
    vote_state_update: VoteStateUpdate,
) -> Instruction {
    let account_metas = vec![
        AccountMeta::new(*vote_pubkey, false),
        AccountMeta::new_readonly(*authorized_voter_pubkey, true),
    ];

    Instruction::new_with_bincode(
        id(),
        &VoteInstruction::UpdateVoteState(vote_state_update),
        account_metas,
    )
}

pub fn update_vote_state_switch(
    vote_pubkey: &Pubkey,
    authorized_voter_pubkey: &Pubkey,
    vote_state_update: VoteStateUpdate,
    proof_hash: Hash,
) -> Instruction {
    let account_metas = vec![
        AccountMeta::new(*vote_pubkey, false),
        AccountMeta::new_readonly(*authorized_voter_pubkey, true),
    ];

    Instruction::new_with_bincode(
        id(),
        &VoteInstruction::UpdateVoteStateSwitch(vote_state_update, proof_hash),
        account_metas,
    )
}

pub fn withdraw(
    vote_pubkey: &Pubkey,
    authorized_withdrawer_pubkey: &Pubkey,
    lamports: u64,
    to_pubkey: &Pubkey,
) -> Instruction {
    let account_metas = vec![
        AccountMeta::new(*vote_pubkey, false),
        AccountMeta::new(*to_pubkey, false),
        AccountMeta::new_readonly(*authorized_withdrawer_pubkey, true),
    ];

    Instruction::new_with_bincode(id(), &VoteInstruction::Withdraw(lamports), account_metas)
}

fn verify_rent_exemption(
    keyed_account: &KeyedAccount,
    rent: &Rent,
) -> Result<(), InstructionError> {
    if !rent.is_exempt(keyed_account.lamports()?, keyed_account.data_len()?) {
        Err(InstructionError::InsufficientFunds)
    } else {
        Ok(())
    }
}

/// These methods facilitate a transition from fetching sysvars from keyed
/// accounts to fetching from the sysvar cache without breaking consensus. In
/// order to keep consistent behavior, they continue to enforce the same checks
/// as `solana_sdk::keyed_account::from_keyed_account` despite dynamically
/// loading them instead of deserializing from account data.
mod get_sysvar_with_keyed_account_check {
    use super::*;

    pub fn clock(
        keyed_account: &KeyedAccount,
        invoke_context: &InvokeContext,
    ) -> Result<Arc<Clock>, InstructionError> {
        if invoke_context.feature_set.is_active(&load_sysvar_key()) {
            from_keyed_account(keyed_account).map(Arc::new)
        } else {
            check_sysvar_keyed_account::<Clock>(keyed_account)?;
            invoke_context.get_sysvar_cache().get_clock()
        }
    }

    pub fn rent(
        keyed_account: &KeyedAccount,
        invoke_context: &InvokeContext,
    ) -> Result<Arc<Rent>, InstructionError> {
        if invoke_context.feature_set.is_active(&load_sysvar_key()) {
            from_keyed_account(keyed_account).map(Arc::new)
        } else {
            check_sysvar_keyed_account::<Rent>(keyed_account)?;
            invoke_context.get_sysvar_cache().get_rent()
        }
    }

    pub fn slot_hashes(
        keyed_account: &KeyedAccount,
        invoke_context: &InvokeContext,
    ) -> Result<Arc<SlotHashes>, InstructionError> {
        if invoke_context.feature_set.is_active(&load_sysvar_key()) {
            from_keyed_account(keyed_account).map(Arc::new)
        } else {
            check_sysvar_keyed_account::<SlotHashes>(keyed_account)?;
            invoke_context.get_sysvar_cache().get_slot_hashes()
        }
    }
}

pub fn load_sysvar_key() -> Pubkey {
    Pubkey::from_str("DB6ZM16kkSCJRQcSYY22aJTNKzucqGPFB7oVhpcwwtCH").unwrap()
}

pub fn breakpoint_key_1() -> Pubkey {
    Pubkey::from_str("3R6L87Uij8QKYnskRLmoophYrF3qNsn5Z8kj4he6VjZ1").unwrap()
}

pub fn breakpoint_key_2() -> Pubkey {
    Pubkey::from_str("2P6gAdwwpo2BGQKCZqEr8UfccH9ZvJ7FQV3SYpX28q1k").unwrap()
}

pub fn breakpoint_key_3() -> Pubkey {
    Pubkey::from_str("7Wk3R244i3qtrGoZstQdFfi341S7dX8RdVuPzncW8A3M").unwrap()
}

pub fn breakpoint_key_4() -> Pubkey {
    Pubkey::from_str("2uAhCi4C9fRaixdR9bSFjQ6ou97sJpcqbETD9kifGk3h").unwrap()
}

pub fn breakpoint_key_5() -> Pubkey {
    Pubkey::from_str("HmJuPV3rf4yt4E5zGHP69iwonCU76peU8xxdpQNeMAjF").unwrap()
}

pub fn process_instruction(
    first_instruction_account: usize,
    data: &[u8],
    invoke_context: &mut InvokeContext,
) -> Result<(), InstructionError> {
    let (breakpoint_1, breakpoint_2, breakpoint_3, breakpoint_4, breakpoint_5) = (
        invoke_context.feature_set.is_active(&breakpoint_key_1()),
        invoke_context.feature_set.is_active(&breakpoint_key_2()),
        invoke_context.feature_set.is_active(&breakpoint_key_3()),
        invoke_context.feature_set.is_active(&breakpoint_key_4()),
        invoke_context.feature_set.is_active(&breakpoint_key_5()),
    );

    if breakpoint_1 {
        return Ok(());
    }

    let keyed_accounts = invoke_context.get_keyed_accounts()?;

    trace!("process_instruction: {:?}", data);
    trace!("keyed_accounts: {:?}", keyed_accounts);

    let me = &mut keyed_account_at_index(keyed_accounts, first_instruction_account)?;
    if me.owner()? != id() {
        return Err(InstructionError::InvalidAccountOwner);
    }

    let signers: HashSet<Pubkey> = get_signers(&keyed_accounts[first_instruction_account..]);
    match limited_deserialize(data)? {
        VoteInstruction::InitializeAccount(vote_init) => {
            let rent = get_sysvar_with_keyed_account_check::rent(
                keyed_account_at_index(keyed_accounts, first_instruction_account + 1)?,
                invoke_context,
            )?;
            verify_rent_exemption(me, &rent)?;
            let clock = get_sysvar_with_keyed_account_check::clock(
                keyed_account_at_index(keyed_accounts, first_instruction_account + 2)?,
                invoke_context,
            )?;
            vote_state::initialize_account(me, &vote_init, &signers, &clock)
        }
        VoteInstruction::Authorize(voter_pubkey, vote_authorize) => {
            let clock = get_sysvar_with_keyed_account_check::clock(
                keyed_account_at_index(keyed_accounts, first_instruction_account + 1)?,
                invoke_context,
            )?;
            vote_state::authorize(me, &voter_pubkey, vote_authorize, &signers, &clock)
        }
        VoteInstruction::UpdateValidatorIdentity => vote_state::update_validator_identity(
            me,
            keyed_account_at_index(keyed_accounts, first_instruction_account + 1)?.unsigned_key(),
            &signers,
        ),
        VoteInstruction::UpdateCommission(commission) => {
            vote_state::update_commission(me, commission, &signers)
        }
        VoteInstruction::Vote(vote) | VoteInstruction::VoteSwitch(vote, _) => {
            if !vote.slots.is_empty() && breakpoint_2 {
                return Ok(());
            }
            inc_new_counter_info!("vote-native", 1);
            let slot_hashes = get_sysvar_with_keyed_account_check::slot_hashes(
                keyed_account_at_index(keyed_accounts, first_instruction_account + 1)?,
                invoke_context,
            )?;
            let clock = get_sysvar_with_keyed_account_check::clock(
                keyed_account_at_index(keyed_accounts, first_instruction_account + 2)?,
                invoke_context,
            )?;
            if breakpoint_3 {
                return Ok(());
            }
            vote_state::process_vote(
                me,
                &slot_hashes,
                &clock,
                &vote,
                &signers,
                breakpoint_4,
                breakpoint_5,
            )
        }
        VoteInstruction::UpdateVoteState(vote_state_update)
        | VoteInstruction::UpdateVoteStateSwitch(vote_state_update, _) => {
            if invoke_context
                .feature_set
                .is_active(&feature_set::allow_votes_to_directly_update_vote_state::id())
            {
                inc_new_counter_info!("vote-state-native", 1);
                let sysvar_cache = invoke_context.get_sysvar_cache();
                let slot_hashes = sysvar_cache.get_slot_hashes()?;
                let clock = sysvar_cache.get_clock()?;
                vote_state::process_vote_state_update(
                    me,
                    slot_hashes.slot_hashes(),
                    &clock,
                    vote_state_update,
                    &signers,
                )
            } else {
                Err(InstructionError::InvalidInstructionData)
            }
        }
        VoteInstruction::Withdraw(lamports) => {
            let to = keyed_account_at_index(keyed_accounts, first_instruction_account + 1)?;
            let rent_sysvar = if invoke_context
                .feature_set
                .is_active(&feature_set::reject_non_rent_exempt_vote_withdraws::id())
            {
                Some(invoke_context.get_sysvar_cache().get_rent()?)
            } else {
                None
            };
            vote_state::withdraw(me, lamports, to, &signers, rent_sysvar.as_deref())
        }
        VoteInstruction::AuthorizeChecked(vote_authorize) => {
            if invoke_context
                .feature_set
                .is_active(&feature_set::vote_stake_checked_instructions::id())
            {
                let voter_pubkey =
                    &keyed_account_at_index(keyed_accounts, first_instruction_account + 3)?
                        .signer_key()
                        .ok_or(InstructionError::MissingRequiredSignature)?;
                vote_state::authorize(
                    me,
                    voter_pubkey,
                    vote_authorize,
                    &signers,
                    &from_keyed_account::<Clock>(keyed_account_at_index(
                        keyed_accounts,
                        first_instruction_account + 1,
                    )?)?,
                )
            } else {
                Err(InstructionError::InvalidInstructionData)
            }
        }
    }
}
