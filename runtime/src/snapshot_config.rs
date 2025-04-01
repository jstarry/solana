use {
    crate::{
        snapshot_bank_utils::{self, DISABLED_SNAPSHOT_ARCHIVE_INTERVAL},
        snapshot_utils::{self, ArchiveFormat, SnapshotVersion, ZstdConfig},
    },
    solana_sdk::clock::Slot,
    std::{num::NonZeroUsize, path::PathBuf, sync::RwLock},
};

#[derive(Clone, Copy, Debug)]
pub struct SnapshotGenerationIntervals {
    /// Generate a new full snapshot archive every this many slots
    pub full_snapshot_interval: Slot,

    /// Generate a new incremental snapshot archive every this many slots
    pub incremental_snapshot_interval: Slot,
}

impl SnapshotGenerationIntervals {
    pub fn enabled(&self) -> bool {
        self.full_snapshot_interval != DISABLED_SNAPSHOT_ARCHIVE_INTERVAL
    }
}

impl Default for SnapshotGenerationIntervals {
    fn default() -> Self {
        Self {
            full_snapshot_interval:
                snapshot_bank_utils::DEFAULT_FULL_SNAPSHOT_ARCHIVE_INTERVAL_SLOTS,
            incremental_snapshot_interval:
                snapshot_bank_utils::DEFAULT_INCREMENTAL_SNAPSHOT_ARCHIVE_INTERVAL_SLOTS,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SnapshotArchiveStoragePaths {
    /// Path to the directory where full snapshot archives are stored
    pub full_snapshot_archives_dir: PathBuf,

    /// Path to the directory where incremental snapshot archives are stored
    pub incremental_snapshot_archives_dir: PathBuf,
}

/// Snapshot configuration and runtime information
#[derive(Clone, Debug)]
pub struct SnapshotConfig {
    pub generation_intervals: RwLock<Option<SnapshotGenerationIntervals>>,

    pub load_at_startup: bool,

    /// Paths to the directories where snapshot archives are stored
    pub archive_storage_paths: SnapshotArchiveStoragePaths,

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
            load_at_startup: true,
            generation_intervals: RwLock::new(Some(SnapshotGenerationIntervals::default())),
            archive_storage_paths: SnapshotArchiveStoragePaths {
                full_snapshot_archives_dir: PathBuf::default(),
                incremental_snapshot_archives_dir: PathBuf::default(),
            },
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
            generation_intervals: RwLock::new(None),
            ..Self::default()
        }
    }

    /// A new snapshot config used to disable snapshot generation and loading at
    /// startup
    pub fn new_disabled() -> Self {
        Self {
            generation_intervals: RwLock::new(None),
            load_at_startup: false,
            ..Self::default()
        }
    }

    /// Should snapshots be generated?
    pub fn should_generate_snapshots(&self) -> bool {
        self.generation_intervals.read().unwrap().is_some()
    }

    /// Should snapshots be loaded?
    pub fn should_load_snapshots(&self) -> bool {
        self.load_at_startup
    }
}
