# CLAUDE.md

## Project overview
- Single Rust 2024 binary crate (`tui-do-list`); entrypoint `src/main.rs`.
- A daily to-do list TUI modelled on a paper notebook: one page per day; unfinished tasks carry over to today automatically; past days keep only completed tasks as history. Built with ratatui 0.30.
- Module split: `model.rs` — `Task`/`Store` and all mutations: carry-over, recurrence, search, sort, stats (pure, no IO) · `storage.rs` — per-list data paths, load, atomic save, instance lock · `config.rs` — `config.toml`: lists, `Theme` (colours), `Keymap` (`Action` ↔ keys) · `app.rs` — `App` state, modes, key handling, undo, list switching · `ui.rs` — all rendering (reads `App`, never mutates the store) · `cli.rs` — the `export` and `notify` subcommands.
- Crossterm is used only via the `ratatui::crossterm` re-export — never add a separate `crossterm` dependency (version mismatch). The `time` crate exists only to talk to ratatui's calendar widget; `chrono` is the date library everywhere else.

## Commands
- `./build.sh` — fmt check → clippy `-D warnings` → tests → release build. Run it before declaring any work done; every step must pass. Also: `install`, `quick`, `clean`, `install-timer`.
- Run against scratch data, never the real data dir: `TUI_DO_LIST_FILE=/tmp/todo-dev.json cargo run`. For multi-list/config testing point `XDG_DATA_HOME`/`XDG_CONFIG_HOME` (or `TUI_DO_LIST_CONFIG`) at scratch dirs.
- Focused test: `cargo test <name>`, e.g. `cargo test carry_over` or `cargo test parse_time`.
- CLI smoke checks: `tui-do-list export [--day DATE|--week|--all] [--show-hidden]`, `tui-do-list notify [--window S]`, `--help`, `--version`.
- Manual TUI verification: drive the release binary inside tmux (`tmux -S target/tmux/s …`) and capture panes; the app is interactive, so plain stdout checks are not enough.

## Invariants — never break these
- Every mutation is persisted immediately with an atomic, durable write (process-unique temp file + fsync + rename + dir fsync). A corrupt data file must produce an error — never overwrite it.
- Carry-over moves only unfinished tasks from days **before** today to the front of today's list; future days are never touched; past days keep completed tasks.
- The interactive app takes an instance lock (`<list>.lock`, pid-based, stale-safe); `export` and `notify` are read-only and must never write data files. They apply carry-over **in memory only** so their output matches the TUI.
- Hidden tasks (`hidden: true`) are screen privacy, **not encryption**: while masked, their text and notes must not appear in any UI surface, status message, search result, export (without `--show-hidden`), or desktop notification — but the JSON stores plain text; never claim otherwise in docs.
- Quitting is `:q` (command mode) or `Ctrl-C`; plain `q`/`Esc` must not quit from the task list (popups still close with them). All list keys are user-configurable via `[keys]` — derive key hints from the keymap (`first_key`), don't hardcode letters (the `:q` footer hint is the one exception).
- User text passed to external commands (`notify-send`) must stay behind a `--` separator.
- The data format is versioned JSON (`version: 1`). Files written by older versions must keep loading: every new `Task`/`Store` field needs a serde default (and `skip_serializing_if` when optional).
- Modal states that hold a task index (confirm-delete, edit/reminder/note editors, calendar-move) must be cancelled on midnight rollover — carry-over invalidates raw indices.
- In sorted view (`s`), `selected` indexes the *visible* order; always map through `visible()`/`selected_task()` before mutating, and re-select the task afterwards.
- `ui.rs` only reads `App`; all mutations live in `model.rs`/`app.rs` and stay unit-tested. Keep `model.rs`, `config.rs`, `storage.rs`, `cli.rs` free of terminal code.

## Conventions
- Add a regression test with every bug fix; model/config/cli/storage tests are plain unit tests (no terminal needed).
- Colours come from `Theme` (10 named slots), never raw `Color::*` in `ui.rs` for anything a user might want to restyle.
- Docs live in `README.md` (user-facing) and `config.example.toml` (every setting, commented); update both when behaviour or keys change.

## Release / package notes
- The release workflow (`.github/workflows/release.yml`) requires a semver tag `vX.Y.Z` that matches the `version` in `Cargo.toml`; the run fails on a mismatch.
- AUR packaging lives in `packaging/aur/`; keep `pkgver` in both `PKGBUILD` (source build) and `PKGBUILD-bin` (release-tarball binary) aligned with `Cargo.toml` when bumping versions, and regenerate `.SRCINFO` (`makepkg --printsrcinfo > .SRCINFO`) before pushing to the AUR.
- The source AUR build disables LTO and debug packages (`options=(!lto !debug)`) because Arch's makepkg defaults can otherwise cause link errors and debug-package extraction issues with Rust builds.
- Release builds x86_64 natively with cargo and aarch64 with `cross`; native release tests only run for x86_64 in CI (the aarch64 binary is cross-compiled, never executed on the runner).
