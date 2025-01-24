//! Vote state

#[cfg(test)]
use arbitrary::{Arbitrary, Unstructured};
#[cfg(feature = "serde")]
use serde_derive::{Deserialize, Serialize};
#[cfg(feature = "frozen-abi")]
use solana_frozen_abi_macro::{frozen_abi, AbiExample};
use {
    crate::authorized_voters::AuthorizedVoters,
    solana_clock::{Epoch, Slot, UnixTimestamp},
    solana_hash::Hash,
    solana_pubkey::Pubkey,
    solana_rent::Rent,
    std::{collections::VecDeque, fmt::Debug},
};

mod vote_state_0_23_5;
pub mod vote_state_1_14_11;
pub use vote_state_1_14_11::*;
mod vote_state_v3;
pub use {vote_state_v3::*, VoteStateV3 as VoteState};
pub mod vote_state_versions;
pub use vote_state_versions::*;

// Maximum number of votes to keep around, tightly coupled with epoch_schedule::MINIMUM_SLOTS_PER_EPOCH
pub const MAX_LOCKOUT_HISTORY: usize = 31;
pub const INITIAL_LOCKOUT: usize = 2;

// Maximum number of credits history to keep around
pub const MAX_EPOCH_CREDITS_HISTORY: usize = 64;

// Number of slots of grace period for which maximum vote credits are awarded - votes landing within this number of slots of the slot that is being voted on are awarded full credits.
pub const VOTE_CREDITS_GRACE_SLOTS: u8 = 2;

// Maximum number of credits to award for a vote; this number of credits is awarded to votes on slots that land within the grace period. After that grace period, vote credits are reduced.
pub const VOTE_CREDITS_MAXIMUM_PER_SLOT: u8 = 16;

#[cfg_attr(
    feature = "frozen-abi",
    frozen_abi(digest = "GvUzgtcxhKVVxPAjSntXGPqjLZK5ovgZzCiUP1tDpB9q"),
    derive(AbiExample)
)]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Default, Debug, PartialEq, Eq, Clone)]
pub struct Vote {
    /// A stack of votes starting with the oldest vote
    pub slots: Vec<Slot>,
    /// signature of the bank's state at the last slot
    pub hash: Hash,
    /// processing timestamp of last slot
    pub timestamp: Option<UnixTimestamp>,
}

impl Vote {
    pub fn new(slots: Vec<Slot>, hash: Hash) -> Self {
        Self {
            slots,
            hash,
            timestamp: None,
        }
    }

    pub fn last_voted_slot(&self) -> Option<Slot> {
        self.slots.last().copied()
    }
}

#[cfg_attr(feature = "frozen-abi", derive(AbiExample))]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Default, Debug, PartialEq, Eq, Copy, Clone)]
#[cfg_attr(test, derive(Arbitrary))]
pub struct Lockout {
    slot: Slot,
    confirmation_count: u32,
}

impl Lockout {
    pub fn new(slot: Slot) -> Self {
        Self::new_with_confirmation_count(slot, 1)
    }

    pub fn new_with_confirmation_count(slot: Slot, confirmation_count: u32) -> Self {
        Self {
            slot,
            confirmation_count,
        }
    }

    // The number of slots for which this vote is locked
    pub fn lockout(&self) -> u64 {
        (INITIAL_LOCKOUT as u64).pow(self.confirmation_count())
    }

    // The last slot at which a vote is still locked out. Validators should not
    // vote on a slot in another fork which is less than or equal to this slot
    // to avoid having their stake slashed.
    pub fn last_locked_out_slot(&self) -> Slot {
        self.slot.saturating_add(self.lockout())
    }

    pub fn is_locked_out_at_slot(&self, slot: Slot) -> bool {
        self.last_locked_out_slot() >= slot
    }

    pub fn slot(&self) -> Slot {
        self.slot
    }

    pub fn confirmation_count(&self) -> u32 {
        self.confirmation_count
    }

    pub fn increase_confirmation_count(&mut self, by: u32) {
        self.confirmation_count = self.confirmation_count.saturating_add(by)
    }
}

#[cfg_attr(feature = "frozen-abi", derive(AbiExample))]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Default, Debug, PartialEq, Eq, Copy, Clone)]
#[cfg_attr(test, derive(Arbitrary))]
pub struct LandedVote {
    // Latency is the difference in slot number between the slot that was voted on (lockout.slot) and the slot in
    // which the vote that added this Lockout landed.  For votes which were cast before versions of the validator
    // software which recorded vote latencies, latency is recorded as 0.
    pub latency: u8,
    pub lockout: Lockout,
}

impl LandedVote {
    pub fn slot(&self) -> Slot {
        self.lockout.slot
    }

    pub fn confirmation_count(&self) -> u32 {
        self.lockout.confirmation_count
    }
}

impl From<LandedVote> for Lockout {
    fn from(landed_vote: LandedVote) -> Self {
        landed_vote.lockout
    }
}

impl From<Lockout> for LandedVote {
    fn from(lockout: Lockout) -> Self {
        Self {
            latency: 0,
            lockout,
        }
    }
}

#[cfg_attr(
    feature = "frozen-abi",
    frozen_abi(digest = "CxyuwbaEdzP7jDCZyxjgQvLGXadBUZF3LoUvbSpQ6tYN"),
    derive(AbiExample)
)]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Default, Debug, PartialEq, Eq, Clone)]
pub struct VoteStateUpdate {
    /// The proposed tower
    pub lockouts: VecDeque<Lockout>,
    /// The proposed root
    pub root: Option<Slot>,
    /// signature of the bank's state at the last slot
    pub hash: Hash,
    /// processing timestamp of last slot
    pub timestamp: Option<UnixTimestamp>,
}

impl From<Vec<(Slot, u32)>> for VoteStateUpdate {
    fn from(recent_slots: Vec<(Slot, u32)>) -> Self {
        let lockouts: VecDeque<Lockout> = recent_slots
            .into_iter()
            .map(|(slot, confirmation_count)| {
                Lockout::new_with_confirmation_count(slot, confirmation_count)
            })
            .collect();
        Self {
            lockouts,
            root: None,
            hash: Hash::default(),
            timestamp: None,
        }
    }
}

impl VoteStateUpdate {
    pub fn new(lockouts: VecDeque<Lockout>, root: Option<Slot>, hash: Hash) -> Self {
        Self {
            lockouts,
            root,
            hash,
            timestamp: None,
        }
    }

    pub fn slots(&self) -> Vec<Slot> {
        self.lockouts.iter().map(|lockout| lockout.slot()).collect()
    }

    pub fn last_voted_slot(&self) -> Option<Slot> {
        self.lockouts.back().map(|l| l.slot())
    }
}

#[cfg_attr(
    feature = "frozen-abi",
    frozen_abi(digest = "6UDiQMH4wbNwkMHosPMtekMYu2Qa6CHPZ2ymK4mc6FGu"),
    derive(AbiExample)
)]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Default, Debug, PartialEq, Eq, Clone)]
pub struct TowerSync {
    /// The proposed tower
    pub lockouts: VecDeque<Lockout>,
    /// The proposed root
    pub root: Option<Slot>,
    /// signature of the bank's state at the last slot
    pub hash: Hash,
    /// processing timestamp of last slot
    pub timestamp: Option<UnixTimestamp>,
    /// the unique identifier for the chain up to and
    /// including this block. Does not require replaying
    /// in order to compute.
    pub block_id: Hash,
}

impl From<Vec<(Slot, u32)>> for TowerSync {
    fn from(recent_slots: Vec<(Slot, u32)>) -> Self {
        let lockouts: VecDeque<Lockout> = recent_slots
            .into_iter()
            .map(|(slot, confirmation_count)| {
                Lockout::new_with_confirmation_count(slot, confirmation_count)
            })
            .collect();
        Self {
            lockouts,
            root: None,
            hash: Hash::default(),
            timestamp: None,
            block_id: Hash::default(),
        }
    }
}

impl TowerSync {
    pub fn new(
        lockouts: VecDeque<Lockout>,
        root: Option<Slot>,
        hash: Hash,
        block_id: Hash,
    ) -> Self {
        Self {
            lockouts,
            root,
            hash,
            timestamp: None,
            block_id,
        }
    }

    /// Creates a tower with consecutive votes for `slot - MAX_LOCKOUT_HISTORY + 1` to `slot` inclusive.
    /// If `slot >= MAX_LOCKOUT_HISTORY`, sets the root to `(slot - MAX_LOCKOUT_HISTORY)`
    /// Sets the hash to `hash` and leaves `block_id` unset.
    pub fn new_from_slot(slot: Slot, hash: Hash) -> Self {
        let lowest_slot = slot
            .saturating_add(1)
            .saturating_sub(MAX_LOCKOUT_HISTORY as u64);
        let slots: Vec<_> = (lowest_slot..slot.saturating_add(1)).collect();
        Self::new_from_slots(
            slots,
            hash,
            (lowest_slot > 0).then(|| lowest_slot.saturating_sub(1)),
        )
    }

    /// Creates a tower with consecutive confirmation for `slots`
    pub fn new_from_slots(slots: Vec<Slot>, hash: Hash, root: Option<Slot>) -> Self {
        let lockouts: VecDeque<Lockout> = slots
            .into_iter()
            .rev()
            .enumerate()
            .map(|(cc, s)| Lockout::new_with_confirmation_count(s, cc.saturating_add(1) as u32))
            .rev()
            .collect();
        Self {
            lockouts,
            hash,
            root,
            timestamp: None,
            block_id: Hash::default(),
        }
    }

    pub fn slots(&self) -> Vec<Slot> {
        self.lockouts.iter().map(|lockout| lockout.slot()).collect()
    }

    pub fn last_voted_slot(&self) -> Option<Slot> {
        self.lockouts.back().map(|l| l.slot())
    }
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Default, Debug, PartialEq, Eq, Clone, Copy)]
pub struct VoteInit {
    pub node_pubkey: Pubkey,
    pub authorized_voter: Pubkey,
    pub authorized_withdrawer: Pubkey,
    pub commission: u8,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum VoteAuthorize {
    Voter,
    Withdrawer,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct VoteAuthorizeWithSeedArgs {
    pub authorization_type: VoteAuthorize,
    pub current_authority_derived_key_owner: Pubkey,
    pub current_authority_derived_key_seed: String,
    pub new_authority: Pubkey,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct VoteAuthorizeCheckedWithSeedArgs {
    pub authorization_type: VoteAuthorize,
    pub current_authority_derived_key_owner: Pubkey,
    pub current_authority_derived_key_seed: String,
}

#[cfg_attr(feature = "frozen-abi", derive(AbiExample))]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, Default, PartialEq, Eq, Clone)]
#[cfg_attr(test, derive(Arbitrary))]
pub struct BlockTimestamp {
    pub slot: Slot,
    pub timestamp: UnixTimestamp,
}

// this is how many epochs a voter can be remembered for slashing
const MAX_ITEMS: usize = 32;

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[cfg_attr(feature = "frozen-abi", derive(AbiExample))]
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(test, derive(Arbitrary))]
pub struct CircBuf<I> {
    buf: [I; MAX_ITEMS],
    /// next pointer
    idx: usize,
    is_empty: bool,
}

impl<I: Default + Copy> Default for CircBuf<I> {
    fn default() -> Self {
        Self {
            buf: [I::default(); MAX_ITEMS],
            idx: MAX_ITEMS
                .checked_sub(1)
                .expect("`MAX_ITEMS` should be positive"),
            is_empty: true,
        }
    }
}

impl<I> CircBuf<I> {
    pub fn append(&mut self, item: I) {
        // remember prior delegate and when we switched, to support later slashing
        self.idx = self
            .idx
            .checked_add(1)
            .and_then(|idx| idx.checked_rem(MAX_ITEMS))
            .expect("`self.idx` should be < `MAX_ITEMS` which should be non-zero");

        self.buf[self.idx] = item;
        self.is_empty = false;
    }

    pub fn buf(&self) -> &[I; MAX_ITEMS] {
        &self.buf
    }

    pub fn last(&self) -> Option<&I> {
        if !self.is_empty {
            self.buf.get(self.idx)
        } else {
            None
        }
    }
}

#[cfg(feature = "serde")]
pub mod serde_compact_vote_state_update {
    use {
        super::*,
        crate::state::Lockout,
        serde::{Deserialize, Deserializer, Serialize, Serializer},
        solana_serde_varint as serde_varint, solana_short_vec as short_vec,
    };

    #[cfg_attr(feature = "frozen-abi", derive(AbiExample))]
    #[derive(serde_derive::Deserialize, serde_derive::Serialize)]
    struct LockoutOffset {
        #[serde(with = "serde_varint")]
        offset: Slot,
        confirmation_count: u8,
    }

    #[derive(serde_derive::Deserialize, serde_derive::Serialize)]
    struct CompactVoteStateUpdate {
        root: Slot,
        #[serde(with = "short_vec")]
        lockout_offsets: Vec<LockoutOffset>,
        hash: Hash,
        timestamp: Option<UnixTimestamp>,
    }

    pub fn serialize<S>(
        vote_state_update: &VoteStateUpdate,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let lockout_offsets = vote_state_update.lockouts.iter().scan(
            vote_state_update.root.unwrap_or_default(),
            |slot, lockout| {
                let Some(offset) = lockout.slot().checked_sub(*slot) else {
                    return Some(Err(serde::ser::Error::custom("Invalid vote lockout")));
                };
                let Ok(confirmation_count) = u8::try_from(lockout.confirmation_count()) else {
                    return Some(Err(serde::ser::Error::custom("Invalid confirmation count")));
                };
                let lockout_offset = LockoutOffset {
                    offset,
                    confirmation_count,
                };
                *slot = lockout.slot();
                Some(Ok(lockout_offset))
            },
        );
        let compact_vote_state_update = CompactVoteStateUpdate {
            root: vote_state_update.root.unwrap_or(Slot::MAX),
            lockout_offsets: lockout_offsets.collect::<Result<_, _>>()?,
            hash: vote_state_update.hash,
            timestamp: vote_state_update.timestamp,
        };
        compact_vote_state_update.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<VoteStateUpdate, D::Error>
    where
        D: Deserializer<'de>,
    {
        let CompactVoteStateUpdate {
            root,
            lockout_offsets,
            hash,
            timestamp,
        } = CompactVoteStateUpdate::deserialize(deserializer)?;
        let root = (root != Slot::MAX).then_some(root);
        let lockouts =
            lockout_offsets
                .iter()
                .scan(root.unwrap_or_default(), |slot, lockout_offset| {
                    *slot = match slot.checked_add(lockout_offset.offset) {
                        None => {
                            return Some(Err(serde::de::Error::custom("Invalid lockout offset")))
                        }
                        Some(slot) => slot,
                    };
                    let lockout = Lockout::new_with_confirmation_count(
                        *slot,
                        u32::from(lockout_offset.confirmation_count),
                    );
                    Some(Ok(lockout))
                });
        Ok(VoteStateUpdate {
            root,
            lockouts: lockouts.collect::<Result<_, _>>()?,
            hash,
            timestamp,
        })
    }
}

#[cfg(feature = "serde")]
pub mod serde_tower_sync {
    use {
        super::*,
        crate::state::Lockout,
        serde::{Deserialize, Deserializer, Serialize, Serializer},
        solana_serde_varint as serde_varint, solana_short_vec as short_vec,
    };

    #[cfg_attr(feature = "frozen-abi", derive(AbiExample))]
    #[derive(serde_derive::Deserialize, serde_derive::Serialize)]
    struct LockoutOffset {
        #[serde(with = "serde_varint")]
        offset: Slot,
        confirmation_count: u8,
    }

    #[derive(serde_derive::Deserialize, serde_derive::Serialize)]
    struct CompactTowerSync {
        root: Slot,
        #[serde(with = "short_vec")]
        lockout_offsets: Vec<LockoutOffset>,
        hash: Hash,
        timestamp: Option<UnixTimestamp>,
        block_id: Hash,
    }

    pub fn serialize<S>(tower_sync: &TowerSync, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let lockout_offsets = tower_sync.lockouts.iter().scan(
            tower_sync.root.unwrap_or_default(),
            |slot, lockout| {
                let Some(offset) = lockout.slot().checked_sub(*slot) else {
                    return Some(Err(serde::ser::Error::custom("Invalid vote lockout")));
                };
                let Ok(confirmation_count) = u8::try_from(lockout.confirmation_count()) else {
                    return Some(Err(serde::ser::Error::custom("Invalid confirmation count")));
                };
                let lockout_offset = LockoutOffset {
                    offset,
                    confirmation_count,
                };
                *slot = lockout.slot();
                Some(Ok(lockout_offset))
            },
        );
        let compact_tower_sync = CompactTowerSync {
            root: tower_sync.root.unwrap_or(Slot::MAX),
            lockout_offsets: lockout_offsets.collect::<Result<_, _>>()?,
            hash: tower_sync.hash,
            timestamp: tower_sync.timestamp,
            block_id: tower_sync.block_id,
        };
        compact_tower_sync.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<TowerSync, D::Error>
    where
        D: Deserializer<'de>,
    {
        let CompactTowerSync {
            root,
            lockout_offsets,
            hash,
            timestamp,
            block_id,
        } = CompactTowerSync::deserialize(deserializer)?;
        let root = (root != Slot::MAX).then_some(root);
        let lockouts =
            lockout_offsets
                .iter()
                .scan(root.unwrap_or_default(), |slot, lockout_offset| {
                    *slot = match slot.checked_add(lockout_offset.offset) {
                        None => {
                            return Some(Err(serde::de::Error::custom("Invalid lockout offset")))
                        }
                        Some(slot) => slot,
                    };
                    let lockout = Lockout::new_with_confirmation_count(
                        *slot,
                        u32::from(lockout_offset.confirmation_count),
                    );
                    Some(Ok(lockout))
                });
        Ok(TowerSync {
            root,
            lockouts: lockouts.collect::<Result<_, _>>()?,
            hash,
            timestamp,
            block_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use {super::*, itertools::Itertools, rand::Rng};

    #[test]
    fn test_serde_compact_vote_state_update() {
        let mut rng = rand::thread_rng();
        for _ in 0..5000 {
            run_serde_compact_vote_state_update(&mut rng);
        }
    }

    fn run_serde_compact_vote_state_update<R: Rng>(rng: &mut R) {
        let lockouts: VecDeque<_> = std::iter::repeat_with(|| {
            let slot = 149_303_885_u64.saturating_add(rng.gen_range(0..10_000));
            let confirmation_count = rng.gen_range(0..33);
            Lockout::new_with_confirmation_count(slot, confirmation_count)
        })
        .take(32)
        .sorted_by_key(|lockout| lockout.slot())
        .collect();
        let root = rng.gen_ratio(1, 2).then(|| {
            lockouts[0]
                .slot()
                .checked_sub(rng.gen_range(0..1_000))
                .expect("All slots should be greater than 1_000")
        });
        let timestamp = rng.gen_ratio(1, 2).then(|| rng.gen());
        let hash = Hash::from(rng.gen::<[u8; 32]>());
        let vote_state_update = VoteStateUpdate {
            lockouts,
            root,
            hash,
            timestamp,
        };
        #[derive(Debug, Eq, PartialEq, Deserialize, Serialize)]
        enum VoteInstruction {
            #[serde(with = "serde_compact_vote_state_update")]
            UpdateVoteState(VoteStateUpdate),
            UpdateVoteStateSwitch(
                #[serde(with = "serde_compact_vote_state_update")] VoteStateUpdate,
                Hash,
            ),
        }
        let vote = VoteInstruction::UpdateVoteState(vote_state_update.clone());
        let bytes = bincode::serialize(&vote).unwrap();
        assert_eq!(vote, bincode::deserialize(&bytes).unwrap());
        let hash = Hash::from(rng.gen::<[u8; 32]>());
        let vote = VoteInstruction::UpdateVoteStateSwitch(vote_state_update, hash);
        let bytes = bincode::serialize(&vote).unwrap();
        assert_eq!(vote, bincode::deserialize(&bytes).unwrap());
    }

    #[test]
    fn test_circbuf_oob() {
        // Craft an invalid CircBuf with out-of-bounds index
        let data: &[u8] = &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
        let circ_buf: CircBuf<()> = bincode::deserialize(data).unwrap();
        assert_eq!(circ_buf.last(), None);
    }
}
