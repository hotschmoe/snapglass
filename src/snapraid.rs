//! Process wrapper and parsers for human-readable `snapraid status` and `snapraid diff` output.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use chrono::{DateTime, NaiveDate, Utc};
use thiserror::Error;

use crate::config::SnapraidConfig;
use crate::model::{ArrayState, DiffSummary, DiskState, DiskStatus};

#[derive(Debug, Error)]
pub enum SnapraidError {
    #[error("could not run snapraid {command}: {source}")]
    Io {
        command: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("snapraid {command} exited with status {status}: {stderr}")]
    Failed {
        command: &'static str,
        status: i32,
        stderr: String,
    },
}

pub trait SnapraidRunner {
    fn status(&self, config_path: &Path) -> Result<String, SnapraidError>;
    fn diff(&self, config_path: &Path) -> Result<String, SnapraidError>;
    fn sync(&self, config_path: &Path) -> Result<String, SnapraidError>;
    fn scrub(&self, config_path: &Path) -> Result<String, SnapraidError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ProcessSnapraidRunner;

impl ProcessSnapraidRunner {
    fn run(command: &'static str, config_path: &Path) -> Result<String, SnapraidError> {
        let output = Command::new("snapraid")
            .arg("-c")
            .arg(config_path)
            .arg(command)
            .output()
            .map_err(|source| SnapraidError::Io { command, source })?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Err(SnapraidError::Failed {
                command,
                status: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            })
        }
    }
}

impl SnapraidRunner for ProcessSnapraidRunner {
    fn status(&self, config_path: &Path) -> Result<String, SnapraidError> {
        Self::run("status", config_path)
    }

    fn diff(&self, config_path: &Path) -> Result<String, SnapraidError> {
        Self::run("diff", config_path)
    }

    fn sync(&self, config_path: &Path) -> Result<String, SnapraidError> {
        Self::run("sync", config_path)
    }

    fn scrub(&self, config_path: &Path) -> Result<String, SnapraidError> {
        Self::run("scrub", config_path)
    }
}

#[cfg(test)]
#[derive(Debug, Clone)]
pub struct FixtureSnapraidRunner {
    pub status_output: String,
    pub diff_output: String,
}

#[cfg(test)]
impl SnapraidRunner for FixtureSnapraidRunner {
    fn status(&self, _config_path: &Path) -> Result<String, SnapraidError> {
        Ok(self.status_output.clone())
    }

    fn diff(&self, _config_path: &Path) -> Result<String, SnapraidError> {
        Ok(self.diff_output.clone())
    }

    fn sync(&self, _config_path: &Path) -> Result<String, SnapraidError> {
        Ok(String::new())
    }

    fn scrub(&self, _config_path: &Path) -> Result<String, SnapraidError> {
        Ok(String::new())
    }
}

pub fn parse_status_output(
    text: &str,
    config: &SnapraidConfig,
    scrub_window_days: i64,
) -> ArrayState {
    let today = Utc::now().date_naive();
    let mut disk_status = HashMap::new();
    let mut disk_scrub = HashMap::new();
    let mut last_sync = None;
    let mut current_disk = None;

    for raw in text.lines() {
        let line = raw.trim();
        let lower = line.to_ascii_lowercase();

        if lower.starts_with("last sync:") {
            last_sync = parse_last_sync(line);
            continue;
        }

        if let Some((name, status)) = parse_disk_status(line) {
            current_disk = Some(name.clone());
            disk_status.insert(name, status);
            continue;
        }

        if let Some((name, date)) = parse_scrub_line(line) {
            disk_scrub.insert(name, date);
            continue;
        }

        if lower.starts_with("scrub:") {
            if let (Some(name), Some(date)) = (
                current_disk.as_ref(),
                line.split_whitespace().find_map(|part| {
                    NaiveDate::parse_from_str(part.trim_end_matches(','), "%Y-%m-%d").ok()
                }),
            ) {
                disk_scrub.insert(name.clone(), date);
            }
        }
    }

    let disks = config
        .data
        .iter()
        .map(|disk| {
            let last_scrub = disk_scrub.get(&disk.name).copied();
            let scrub_age_days = last_scrub.map(|date| (today - date).num_days());
            let scrub_stale = scrub_age_days
                .map(|age| age > scrub_window_days)
                .unwrap_or(true);
            DiskState {
                name: disk.name.clone(),
                path: disk.path.clone(),
                status: disk_status
                    .get(&disk.name)
                    .copied()
                    .unwrap_or(DiskStatus::Unknown),
                last_scrub,
                scrub_age_days,
                scrub_stale,
            }
        })
        .collect();

    ArrayState {
        config: config.clone(),
        last_sync,
        disks,
        diff: None,
    }
}

pub fn parse_diff_output(text: &str) -> DiffSummary {
    let mut summary = DiffSummary::default();

    for raw in text.lines() {
        let line = raw.trim();
        if let Some((count, label)) = parse_count_label(line) {
            let label = label.to_ascii_lowercase();
            if label.contains("equal") {
                summary.equal = count;
            } else if label.contains("add") {
                summary.added = count;
            } else if label.contains("remove") || label.contains("delete") {
                summary.removed = count;
            } else if label.contains("update")
                || label.contains("change")
                || label.contains("modify")
            {
                summary.updated = count;
            } else if label.contains("move") || label.contains("rename") {
                summary.moved = count;
            } else if label.contains("copy") || label.contains("copi") {
                summary.copied = count;
            } else if label.contains("restore") {
                summary.restored = count;
            }
        }
    }

    summary
}

fn parse_count_label(line: &str) -> Option<(u64, &str)> {
    let line = line.trim_start_matches(|c: char| c == '-' || c.is_whitespace());
    let (left, right) = line.split_once(' ')?;
    let count = left.parse().ok()?;
    Some((count, right.trim()))
}

fn parse_last_sync(line: &str) -> Option<DateTime<Utc>> {
    let (_, value) = line.split_once(':')?;
    let value = value.trim();

    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
            DateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S %z").map(|dt| dt.with_timezone(&Utc))
        })
        .ok()
}

fn parse_disk_status(line: &str) -> Option<(String, DiskStatus)> {
    let lower = line.to_ascii_lowercase();
    if !lower.starts_with("disk ") {
        return None;
    }

    let mut parts = line.split_whitespace();
    parts.next()?;
    let name = parts.next()?.trim_end_matches(':').to_owned();
    let status = if lower.contains(" ok") || lower.ends_with(" ok") {
        DiskStatus::Ok
    } else if lower.contains("warning") {
        DiskStatus::Warning
    } else if lower.contains("error") || lower.contains("fail") {
        DiskStatus::Error
    } else {
        DiskStatus::Unknown
    };
    Some((name, status))
}

fn parse_scrub_line(line: &str) -> Option<(String, NaiveDate)> {
    let lower = line.to_ascii_lowercase();
    if !lower.starts_with("scrub ") {
        return None;
    }

    let mut parts = line.split_whitespace();
    parts.next()?;
    let name = parts.next()?.trim_end_matches(':').to_owned();
    let date = parts.find_map(|part| NaiveDate::parse_from_str(part, "%Y-%m-%d").ok())?;
    Some((name, date))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_config;

    const CONFIG: &str = r#"
        parity /mnt/parity/snapraid.parity
        content /var/snapraid.content
        data d1 /srv/disk1
        data d2 /srv/disk2
        data d3 /srv/disk3
        exclude *.tmp
    "#;
    const STATUS: &str = include_str!("../tests/fixtures/status.txt");
    const DIFF: &str = include_str!("../tests/fixtures/diff.txt");

    #[test]
    fn parses_status_fixture_into_array_state() {
        let config = parse_config(CONFIG);
        let state = parse_status_output(STATUS, &config, 30);

        assert_eq!(
            state.last_sync.unwrap().to_rfc3339(),
            "2026-06-20T03:14:00+00:00"
        );
        assert_eq!(state.disks.len(), 3);
        assert_eq!(state.disks[0].name, "d1");
        assert_eq!(state.disks[0].status, DiskStatus::Ok);
        assert_eq!(
            state.disks[0].last_scrub,
            Some(NaiveDate::from_ymd_opt(2026, 6, 12).unwrap())
        );
        assert!(!state.disks[0].scrub_stale);
        assert_eq!(state.disks[1].status, DiskStatus::Warning);
        assert!(state.disks[1].scrub_stale);
        assert_eq!(state.disks[2].status, DiskStatus::Ok);
    }

    #[test]
    fn parses_diff_fixture_into_summary() {
        let summary = parse_diff_output(DIFF);

        assert_eq!(summary.equal, 128_904);
        assert_eq!(summary.added, 42);
        assert_eq!(summary.removed, 9);
        assert_eq!(summary.updated, 17);
        assert_eq!(summary.moved, 3);
        assert_eq!(summary.copied, 2);
        assert_eq!(summary.restored, 1);
        assert_eq!(summary.changed(), 74);
        assert_eq!(summary.exit_code(200), crate::model::ExitCode::DriftPresent);
        assert_eq!(
            summary.exit_code(10),
            crate::model::ExitCode::DriftExceedsThreshold
        );
    }

    #[test]
    fn fixture_runner_returns_captured_outputs_without_snapraid() {
        let runner = FixtureSnapraidRunner {
            status_output: STATUS.to_owned(),
            diff_output: DIFF.to_owned(),
        };

        assert!(runner
            .status(Path::new("/does/not/matter"))
            .unwrap()
            .contains("Last sync"));
        assert!(runner
            .diff(Path::new("/does/not/matter"))
            .unwrap()
            .contains("There are differences"));
    }
}
