//! Lightweight TUI application state separated from rendering and terminal I/O.

use std::fmt;

use crate::config::SnapraidConfig;
use crate::model::{ArrayState, DiffSummary};

#[derive(Debug, Clone)]
pub struct TuiApp {
    pub config: SnapraidConfig,
    pub scrub_window_days: i64,
    pub active_pane: Pane,
    pub array: Option<ArrayState>,
    pub diff: Option<DiffSummary>,
    pub pending_confirmation: Option<ConfirmAction>,
    pub message: String,
}

impl TuiApp {
    pub fn new(config: SnapraidConfig, scrub_window_days: i64) -> Self {
        Self {
            config,
            scrub_window_days,
            active_pane: Pane::Disks,
            array: None,
            diff: None,
            pending_confirmation: None,
            message: "loading".to_owned(),
        }
    }

    pub fn cycle_pane(&mut self) {
        self.active_pane = self.active_pane.next();
    }

    pub fn request_confirmation(&mut self, action: ConfirmAction) {
        self.pending_confirmation = Some(action);
        self.message = format!("press Enter to confirm {action}, Esc to cancel");
    }

    pub fn clear_confirmation(&mut self) {
        if self.pending_confirmation.take().is_some() {
            self.message = "cancelled".to_owned();
        }
    }

    pub fn footer_text(&self) -> String {
        match self.pending_confirmation {
            Some(action) => format!("confirm {action}: Enter | cancel: Esc | {}", self.message),
            None => format!(
                "s sync | c scrub | d diff | r refresh | tab pane | q quit | {}",
                self.message
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Disks,
    Parity,
    Diff,
}

impl Pane {
    pub fn index(self) -> usize {
        match self {
            Self::Disks => 0,
            Self::Parity => 1,
            Self::Diff => 2,
        }
    }

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

impl fmt::Display for ConfirmAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sync => f.write_str("sync"),
            Self::Scrub => f.write_str("scrub"),
        }
    }
}
