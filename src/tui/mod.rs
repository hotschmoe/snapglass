//! Ratatui dashboard and keyboard loop for interactive snapglass use.

pub mod app;

use std::io;
use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, Utc};
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, Gauge, Paragraph, Row, Table, TableState, Wrap,
};
use ratatui::Terminal;

use crate::config::SnapraidConfig;
use crate::model::{ArrayHealth, ArrayState};
use crate::snapraid::{parse_diff_output, parse_status_output, SnapraidRunner};

use self::app::{ConfirmAction, Pane, StatusLevel, StatusMessage, TuiApp};

// ---------------------------------------------------------------------------
// Centralized styling helpers — every health/age decision routes through these
// so color stays semantic and consistent across panes.
// ---------------------------------------------------------------------------

/// Foreground style for a given array-health state.
fn health_style(health: ArrayHealth) -> Style {
    match health {
        ArrayHealth::Ok => Style::default().fg(Color::Green),
        ArrayHealth::Warning => Style::default().fg(Color::Yellow),
        ArrayHealth::Danger => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ArrayHealth::Unknown => Style::default().fg(Color::DarkGray),
    }
}

/// Short label for an array-health state (replaces `{:?}` debug formatting).
fn health_label(health: ArrayHealth) -> &'static str {
    match health {
        ArrayHealth::Ok => "OK",
        ArrayHealth::Warning => "WARNING",
        ArrayHealth::Danger => "DANGER",
        ArrayHealth::Unknown => "UNKNOWN",
    }
}

/// Color an age (in days) against a freshness window: green well inside,
/// yellow approaching the edge, red past it.
fn age_style(days: i64, window: i64) -> Style {
    let window = window.max(1);
    if days <= window / 2 {
        Style::default().fg(Color::Green)
    } else if days <= window {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    }
}

/// Color a disk usage percentage: calm until it gets tight.
fn use_percent_style(percent: u8) -> Style {
    if percent >= 90 {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else if percent >= 75 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Green)
    }
}

fn status_line_style(level: StatusLevel) -> Style {
    match level {
        StatusLevel::Info => Style::default().fg(Color::Gray),
        StatusLevel::Ok => Style::default().fg(Color::Green),
        StatusLevel::Warn => Style::default().fg(Color::Yellow),
        StatusLevel::Error => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    }
}

/// Border style for a pane, brightened when focused.
fn pane_border_style(focused: bool) -> Style {
    if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn pane_block(title: &str, focused: bool) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(if focused {
            BorderType::Thick
        } else {
            BorderType::Plain
        })
        .border_style(pane_border_style(focused))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().add_modifier(Modifier::BOLD),
        ))
}

// ---------------------------------------------------------------------------
// Terminal lifecycle
// ---------------------------------------------------------------------------

pub fn run_tui<R: SnapraidRunner>(
    runner: &R,
    config_path: &Path,
    config: SnapraidConfig,
    scrub_window_days: i64,
    threshold: u64,
) -> anyhow::Result<()> {
    let mut app = TuiApp::new(config, scrub_window_days, threshold);
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
        terminal.draw(|frame| draw(frame, app))?;

        if !event::poll(Duration::from_millis(200))? {
            continue;
        }

        if let Event::Key(key) = event::read()? {
            // While a confirmation is pending, only Enter / Esc are meaningful;
            // everything else is swallowed so a stray key can't trigger anything.
            if app.pending_confirmation.is_some() {
                match key.code {
                    KeyCode::Enter => confirm(app, runner, config_path),
                    KeyCode::Esc => app.clear_confirmation(),
                    KeyCode::Char('q') => break,
                    _ => {}
                }
                continue;
            }

            match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Tab => app.cycle_pane(),
                KeyCode::Up => {
                    if app.active_pane == Pane::Disks {
                        app.move_selection(-1);
                    }
                }
                KeyCode::Down => {
                    if app.active_pane == Pane::Disks {
                        app.move_selection(1);
                    }
                }
                KeyCode::Char('d') => refresh_diff(app, runner, config_path),
                KeyCode::Char('r') => refresh_all(app, runner, config_path),
                KeyCode::Char('s') => app.request_confirmation(ConfirmAction::Sync),
                KeyCode::Char('c') => app.request_confirmation(ConfirmAction::Scrub),
                _ => {}
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Top-level draw: banner, three co-visible panes, footer, then modal on top.
// ---------------------------------------------------------------------------

fn draw(frame: &mut ratatui::Frame<'_>, app: &TuiApp) {
    let size = frame.size();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // health banner (dominant verdict)
            Constraint::Min(8),    // body: disks | (parity/scrub + diff)
            Constraint::Length(2), // status line + keymap legend
        ])
        .split(size);

    render_banner(frame, app, rows[0]);
    render_body(frame, app, rows[1]);
    render_footer(frame, app, rows[2]);

    // The confirmation modal is rendered last so it sits on top of everything.
    if let Some(action) = app.pending_confirmation {
        render_confirm_modal(frame, app, action, size);
    }
}

/// The single most important line on screen: a full-width colored verdict.
fn render_banner(frame: &mut ratatui::Frame<'_>, app: &TuiApp, area: Rect) {
    let (text, style) = banner_content(app);
    let banner = Paragraph::new(Line::from(Span::styled(text, style)))
        .alignment(Alignment::Center)
        .style(style);
    frame.render_widget(banner, area);
}

fn banner_content(app: &TuiApp) -> (String, Style) {
    let dim = Style::default().fg(Color::Black).bg(Color::DarkGray);

    let Some(array) = &app.array else {
        return ("  LOADING — reading snapraid status…  ".to_owned(), dim);
    };

    let drift = app.diff.as_ref().map(|d| d.changed()).unwrap_or(0);
    let risky = app.diff.as_ref().map(|d| d.risky_changed()).unwrap_or(0);
    let scrub_stale = array.scrub.stale || array.scrub.never_scrubbed;
    let over_threshold = risky > app.threshold;

    // Severity precedence: hard danger > over-threshold drift > drift/stale > ok.
    let (bg, text) = if array.health == ArrayHealth::Danger || over_threshold {
        let detail = if over_threshold {
            format!("{risky} removed/updated exceed threshold {}", app.threshold)
        } else {
            "array reports errors — review before syncing".to_owned()
        };
        (Color::Red, format!("DANGER — {detail}"))
    } else if drift > 0 || array.health == ArrayHealth::Warning || scrub_stale {
        let mut parts = Vec::new();
        if drift > 0 {
            parts.push(format!("{drift} changes pending sync"));
        }
        if array.scrub.never_scrubbed {
            parts.push("never scrubbed".to_owned());
        } else if array.scrub.stale {
            let oldest = array
                .scrub
                .oldest_days
                .map(|d| format!("scrub stale (oldest {d}d)"))
                .unwrap_or_else(|| "scrub stale".to_owned());
            parts.push(oldest);
        }
        if parts.is_empty() {
            parts.push("attention recommended".to_owned());
        }
        (Color::Yellow, format!("DRIFT — {}", parts.join(" · ")))
    } else if array.health == ArrayHealth::Unknown {
        return (
            "  UNKNOWN — could not determine array health  ".to_owned(),
            dim,
        );
    } else {
        (Color::Green, "ARRAY OK — in sync · scrub fresh".to_owned())
    };

    (
        format!("  {text}  "),
        Style::default()
            .fg(Color::Black)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    )
}

fn render_body(frame: &mut ratatui::Frame<'_>, app: &TuiApp, area: Rect) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);

    render_disks(frame, app, columns[0]);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(7), Constraint::Length(8)])
        .split(columns[1]);

    render_parity_scrub(frame, app, right[0]);
    render_diff(frame, app, right[1]);
}

// ---------------------------------------------------------------------------
// Disks pane — a real Table with right-aligned numerics and selection.
// ---------------------------------------------------------------------------

fn render_disks(frame: &mut ratatui::Frame<'_>, app: &TuiApp, area: Rect) {
    let focused = app.active_pane == Pane::Disks;
    let block = pane_block("Data Disks", focused);

    let Some(array) = &app.array else {
        let para = Paragraph::new(empty_lines(app, "no status loaded — press r to refresh"))
            .block(block)
            .wrap(Wrap { trim: true });
        frame.render_widget(para, area);
        return;
    };

    if array.disks.is_empty() {
        let para = Paragraph::new(vec![Line::from(Span::styled(
            "no data disks in config",
            Style::default().fg(Color::DarkGray),
        ))])
        .block(block);
        frame.render_widget(para, area);
        return;
    }

    let header = Row::new([
        cell_r("Disk"),
        cell_r("Files"),
        cell_r("Frag"),
        cell_r("Excess"),
        cell_r("Used GB"),
        cell_r("Free GB"),
        cell_r("Use%"),
    ])
    .style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );

    let rows = array.disks.iter().map(|disk| {
        let frag = disk.fragmented_files.unwrap_or(0);
        let excess = disk.excess_fragments.unwrap_or(0);
        let frag_style = if frag > 0 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let excess_style = if excess > 0 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let use_cell = match disk.use_percent {
            Some(p) => Cell::from(num_right(format!("{p}%"))).style(use_percent_style(p)),
            None => Cell::from(num_right("?")).style(Style::default().fg(Color::DarkGray)),
        };

        Row::new([
            Cell::from(disk.name.clone()),
            Cell::from(num_right(opt_u64(disk.files))),
            Cell::from(num_right(frag.to_string())).style(frag_style),
            Cell::from(num_right(excess.to_string())).style(excess_style),
            Cell::from(num_right(opt_str(disk.used_gb.as_deref()))),
            Cell::from(num_right(opt_str(disk.free_gb.as_deref()))),
            use_cell,
        ])
    });

    let widths = [
        Constraint::Length(8),
        Constraint::Length(11),
        Constraint::Length(6),
        Constraint::Length(7),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(6),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(Color::Indexed(236))
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(if focused { ">> " } else { "   " })
        .column_spacing(1);

    let mut state = TableState::default();
    state.select(Some(
        app.disk_selection.min(array.disks.len().saturating_sub(1)),
    ));
    frame.render_stateful_widget(table, area, &mut state);
}

// ---------------------------------------------------------------------------
// Parity & Scrub pane — sync status + scrub gauge + age band + chip.
// ---------------------------------------------------------------------------

fn render_parity_scrub(frame: &mut ratatui::Frame<'_>, app: &TuiApp, area: Rect) {
    let focused = app.active_pane == Pane::Parity;
    let block = pane_block("Parity & Scrub", focused);

    let Some(array) = &app.array else {
        let para = Paragraph::new(empty_lines(app, "no status loaded — press r to refresh"))
            .block(block)
            .wrap(Wrap { trim: true });
        frame.render_widget(para, area);
        return;
    };

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4), // sync facts
            Constraint::Length(1), // unscrubbed gauge
            Constraint::Min(2),    // age band + chip + messages
        ])
        .split(inner);

    // --- sync / parity facts ---
    let mut facts = Vec::new();
    facts.push(kv_line(
        "Last sync",
        last_sync_span(array.last_sync, app.scrub_window_days),
    ));
    facts.push(kv_line("Sync", sync_state_span(array)));
    facts.push(kv_line(
        "Health",
        Span::styled(health_label(array.health), health_style(array.health)),
    ));
    let err_span = match array.error_count {
        Some(0) | None => Span::styled("0", Style::default().fg(Color::Green)),
        Some(n) => Span::styled(
            n.to_string(),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
    };
    facts.push(kv_line("Errors", err_span));
    frame.render_widget(Paragraph::new(facts), chunks[0]);

    // --- unscrubbed gauge ---
    let unscrubbed = array.scrub.unscrubbed_percent.unwrap_or(0);
    let gauge_color = if array.scrub.never_scrubbed || array.scrub.stale {
        Color::Red
    } else if unscrubbed >= 50 {
        Color::Yellow
    } else {
        Color::Green
    };
    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(gauge_color))
        .percent(u16::from(unscrubbed.min(100)))
        .label(format!("{unscrubbed}% unscrubbed"));
    frame.render_widget(gauge, chunks[1]);

    // --- age band + chip ---
    let mut tail = Vec::new();
    let w = app.scrub_window_days;
    let band = Line::from(vec![
        Span::styled("oldest ", Style::default().fg(Color::DarkGray)),
        age_span(array.scrub.oldest_days, w),
        Span::raw("   "),
        Span::styled("med ", Style::default().fg(Color::DarkGray)),
        age_span(array.scrub.median_days, w),
        Span::raw("   "),
        Span::styled("new ", Style::default().fg(Color::DarkGray)),
        age_span(array.scrub.newest_days, w),
    ]);
    tail.push(band);

    let chip = if array.scrub.never_scrubbed {
        Span::styled(
            " NEVER SCRUBBED ",
            Style::default()
                .fg(Color::White)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD),
        )
    } else if array.scrub.stale {
        Span::styled(
            " STALE ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            " FRESH ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )
    };
    tail.push(Line::from(chip));

    for message in &array.messages {
        tail.push(Line::from(Span::styled(
            format!("• {message}"),
            Style::default().fg(Color::Yellow),
        )));
    }

    frame.render_widget(Paragraph::new(tail).wrap(Wrap { trim: true }), chunks[2]);
}

// ---------------------------------------------------------------------------
// Diff pane — colored counts with risky removed/updated highlighted.
// ---------------------------------------------------------------------------

fn render_diff(frame: &mut ratatui::Frame<'_>, app: &TuiApp, area: Rect) {
    let focused = app.active_pane == Pane::Diff;
    let block = pane_block("Pending Diff", focused);

    let Some(diff) = &app.diff else {
        let para = Paragraph::new(empty_lines(app, "no diff loaded — press d to refresh"))
            .block(block)
            .wrap(Wrap { trim: true });
        frame.render_widget(para, area);
        return;
    };

    let neutral = Style::default().fg(Color::Gray);
    let risky_style = |n: u64| {
        if n > 0 {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        }
    };
    let changed_style = |n: u64| {
        if n > 0 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        }
    };

    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        diff_count("equal", diff.equal, neutral),
        Span::raw("   "),
        diff_count("added", diff.added, changed_style(diff.added)),
        Span::raw("   "),
        diff_count("removed", diff.removed, risky_style(diff.removed)),
    ]));
    lines.push(Line::from(vec![
        diff_count("updated", diff.updated, risky_style(diff.updated)),
        Span::raw("   "),
        diff_count("moved", diff.moved, changed_style(diff.moved)),
        Span::raw("   "),
        diff_count("copied", diff.copied, changed_style(diff.copied)),
    ]));

    let changed = diff.changed();
    let risky = diff.risky_changed();
    lines.push(Line::from(vec![
        Span::styled("changed ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            changed.to_string(),
            if changed > 0 {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            },
        ),
        Span::raw("   "),
        Span::styled("risky ", Style::default().fg(Color::DarkGray)),
        Span::styled(risky.to_string(), risky_style(risky)),
        Span::styled(
            format!("  (threshold {})", app.threshold),
            Style::default().fg(Color::DarkGray),
        ),
    ]));

    if risky > app.threshold {
        lines.push(Line::from(Span::styled(
            format!(
                "OVER THRESHOLD — {risky} risky changes exceed {}",
                app.threshold
            ),
            Style::default()
                .fg(Color::White)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD),
        )));
    } else if changed == 0 {
        lines.push(Line::from(Span::styled(
            "IN SYNC — nothing pending",
            Style::default().fg(Color::Green),
        )));
    }

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

// ---------------------------------------------------------------------------
// Footer — transient status line over a static keymap legend.
// ---------------------------------------------------------------------------

fn render_footer(frame: &mut ratatui::Frame<'_>, app: &TuiApp, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);

    let status = Paragraph::new(Line::from(Span::styled(
        format!(" {}", app.status.text),
        status_line_style(app.status.level),
    )));
    frame.render_widget(status, rows[0]);

    // Render the keymap as alternating bold-cyan keys / dim labels.
    let mut spans = vec![Span::raw(" ")];
    for token in app.keymap_hint().split("  ") {
        if let Some((key, label)) = token.split_once(' ') {
            spans.push(Span::styled(
                key.to_owned(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(" {label}   "),
                Style::default().fg(Color::DarkGray),
            ));
        } else {
            spans.push(Span::styled(
                format!("{token}   "),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), rows[1]);
}

// ---------------------------------------------------------------------------
// Confirmation modal — centered, red double border, on top of everything.
// ---------------------------------------------------------------------------

fn render_confirm_modal(
    frame: &mut ratatui::Frame<'_>,
    app: &TuiApp,
    action: ConfirmAction,
    area: Rect,
) {
    let modal = centered_rect(56, 11, area);
    frame.render_widget(Clear, modal);

    let title = match action {
        ConfirmAction::Sync => " CONFIRM SYNC ",
        ConfirmAction::Scrub => " CONFIRM SCRUB ",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
        .title(Span::styled(
            title,
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(Color::Black));

    let impact = match action {
        ConfirmAction::Sync => {
            let risky = app.diff.as_ref().map(|d| d.risky_changed()).unwrap_or(0);
            let changed = app.diff.as_ref().map(|d| d.changed()).unwrap_or(0);
            format!("Pending: {changed} changes · {risky} removed/updated")
        }
        ConfirmAction::Scrub => {
            let pct = app
                .array
                .as_ref()
                .and_then(|a| a.scrub.unscrubbed_percent)
                .unwrap_or(0);
            format!("Will verify currently unscrubbed blocks ({pct}% unscrubbed)")
        }
    };

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "This runs a DESTRUCTIVE command:",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!("    {}    ", action.command()),
            Style::default()
                .fg(Color::White)
                .bg(Color::Indexed(236))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(impact, Style::default().fg(Color::Yellow))),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "  [ Enter ] RUN  ",
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("      "),
            Span::styled("[ Esc ] Cancel", Style::default().fg(Color::Gray)),
        ]),
    ];

    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .alignment(Alignment::Center),
        modal,
    );
}

// ---------------------------------------------------------------------------
// Small rendering helpers
// ---------------------------------------------------------------------------

fn empty_lines<'a>(app: &TuiApp, fallback: &'a str) -> Vec<Line<'a>> {
    // Distinguish error from a quiet "not loaded yet".
    match app.status.level {
        StatusLevel::Error => vec![Line::from(Span::styled(
            app.status.text.clone(),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ))],
        _ => vec![Line::from(Span::styled(
            fallback,
            Style::default().fg(Color::DarkGray),
        ))],
    }
}

fn kv_line<'a>(key: &'a str, value: Span<'a>) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{key:<10}"), Style::default().fg(Color::DarkGray)),
        value,
    ])
}

fn sync_state_span(array: &ArrayState) -> Span<'static> {
    match (array.sync_in_progress, array.fully_synced) {
        (Some(true), _) => Span::styled("in progress", Style::default().fg(Color::Cyan)),
        (_, Some(false)) => Span::styled("not fully synced", Style::default().fg(Color::Yellow)),
        (Some(false), Some(true)) => {
            Span::styled("idle, fully synced", Style::default().fg(Color::Green))
        }
        (Some(false), _) => Span::styled("idle", Style::default().fg(Color::Gray)),
        _ => Span::styled("unknown", Style::default().fg(Color::DarkGray)),
    }
}

fn last_sync_span(last_sync: Option<DateTime<Utc>>, window: i64) -> Span<'static> {
    match last_sync {
        Some(dt) => {
            let delta = Utc::now().signed_duration_since(dt);
            let days = delta.num_days();
            let label = humanize_duration(delta);
            Span::styled(label, age_style(days.max(0), window))
        }
        None => Span::styled(
            "never synced",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    }
}

fn humanize_duration(delta: chrono::Duration) -> String {
    let secs = delta.num_seconds().max(0);
    if secs < 60 {
        return "just now".to_owned();
    }
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let mins = (secs % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h ago")
    } else if hours > 0 {
        format!("{hours}h {mins}m ago")
    } else {
        format!("{mins}m ago")
    }
}

fn age_span(days: Option<i64>, window: i64) -> Span<'static> {
    match days {
        Some(d) => Span::styled(format!("{d}d"), age_style(d, window)),
        None => Span::styled("?", Style::default().fg(Color::DarkGray)),
    }
}

fn diff_count(label: &'static str, n: u64, value_style: Style) -> Span<'static> {
    Span::styled(
        format!("{label} {n}"),
        if n > 0 {
            value_style
        } else {
            Style::default().fg(Color::DarkGray)
        },
    )
}

fn cell_r(text: &'static str) -> Cell<'static> {
    Cell::from(Line::from(text).alignment(Alignment::Right))
}

fn num_right(text: impl Into<String>) -> Line<'static> {
    Line::from(text.into()).alignment(Alignment::Right)
}

fn opt_u64(v: Option<u64>) -> String {
    v.map(|n| n.to_string()).unwrap_or_else(|| "?".to_owned())
}

fn opt_str(v: Option<&str>) -> String {
    v.map(|s| s.to_owned()).unwrap_or_else(|| "?".to_owned())
}

/// A `Rect` of the given width/height centered inside `area`.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

// ---------------------------------------------------------------------------
// Data refresh
// ---------------------------------------------------------------------------

fn refresh_all<R: SnapraidRunner>(app: &mut TuiApp, runner: &R, config_path: &Path) {
    match runner.status(config_path) {
        Ok(output) => {
            app.array = Some(parse_status_output(
                &output,
                &app.config,
                app.scrub_window_days,
            ));
            app.clamp_selection();
            app.status = StatusMessage::ok("status refreshed");
        }
        Err(error) => app.status = StatusMessage::error(error.to_string()),
    }
    refresh_diff(app, runner, config_path);
}

fn refresh_diff<R: SnapraidRunner>(app: &mut TuiApp, runner: &R, config_path: &Path) {
    match runner.diff(config_path) {
        Ok(output) => {
            app.diff = Some(parse_diff_output(&output));
            if !matches!(app.status.level, StatusLevel::Error) {
                app.status = StatusMessage::ok("diff refreshed");
            }
        }
        Err(error) => app.status = StatusMessage::error(error.to_string()),
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

    app.status = match result {
        Ok(_) => StatusMessage::ok(format!("{action} completed")),
        Err(error) => StatusMessage::error(error.to_string()),
    };

    // After a sync/scrub the array state is stale; pull fresh status.
    if app.status.level == StatusLevel::Ok {
        refresh_all(app, runner, config_path);
    }
}
