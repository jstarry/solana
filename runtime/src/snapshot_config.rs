use {
    crate::{
        snapshot_bank_utils,
        snapshot_utils::{self, ArchiveFormat, SnapshotVersion, ZstdConfig},
    },
    solana_sdk::clock::Slot,
    std::{num::NonZeroUsize, path::PathBuf},
};

/// Snapshot configuration and runtime information
#[derive(Clone, Debug)]
pub struct SnapshotConfig {
    /// Specifies the ways thats snapshots are allowed to be used
    pub usage: SnapshotUsage,

    /// Generate a new full snapshot archive every this many slots
    pub full_snapshot_archive_interval_slots: Slot,

    /// Generate a new incremental snapshot archive every this many slots
    pub incremental_snapshot_archive_interval_slots: Slot,

    /// Path to the directory where full snapshot archives are stored
    pub full_snapshot_archives_dir: PathBuf,

    /// Path to the directory where incremental snapshot archives are stored
    pub incremental_snapshot_archives_dir: PathBuf,

    /// Path to the directory where bank snapshots are stored
    pub bank_snapshots_dir: PathBuf,

    /// The archive format to use for snapshots
    pub archive_format: ArchiveFormat,

    /// Snapshot version to generate
    pub snapshot_version: SnapshotVersion,

    /// Maximum number of full snapshot archives to retain
    pub maximum_full_snapshot_archives_to_retain: NonZeroUsize,

    /// Maximum number of incremental snapshot archives to retain
    /// NOTE: Incremental snapshots will only be kept for the latest full snapshot
    pub maximum_incremental_snapshot_archives_to_retain: NonZeroUsize,

    /// This is the `debug_verify` parameter to use when calling `update_accounts_hash()`
    pub accounts_hash_debug_verify: bool,

    // Thread niceness adjustment for snapshot packager service
    pub packager_thread_niceness_adj: i8,
}

impl Default for SnapshotConfig {
    fn default() -> Self {
        Self {
            usage: SnapshotUsage::LoadAndGenerate,
            full_snapshot_archive_interval_slots:
                snapshot_bank_utils::DEFAULT_FULL_SNAPSHOT_ARCHIVE_INTERVAL_SLOTS,
            incremental_snapshot_archive_interval_slots:
                snapshot_bank_utils::DEFAULT_INCREMENTAL_SNAPSHOT_ARCHIVE_INTERVAL_SLOTS,
            full_snapshot_archives_dir: PathBuf::default(),
            incremental_snapshot_archives_dir: PathBuf::default(),
            bank_snapshots_dir: PathBuf::default(),
            archive_format: ArchiveFormat::TarZstd {
                config: ZstdConfig::default(),
            },
            snapshot_version: SnapshotVersion::default(),
            maximum_full_snapshot_archives_to_retain:
                snapshot_utils::DEFAULT_MAX_FULL_SNAPSHOT_ARCHIVES_TO_RETAIN,
            maximum_incremental_snapshot_archives_to_retain:
                snapshot_utils::DEFAULT_MAX_INCREMENTAL_SNAPSHOT_ARCHIVES_TO_RETAIN,
            accounts_hash_debug_verify: false,
            packager_thread_niceness_adj: 0,
        }
    }
}

impl SnapshotConfig {
    /// A new snapshot config used for only loading at startup
    pub fn new_load_only() -> Self {
        Self {
            usage: SnapshotUsage::LoadOnly,
            ..Self::default()
        }
    }

    /// A new snapshot config used to disable snapshot generation and loading at
    /// startup
    pub fn new_disabled() -> Self {
        Self {
            usage: SnapshotUsage::Disabled,
            ..Self::default()
        }
    }

    /// Should snapshots be generated?
    pub fn should_generate_snapshots(&self) -> bool {
        self.usage == SnapshotUsage::LoadAndGenerate
    }

    /// Should snapshots be loaded?
    pub fn should_load_snapshots(&self) -> bool {
        self.usage == SnapshotUsage::LoadAndGenerate || self.usage == SnapshotUsage::LoadOnly
    }
}

/// Specify the ways that snapshots are allowed to be used
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SnapshotUsage {
    /// Snapshots are never generated or loaded at startup,
    /// instead start from genesis.
    Disabled,
    /// Snapshots are only used at startup, to load the accounts and bank
    LoadOnly,
    /// Snapshots are used everywhere; both at startup (i.e. load) and steady-state (i.e.
    /// generate).  This enables taking snapshots.
    LoadAndGenerate,
}

pub struct SnapshotArchiveConfig {
    /// Path to the directory where full snapshot archives are stored
    pub full_snapshot_archives_dir: PathBuf,

    /// Path to the directory where incremental snapshot archives are stored
    pub incremental_snapshot_archives_dir: PathBuf,

    /// Path to the directory where bank snapshots are stored
    pub bank_snapshots_dir: PathBuf,

    /// The archive format to use for snapshots
    pub archive_format: ArchiveFormat,

    /// Snapshot version to generate
    pub snapshot_version: SnapshotVersion,
}

impl From<&SnapshotConfig> for SnapshotArchiveConfig {
    fn from(snapshot_config: &SnapshotConfig) -> Self {
        Self {
            full_snapshot_archives_dir: snapshot_config.full_snapshot_archives_dir.clone(),
            incremental_snapshot_archives_dir: snapshot_config
                .incremental_snapshot_archives_dir
                .clone(),
            bank_snapshots_dir: snapshot_config.bank_snapshots_dir.clone(),
            archive_format: snapshot_config.archive_format,
            snapshot_version: snapshot_config.snapshot_version,
        }
    }
}
