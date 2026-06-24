# snapglass

A read-mostly lens over [snapRAID](https://www.snapraid.it/) state. A Rust CLI + TUI for
seeing — at a glance — whether your array is synced, how stale your parity is, when you last
scrubbed, and what's drifted since.

Built for a single Ubuntu box that replaced an unRAID setup. Personal tool, not a distro package.

## Why

`snapraid status` and `snapraid diff` give you a lot, but as walls of text you have to
re-read every time. snapglass parses that output (and the snapRAID config) into a glanceable
dashboard: parity health, sync drift, array scrub age, and the diff summary before you commit
to a sync.

It does **not** run destructive operations for you by default. Sync, scrub, and fix stay opt-in
and explicit.

## Features

- `snapglass status` — one-screen summary: disks, parity drives, scrub freshness, drift.
- `snapglass diff` — parsed `snapraid diff`: added / removed / updated / moved / copied counts,
  with the danger flag if removals exceed your configured threshold.
- `snapglass tui` — live TUI: data-disk table metrics, array scrub age, parity status, pending diff, with
  keybindings to trigger a sync/scrub (with confirmation).
- Reads `/etc/snapraid.conf` to discover data disks, parity, and content files — no duplicate config.
- Array-wide scrub-age tracking: a gauge for the unscrubbed percentage plus an
  oldest / median / newest age band, each colored against your chosen freshness
  window, with a FRESH / STALE / NEVER SCRUBBED chip. (Scrub age is array-wide,
  not a per-disk heatmap.)

## Non-goals

- Not a replacement for the `snapraid` binary — it wraps and reads it, nothing more.
- No scheduling/cron logic (use systemd timers; see below).
- No multi-host awareness. One array, one box.

## Requirements

- `snapraid` installed and on `PATH`.
- Read access to `/etc/snapraid.conf` (or pass `--config`).
- Rust toolchain to build (stable).

## Install

```sh
git clone <your-remote>/snapglass
cd snapglass
cargo install --path .
```

## Usage

```sh
snapglass status                 # one-shot summary
snapglass status --config /etc/snapraid.conf
snapglass diff                   # parsed diff, exit code reflects drift severity
snapglass diff --threshold 100   # warn if >100 files removed/changed
snapglass tui                    # interactive dashboard
```

### Exit codes (for scripting)

| Code | Meaning                                      |
|------|----------------------------------------------|
| 0    | Array in sync, nothing notable               |
| 1    | Drift present (sync recommended)             |
| 2    | Drift exceeds threshold (review before sync) |
| 10   | Could not read config / run snapraid         |

This makes `snapglass diff` usable as a guard in a systemd timer that emails you only when
something actually needs attention.

## TUI keys

All three panes (disks, parity/scrub, diff) are shown at once; `tab` moves the
focus (bright border) between them rather than swapping what's visible.

| Key       | Action                                          |
|-----------|-------------------------------------------------|
| `s`       | Run `snapraid sync` (modal confirm)             |
| `c`       | Run `snapraid scrub` (modal confirm)            |
| `d`       | Refresh diff                                    |
| `r`       | Refresh all status                              |
| `tab`     | Move focus between panes (disks / parity / diff)|
| `↑` / `↓` | Move the disk-table cursor (Disks pane focused) |
| `q`       | Quit                                            |

`snapglass tui --threshold N` sets the risky-change threshold that turns the diff
pane and health banner red (default 200, matching `snapglass diff`).

## Suggested systemd timer

snapglass doesn't schedule anything itself. Pair it with a timer that runs a guarded sync:

```ini
# /etc/systemd/system/snapraid-sync.service
[Service]
Type=oneshot
ExecStart=/usr/bin/env sh -c 'snapglass diff --threshold 200 || exit 0; snapraid sync && snapraid scrub -p 8'
```

(Tune the threshold and scrub percentage to taste.)

## Layout

```
snapglass/
├─ src/
│  ├─ main.rs        # clap CLI dispatch
│  ├─ config.rs      # parse /etc/snapraid.conf
│  ├─ snapraid.rs    # invoke + parse status/diff output
│  ├─ model.rs       # ArrayState, DiskState, DiffSummary
│  └─ tui/           # ratatui dashboard
└─ README.md
```

## Status

Personal project, built for one machine. No stability guarantees, no support promised. Use at
your own risk — especially around any command that triggers `sync`/`scrub`/`fix`.

## License

Personal use. (Add a license file if you ever decide to share it.)
