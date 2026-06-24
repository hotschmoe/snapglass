//! Lightweight TUI application state separated from rendering and terminal I/O.

use std::fmt;

use crate::config::SnapraidConfig;
use crate::model::{ArrayState, DiffSummary};

#[derive(Debug, Clone)]
pub struct TuiApp {
    pub config: SnapraidConfig,
    pub scrub_window_days: i64,
    pub threshold: u64,
    pub active_pane: Pane,
    pub array: Option<ArrayState>,
    pub diff: Option<DiffSummary>,
    pub disk_selection: usize,
    pub pending_confirmation: Option<ConfirmAction>,
    pub status: StatusMessage,
}

impl TuiApp {
    pub fn new(config: SnapraidConfig, scrub_window_days: i64, threshold: u64) -> Self {
        Self {
            config,
            scrub_window_days,
            threshold,
            active_pane: Pane::Disks,
            array: None,
            diff: None,
            disk_selection: 0,
            pending_confirmation: None,
            status: StatusMessage::info("loading"),
        }
    }

    pub fn cycle_pane(&mut self) {
        self.active_pane = self.active_pane.next();
    }

    pub fn disk_count(&self) -> usize {
        self.array
            .as_ref()
            .map(|array| array.disks.len())
            .unwrap_or(0)
    }

    /// Move the disk-table cursor when the Disks pane is focused. Wraps.
    pub fn move_selection(&mut self, delta: isize) {
        let count = self.disk_count();
        if count == 0 {
            self.disk_selection = 0;
            return;
        }
        let len = count as isize;
        let next = (self.disk_selection as isize + delta).rem_euclid(len);
        self.disk_selection = next as usize;
    }

    /// Keep the selection index in range after a refresh changes disk count.
    pub fn clamp_selection(&mut self) {
        let count = self.disk_count();
        if count == 0 {
            self.disk_selection = 0;
        } else if self.disk_selection >= count {
            self.disk_selection = count - 1;
        }
    }

    pub fn request_confirmation(&mut self, action: ConfirmAction) {
        self.pending_confirmation = Some(action);
        self.status = StatusMessage::warn(format!("confirm {action}: Enter to run, Esc to cancel"));
    }

    pub fn clear_confirmation(&mut self) {
        if self.pending_confirmation.take().is_some() {
            self.status = StatusMessage::info("cancelled");
        }
    }

    /// Static keymap legend, rendered separately from the transient status line.
    pub fn keymap_hint(&self) -> &'static str {
        if self.pending_confirmation.is_some() {
            "Enter run  Esc cancel"
        } else {
            "s sync  c scrub  d diff  r refresh  up/down select  tab focus  q quit"
        }
    }
}

/// A transient status line message with a severity used to pick its color.
#[derive(Debug, Clone)]
pub struct StatusMessage {
    pub text: String,
    pub level: StatusLevel,
}

impl StatusMessage {
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            level: StatusLevel::Info,
        }
    }

    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            level: StatusLevel::Ok,
        }
    }

    pub fn warn(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            level: StatusLevel::Warn,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            level: StatusLevel::Error,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLevel {
    Info,
    Ok,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Disks,
    Parity,
    Diff,
}

impl Pane {
    fn next(self) -> Self {
        match self {
            Self::Disks => Self::Parity,
            Self::Parity => Self::Diff,
            Self::Diff => Self::Disks,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmAction {
    Sync,
    Scrub,
}

impl ConfirmAction {
    /// The literal command snapglass will hand to snapraid, shown in the modal.
    pub fn command(self) -> &'static str {
        match self {
            Self::Sync => "snapraid sync",
            Self::Scrub => "snapraid scrub",
        }
    }
}

impl fmt::Display for ConfirmAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sync => f.write_str("sync"),
            Self::Scrub => f.write_str("scrub"),
        }
    }
}
