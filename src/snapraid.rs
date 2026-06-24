//! Process wrapper and parsers for human-readable `snapraid status` and `snapraid diff` output.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;

use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::config::SnapraidConfig;
use crate::model::{ArrayHealth, ArrayState, DiffSummary, DiskState, DiskStatus, ScrubState};

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
    let config_names = config
        .data
        .iter()
        .map(|disk| disk.name.as_str())
        .collect::<HashSet<_>>();
    let mut disk_rows = HashMap::new();
    let mut last_sync = None;
    let mut scrub = ScrubState::default();
    let mut sync_in_progress = None;
    let mut fully_synced = None;
    let mut messages = Vec::new();
    let mut error_count = None;
    let mut saw_ok_health = false;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();

        if lower.starts_with("last sync:") {
            last_sync = parse_last_sync(line);
            continue;
        }

        if let Some(row) = parse_status_table_row(line, &config_names) {
            disk_rows.insert(row.name.clone(), row);
            continue;
        }

        if let Some(percent) = parse_unscrubbed_percent(line) {
            scrub.unscrubbed_percent = Some(percent);
            scrub.never_scrubbed = percent == 100;
            continue;
        }

        if lower.contains("oldest block was scrubbed") {
            if let Some((oldest, median, newest)) = parse_scrub_age_line(line) {
                scrub.oldest_days = Some(oldest);
                scrub.median_days = Some(median);
                scrub.newest_days = Some(newest);
            }
            continue;
        }

        if lower.contains("not scrubbed") && lower.contains("all") {
            scrub.never_scrubbed = true;
            scrub.unscrubbed_percent.get_or_insert(100);
            messages.push(line.to_owned());
            continue;
        }

        if lower == "no sync is in progress." {
            sync_in_progress = Some(false);
            fully_synced.get_or_insert(true);
            continue;
        }

        if lower.contains("sync is in progress") {
            sync_in_progress = Some(true);
            continue;
        }

        if lower.contains("not fully synced") || lower.contains("has never been synced") {
            fully_synced = Some(false);
            messages.push(line.to_owned());
            continue;
        }

        if lower.starts_with("warning!") || lower.starts_with("danger!") {
            if lower.starts_with("danger!") {
                error_count = parse_error_count(line).or(error_count);
            }
            messages.push(line.to_owned());
            continue;
        }

        if lower.trim_end_matches('.') == "no error detected" {
            saw_ok_health = true;
            continue;
        }

        if lower.contains("error") {
            error_count = parse_error_count(line).or(error_count);
            messages.push(line.to_owned());
        }
    }

    scrub.stale = scrub.never_scrubbed
        || scrub
            .oldest_days
            .map(|age| age > scrub_window_days)
            .unwrap_or(false);

    let health = classify_health(&messages, error_count, saw_ok_health);

    let disks = config
        .data
        .iter()
        .map(|disk| {
            let row = disk_rows.get(&disk.name);
            DiskState {
                name: disk.name.clone(),
                path: disk.path.clone(),
                status: row.map(|_| DiskStatus::Ok).unwrap_or(DiskStatus::Unknown),
                files: row.and_then(|row| row.files),
                fragmented_files: row.and_then(|row| row.fragmented_files),
                excess_fragments: row.and_then(|row| row.excess_fragments),
                wasted_gb: row.and_then(|row| row.wasted_gb.clone()),
                used_gb: row.and_then(|row| row.used_gb.clone()),
                free_gb: row.and_then(|row| row.free_gb.clone()),
                use_percent: row.and_then(|row| row.use_percent),
            }
        })
        .collect();

    ArrayState {
        config: config.clone(),
        last_sync,
        scrub,
        sync_in_progress,
        fully_synced,
        health,
        messages,
        error_count,
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct StatusDiskRow {
    name: String,
    files: Option<u64>,
    fragmented_files: Option<u64>,
    excess_fragments: Option<u64>,
    wasted_gb: Option<String>,
    used_gb: Option<String>,
    free_gb: Option<String>,
    use_percent: Option<u8>,
}

fn parse_status_table_row(line: &str, config_names: &HashSet<&str>) -> Option<StatusDiskRow> {
    let parts = line.split_whitespace().collect::<Vec<_>>();
    let name = parts.last()?.trim();
    if !config_names.contains(name) {
        return None;
    }

    let values = &parts[..parts.len() - 1];
    let mut row = StatusDiskRow {
        name: name.to_owned(),
        ..StatusDiskRow::default()
    };

    match values {
        [files, fragmented, excess, wasted, used, free, use_percent, ..] => {
            row.files = parse_u64_token(files);
            row.fragmented_files = parse_u64_token(fragmented);
            row.excess_fragments = parse_u64_token(excess);
            row.wasted_gb = parse_value_token(wasted);
            row.used_gb = parse_value_token(used);
            row.free_gb = parse_value_token(free);
            row.use_percent = parse_percent_token(use_percent);
        }
        [files, fragmented, excess, used, free, use_percent] => {
            row.files = parse_u64_token(files);
            row.fragmented_files = parse_u64_token(fragmented);
            row.excess_fragments = parse_u64_token(excess);
            row.used_gb = parse_value_token(used);
            row.free_gb = parse_value_token(free);
            row.use_percent = parse_percent_token(use_percent);
        }
        [files, fragmented, used, free, use_percent] => {
            row.files = parse_u64_token(files);
            row.fragmented_files = parse_u64_token(fragmented);
            row.used_gb = parse_value_token(used);
            row.free_gb = parse_value_token(free);
            row.use_percent = parse_percent_token(use_percent);
        }
        _ => return None,
    }

    Some(row)
}

fn parse_unscrubbed_percent(line: &str) -> Option<u8> {
    let lower = line.to_ascii_lowercase();
    if !lower.contains("of the array is not scrubbed") {
        return None;
    }

    line.split_whitespace()
        .find_map(|part| parse_percent_token(part.trim_end_matches('.')))
}

fn parse_scrub_age_line(line: &str) -> Option<(i64, i64, i64)> {
    let numbers = numbers_in_line(line);
    match numbers.as_slice() {
        [oldest, median, newest, ..] => Some((*oldest, *median, *newest)),
        _ => None,
    }
}

fn parse_error_count(line: &str) -> Option<u64> {
    let lower = line.to_ascii_lowercase();
    if !lower.contains("error") {
        return None;
    }

    line.split_whitespace()
        .find_map(|part| parse_u64_token(part.trim_matches(|c: char| !c.is_ascii_digit())))
}

fn classify_health(
    messages: &[String],
    error_count: Option<u64>,
    saw_ok_health: bool,
) -> ArrayHealth {
    if error_count.unwrap_or(0) > 0
        || messages
            .iter()
            .any(|message| message.to_ascii_lowercase().starts_with("danger!"))
    {
        ArrayHealth::Danger
    } else if messages
        .iter()
        .map(|message| message.to_ascii_lowercase())
        .any(|message| {
            message.starts_with("warning!")
                || message.contains("not fully synced")
                || message.contains("never been synced")
        })
    {
        ArrayHealth::Warning
    } else if saw_ok_health {
        ArrayHealth::Ok
    } else {
        ArrayHealth::Unknown
    }
}

fn parse_u64_token(token: &str) -> Option<u64> {
    token.replace(',', "").parse().ok()
}

fn parse_value_token(token: &str) -> Option<String> {
    let token = token.trim();
    if token.is_empty() || token == "-" {
        None
    } else {
        Some(token.to_owned())
    }
}

fn parse_percent_token(token: &str) -> Option<u8> {
    token.trim_end_matches('%').parse().ok()
}

fn numbers_in_line(line: &str) -> Vec<i64> {
    let mut numbers = Vec::new();
    let mut current = String::new();

    for ch in line.chars() {
        if ch.is_ascii_digit() {
            current.push(ch);
        } else if !current.is_empty() {
            if let Ok(number) = current.parse() {
                numbers.push(number);
            }
            current.clear();
        }
    }

    if !current.is_empty() {
        if let Ok(number) = current.parse() {
            numbers.push(number);
        }
    }

    numbers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_config;
    use crate::model::ExitCode;

    const CONFIG: &str = r#"
        parity /mnt/parity/snapraid.parity
        content /var/snapraid.content
        data d1 /srv/disk1
        data d2 /srv/disk2
        data d3 /srv/disk3
        exclude *.tmp
    "#;
    const STATUS_12_CLEAN: &str = include_str!("../tests/fixtures/status_12_clean_in_sync.txt");
    const STATUS_11_CLEAN: &str = include_str!("../tests/fixtures/status_11_clean_in_sync.txt");
    const STATUS_NEVER_SYNCED: &str = include_str!("../tests/fixtures/status_never_synced.txt");
    const STATUS_NEVER_SCRUBBED: &str = include_str!("../tests/fixtures/status_never_scrubbed.txt");
    const STATUS_DANGER_ERRORS: &str = include_str!("../tests/fixtures/status_danger_errors.txt");
    const STATUS_EMPTY: &str = include_str!("../tests/fixtures/status_empty.txt");
    const DIFF_ZERO: &str = include_str!("../tests/fixtures/diff_zero.txt");
    const DIFF_THRESHOLD_EXCEEDED: &str =
        include_str!("../tests/fixtures/diff_threshold_exceeded.txt");

    #[test]
    fn parses_status_12_table_and_array_scrub_age() {
        let config = parse_config(CONFIG);
        let state = parse_status_output(STATUS_12_CLEAN, &config, 30);

        assert_eq!(state.health, ArrayHealth::Ok);
        assert_eq!(state.sync_in_progress, Some(false));
        assert_eq!(state.fully_synced, Some(true));
        assert_eq!(state.scrub.unscrubbed_percent, Some(47));
        assert_eq!(state.scrub.oldest_days, Some(12));
        assert_eq!(state.scrub.median_days, Some(6));
        assert_eq!(state.scrub.newest_days, Some(0));
        assert!(!state.scrub.stale);
        assert_eq!(state.disks.len(), 3);
        assert_eq!(state.disks[0].name, "d1");
        assert_eq!(state.disks[0].status, DiskStatus::Ok);
        assert_eq!(state.disks[0].files, Some(3));
        assert_eq!(state.disks[1].fragmented_files, Some(1));
        assert_eq!(state.disks[1].excess_fragments, Some(3));
        assert_eq!(state.disks[1].used_gb.as_deref(), Some("104"));
        assert_eq!(state.disks[1].free_gb.as_deref(), Some("833"));
        assert_eq!(state.disks[1].use_percent, Some(11));
        assert_eq!(state.disks[2].use_percent, Some(0));
    }

    #[test]
    fn parses_status_11_table_without_wasted_column() {
        let config = parse_config(CONFIG);
        let state = parse_status_output(STATUS_11_CLEAN, &config, 30);

        assert_eq!(state.health, ArrayHealth::Ok);
        assert_eq!(state.scrub.unscrubbed_percent, Some(3));
        assert_eq!(state.scrub.oldest_days, Some(3));
        assert_eq!(state.disks[0].files, Some(30));
        assert_eq!(state.disks[0].wasted_gb, None);
        assert_eq!(state.disks[0].used_gb.as_deref(), Some("540"));
        assert_eq!(state.disks[0].free_gb.as_deref(), Some("397"));
        assert_eq!(state.disks[0].use_percent, Some(58));
    }

    #[test]
    fn parses_never_synced_warning() {
        let config = parse_config(CONFIG);
        let state = parse_status_output(STATUS_NEVER_SYNCED, &config, 30);

        assert_eq!(state.health, ArrayHealth::Warning);
        assert_eq!(state.fully_synced, Some(false));
        assert_eq!(state.sync_in_progress, Some(false));
        assert!(state.scrub.never_scrubbed);
        assert!(state.scrub.stale);
        assert!(state
            .messages
            .iter()
            .any(|message| message.contains("NOT fully synced")));
    }

    #[test]
    fn parses_never_scrubbed_as_array_level_stale() {
        let config = parse_config(CONFIG);
        let state = parse_status_output(STATUS_NEVER_SCRUBBED, &config, 30);

        assert_eq!(state.health, ArrayHealth::Warning);
        assert_eq!(state.scrub.unscrubbed_percent, Some(100));
        assert!(state.scrub.never_scrubbed);
        assert!(state.scrub.stale);
        assert_eq!(state.disks[0].status, DiskStatus::Ok);
    }

    #[test]
    fn parses_danger_and_error_count() {
        let config = parse_config(CONFIG);
        let state = parse_status_output(STATUS_DANGER_ERRORS, &config, 30);

        assert_eq!(state.health, ArrayHealth::Danger);
        assert_eq!(state.error_count, Some(5));
        assert_eq!(state.fully_synced, Some(false));
        assert_eq!(state.scrub.oldest_days, Some(75));
        assert!(state.scrub.stale);
        assert!(state
            .messages
            .iter()
            .any(|message| message.starts_with("DANGER!")));
    }

    #[test]
    fn parses_empty_status_as_unknown_without_panicking() {
        let config = parse_config(CONFIG);
        let state = parse_status_output(STATUS_EMPTY, &config, 30);

        assert_eq!(state.health, ArrayHealth::Unknown);
        assert_eq!(state.sync_in_progress, None);
        assert_eq!(state.fully_synced, None);
        assert_eq!(state.scrub, ScrubState::default());
        assert_eq!(state.disks.len(), 3);
        assert!(state
            .disks
            .iter()
            .all(|disk| disk.status == DiskStatus::Unknown));
    }

    #[test]
    fn parses_zero_diff_summary() {
        let summary = parse_diff_output(DIFF_ZERO);

        assert_eq!(summary.changed(), 0);
        assert_eq!(summary.risky_changed(), 0);
        assert_eq!(summary.exit_code(200), ExitCode::InSync);
    }

    #[test]
    fn parses_diff_summary_with_threshold_exceeded() {
        let summary = parse_diff_output(DIFF_THRESHOLD_EXCEEDED);

        assert_eq!(summary.equal, 128_904);
        assert_eq!(summary.added, 42);
        assert_eq!(summary.removed, 9);
        assert_eq!(summary.updated, 17);
        assert_eq!(summary.moved, 3);
        assert_eq!(summary.copied, 2);
        assert_eq!(summary.restored, 1);
        assert_eq!(summary.changed(), 74);
        assert_eq!(summary.exit_code(200), ExitCode::DriftPresent);
        assert_eq!(summary.exit_code(10), ExitCode::DriftExceedsThreshold);
    }

    #[test]
    fn fixture_runner_returns_captured_outputs_without_snapraid() {
        let runner = FixtureSnapraidRunner {
            status_output: STATUS_12_CLEAN.to_owned(),
            diff_output: DIFF_THRESHOLD_EXCEEDED.to_owned(),
        };

        assert!(runner
            .status(Path::new("/does/not/matter"))
            .unwrap()
            .contains("SnapRAID status report"));
        assert!(runner
            .diff(Path::new("/does/not/matter"))
            .unwrap()
            .contains("There are differences"));
    }
}
