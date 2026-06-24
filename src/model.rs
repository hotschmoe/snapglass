//! Shared domain models for parsed snapRAID and snapglass state.

use chrono::{DateTime, Utc};

use crate::config::SnapraidConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    InSync = 0,
    DriftPresent = 1,
    DriftExceedsThreshold = 2,
    RuntimeFailure = 10,
}

impl ExitCode {
    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrayState {
    pub config: SnapraidConfig,
    pub last_sync: Option<DateTime<Utc>>,
    pub scrub: ScrubState,
    pub sync_in_progress: Option<bool>,
    pub fully_synced: Option<bool>,
    pub health: ArrayHealth,
    pub messages: Vec<String>,
    pub error_count: Option<u64>,
    pub disks: Vec<DiskState>,
    pub diff: Option<DiffSummary>,
}

impl ArrayState {
    pub fn attach_diff(mut self, diff: DiffSummary) -> Self {
        self.diff = Some(diff);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskState {
    pub name: String,
    pub path: String,
    pub status: DiskStatus,
    pub files: Option<u64>,
    pub fragmented_files: Option<u64>,
    pub excess_fragments: Option<u64>,
    pub wasted_gb: Option<String>,
    pub used_gb: Option<String>,
    pub free_gb: Option<String>,
    pub use_percent: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskStatus {
    Ok,
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScrubState {
    pub unscrubbed_percent: Option<u8>,
    pub oldest_days: Option<i64>,
    pub median_days: Option<i64>,
    pub newest_days: Option<i64>,
    pub never_scrubbed: bool,
    pub stale: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrayHealth {
    Ok,
    Warning,
    Danger,
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffSummary {
    pub equal: u64,
    pub added: u64,
    pub removed: u64,
    pub updated: u64,
    pub moved: u64,
    pub copied: u64,
    pub restored: u64,
}

impl DiffSummary {
    pub fn changed(&self) -> u64 {
        self.added + self.removed + self.updated + self.moved + self.copied + self.restored
    }

    pub fn risky_changed(&self) -> u64 {
        self.removed + self.updated
    }

    pub fn has_drift(&self) -> bool {
        self.changed() > 0
    }

    pub fn exit_code(&self, threshold: u64) -> ExitCode {
        if self.risky_changed() > threshold {
            ExitCode::DriftExceedsThreshold
        } else if self.has_drift() {
            ExitCode::DriftPresent
        } else {
            ExitCode::InSync
        }
    }
}
