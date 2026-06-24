//! Shared domain models for parsed snapRAID and snapglass state.

use chrono::{DateTime, NaiveDate, Utc};

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
    pub last_scrub: Option<NaiveDate>,
    pub scrub_age_days: Option<i64>,
    pub scrub_stale: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskStatus {
    Ok,
    Warning,
    Error,
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
