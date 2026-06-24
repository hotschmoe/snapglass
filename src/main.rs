//! Clap CLI dispatch for snapglass commands.

mod config;
mod model;
mod snapraid;
mod tui;

use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};

use crate::model::ExitCode;
use crate::snapraid::{
    parse_diff_output, parse_status_output, ProcessSnapraidRunner, SnapraidRunner,
};

#[derive(Debug, Parser)]
#[command(
    name = "snapglass",
    version,
    about = "A read-mostly lens over snapRAID state"
)]
struct Cli {
    #[arg(long, global = true, default_value = "/etc/snapraid.conf")]
    config: PathBuf,

    #[arg(long, global = true, default_value_t = 30)]
    scrub_window_days: i64,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Status,
    Diff {
        #[arg(long, default_value_t = 200)]
        threshold: u64,
    },
    Tui,
}

fn main() {
    let cli = Cli::parse();
    let runner = ProcessSnapraidRunner;

    let code = match run(cli, &runner) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::RuntimeFailure
        }
    };

    process::exit(code.as_i32());
}

fn run<R: SnapraidRunner>(cli: Cli, runner: &R) -> anyhow::Result<ExitCode> {
    let config = config::read_config(&cli.config)?;

    match cli.command {
        Command::Status => {
            let status_output = runner.status(&cli.config)?;
            let diff_output = runner.diff(&cli.config)?;
            let diff = parse_diff_output(&diff_output);
            let state = parse_status_output(&status_output, &config, cli.scrub_window_days)
                .attach_diff(diff);
            print_status(&state);
            Ok(state
                .diff
                .as_ref()
                .map(|diff| diff.exit_code(200))
                .unwrap_or(ExitCode::InSync))
        }
        Command::Diff { threshold } => {
            let output = runner.diff(&cli.config)?;
            let diff = parse_diff_output(&output);
            print_diff(&diff, threshold);
            Ok(diff.exit_code(threshold))
        }
        Command::Tui => {
            tui::run_tui(runner, &cli.config, config, cli.scrub_window_days)?;
            Ok(ExitCode::InSync)
        }
    }
}

fn print_status(state: &model::ArrayState) {
    println!("snapglass status");
    println!(
        "last sync: {}",
        state
            .last_sync
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| "unknown".to_owned())
    );
    println!("data disks: {}", state.disks.len());
    println!("parity files: {}", state.config.parity.len());
    println!("content files: {}", state.config.content.len());

    if let Some(diff) = &state.diff {
        println!(
            "drift: {} changed ({} added, {} removed, {} updated, {} moved, {} copied, {} restored)",
            diff.changed(),
            diff.added,
            diff.removed,
            diff.updated,
            diff.moved,
            diff.copied,
            diff.restored
        );
    }

    println!();
    for disk in &state.disks {
        let scrub = disk
            .last_scrub
            .map(|date| date.to_string())
            .unwrap_or_else(|| "unknown".to_owned());
        let stale = if disk.scrub_stale { "stale" } else { "ok" };
        println!(
            "{}: {:?}, last scrub {}, scrub age {}, {}",
            disk.name,
            disk.status,
            scrub,
            disk.scrub_age_days
                .map(|days| format!("{days} days"))
                .unwrap_or_else(|| "unknown".to_owned()),
            stale
        );
    }
}

fn print_diff(diff: &model::DiffSummary, threshold: u64) {
    println!("snapglass diff");
    println!("equal: {}", diff.equal);
    println!("added: {}", diff.added);
    println!("removed: {}", diff.removed);
    println!("updated: {}", diff.updated);
    println!("moved: {}", diff.moved);
    println!("copied: {}", diff.copied);
    println!("restored: {}", diff.restored);
    println!("changed: {}", diff.changed());
    println!(
        "risky changed: {} (threshold {})",
        diff.risky_changed(),
        threshold
    );
}
