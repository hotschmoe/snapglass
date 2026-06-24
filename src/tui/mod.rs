//! Ratatui dashboard and keyboard loop for interactive snapglass use.

pub mod app;

use std::io;
use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Tabs};
use ratatui::Terminal;

use crate::config::SnapraidConfig;
use crate::snapraid::{parse_diff_output, parse_status_output, SnapraidRunner};

use self::app::{ConfirmAction, Pane, TuiApp};

pub fn run_tui<R: SnapraidRunner>(
    runner: &R,
    config_path: &Path,
    config: SnapraidConfig,
    scrub_window_days: i64,
) -> anyhow::Result<()> {
    let mut app = TuiApp::new(config, scrub_window_days);
    refresh_all(&mut app, runner, config_path);

    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("create terminal")?;

    let result = run_loop(&mut terminal, &mut app, runner, config_path);

    disable_raw_mode().context("disable raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen).context("leave alternate screen")?;
    terminal.show_cursor().context("show cursor")?;

    result
}

fn run_loop<R: SnapraidRunner>(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut TuiApp,
    runner: &R,
    config_path: &Path,
) -> anyhow::Result<()> {
    loop {
        terminal.draw(|frame| {
            let size = frame.size();
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(6),
                    Constraint::Length(3),
                ])
                .split(size);

            let titles = ["Disks", "Parity", "Diff"]
                .iter()
                .map(|title| Line::from(Span::styled(*title, Style::default().fg(Color::Cyan))))
                .collect::<Vec<_>>();
            let tabs = Tabs::new(titles)
                .select(app.active_pane.index())
                .block(Block::default().borders(Borders::ALL).title("snapglass"))
                .highlight_style(Style::default().add_modifier(Modifier::BOLD));
            frame.render_widget(tabs, rows[0]);

            match app.active_pane {
                Pane::Disks => render_disks(frame, app, rows[1]),
                Pane::Parity => render_parity(frame, app, rows[1]),
                Pane::Diff => render_diff(frame, app, rows[1]),
            }

            let footer = Paragraph::new(app.footer_text())
                .block(Block::default().borders(Borders::ALL).title("keys"));
            frame.render_widget(footer, rows[2]);
        })?;

        if !event::poll(Duration::from_millis(200))? {
            continue;
        }

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Tab => app.cycle_pane(),
                KeyCode::Char('d') => refresh_diff(app, runner, config_path),
                KeyCode::Char('r') => refresh_all(app, runner, config_path),
                KeyCode::Char('s') => app.request_confirmation(ConfirmAction::Sync),
                KeyCode::Char('c') => app.request_confirmation(ConfirmAction::Scrub),
                KeyCode::Enter => confirm(app, runner, config_path),
                KeyCode::Esc => app.clear_confirmation(),
                _ => {}
            }
        }
    }

    Ok(())
}

fn render_disks(frame: &mut ratatui::Frame<'_>, app: &TuiApp, area: ratatui::layout::Rect) {
    let items = app
        .array
        .as_ref()
        .map(|array| {
            array
                .disks
                .iter()
                .map(|disk| {
                    ListItem::new(format!(
                        "{}  {:?}  files {}  frag {}  use {}  {}",
                        disk.name,
                        disk.status,
                        disk.files
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "unknown".to_owned()),
                        disk.fragmented_files
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "unknown".to_owned()),
                        disk.use_percent
                            .map(|value| format!("{value}%"))
                            .unwrap_or_else(|| "unknown".to_owned()),
                        disk.path
                    ))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec![ListItem::new("no status loaded")]);

    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title("disks")),
        area,
    );
}

fn render_parity(frame: &mut ratatui::Frame<'_>, app: &TuiApp, area: ratatui::layout::Rect) {
    let mut lines = Vec::new();
    lines.push(Line::from(format!(
        "last sync: {}",
        app.array
            .as_ref()
            .and_then(|array| array.last_sync)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| "unknown".to_owned())
    )));
    if let Some(array) = &app.array {
        lines.push(Line::from(format!("health: {:?}", array.health)));
        lines.push(Line::from(format!(
            "sync: {}",
            match (array.sync_in_progress, array.fully_synced) {
                (Some(true), _) => "in progress",
                (_, Some(false)) => "not fully synced",
                (Some(false), Some(true)) => "idle, fully synced",
                (Some(false), _) => "idle",
                _ => "unknown",
            }
        )));
        lines.push(Line::from(format!(
            "scrub: {}% unscrubbed, oldest {}, {}",
            array
                .scrub
                .unscrubbed_percent
                .map(|percent| percent.to_string())
                .unwrap_or_else(|| "unknown".to_owned()),
            array
                .scrub
                .oldest_days
                .map(|days| format!("{days}d"))
                .unwrap_or_else(|| "unknown".to_owned()),
            if array.scrub.stale { "STALE" } else { "ok" }
        )));
        lines.extend(
            array
                .messages
                .iter()
                .map(|message| Line::from(format!("message: {message}"))),
        );
    }
    lines.push(Line::from(""));
    lines.extend(
        app.config
            .parity
            .iter()
            .map(|path| Line::from(path.clone())),
    );

    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("parity")),
        area,
    );
}

fn render_diff(frame: &mut ratatui::Frame<'_>, app: &TuiApp, area: ratatui::layout::Rect) {
    let text = app
        .diff
        .as_ref()
        .map(|diff| {
            vec![
                Line::from(format!("equal: {}", diff.equal)),
                Line::from(format!("added: {}", diff.added)),
                Line::from(format!("removed: {}", diff.removed)),
                Line::from(format!("updated: {}", diff.updated)),
                Line::from(format!("moved: {}", diff.moved)),
                Line::from(format!("copied: {}", diff.copied)),
                Line::from(format!("restored: {}", diff.restored)),
                Line::from(format!("changed: {}", diff.changed())),
            ]
        })
        .unwrap_or_else(|| vec![Line::from("no diff loaded")]);

    frame.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title("diff")),
        area,
    );
}

fn refresh_all<R: SnapraidRunner>(app: &mut TuiApp, runner: &R, config_path: &Path) {
    match runner.status(config_path) {
        Ok(output) => {
            app.array = Some(parse_status_output(
                &output,
                &app.config,
                app.scrub_window_days,
            ));
            app.message = "status refreshed".to_owned();
        }
        Err(error) => app.message = error.to_string(),
    }
    refresh_diff(app, runner, config_path);
}

fn refresh_diff<R: SnapraidRunner>(app: &mut TuiApp, runner: &R, config_path: &Path) {
    match runner.diff(config_path) {
        Ok(output) => {
            app.diff = Some(parse_diff_output(&output));
            app.message = "diff refreshed".to_owned();
        }
        Err(error) => app.message = error.to_string(),
    }
}

fn confirm<R: SnapraidRunner>(app: &mut TuiApp, runner: &R, config_path: &Path) {
    let Some(action) = app.pending_confirmation.take() else {
        return;
    };

    let result = match action {
        ConfirmAction::Sync => runner.sync(config_path),
        ConfirmAction::Scrub => runner.scrub(config_path),
    };

    app.message = match result {
        Ok(_) => format!("{action} completed"),
        Err(error) => error.to_string(),
    };
}
