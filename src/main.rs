mod app;
mod cli;
mod config;
mod model;
mod storage;
mod ui;

use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::Local;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

use app::App;
use config::{Config, Keymap, Theme};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => run_tui(),
        Some("export") => export(&args[1..]),
        Some("notify") => notify(&args[1..]),
        Some("-h" | "--help" | "help") => {
            print!("{}", cli::USAGE);
            Ok(())
        }
        Some("-V" | "--version") => {
            println!("tui-do-list {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(other) => bail!("unknown command {other:?}\n\n{}", cli::USAGE),
    }
}

fn load_config() -> Result<Config> {
    let path = config::config_path()?;
    config::load(&path)
}

fn run_tui() -> Result<()> {
    let config = load_config()?;
    let keymap = Keymap::from_config(&config.keys).context("config.toml")?;
    let theme = Theme::from_config(&config.colors).context("config.toml")?;
    // One interactive instance at a time, so saves cannot clobber each other.
    let paths = storage::list_paths(&config)?;
    let _lock = storage::acquire_lock(&paths[0].1)?;
    let lists = storage::load_lists(&config)?;

    let mut app = App::with_lists(lists, Local::now().date_naive());
    app.keymap = keymap;
    app.theme = theme;
    app.sorted = config.sort_by_priority;
    app.confirm_delete = config.confirm_delete;
    app.lists_mutable = std::env::var_os(storage::ENV_OVERRIDE).is_none();

    let result = ratatui::run(|terminal| -> Result<()> {
        while !app.quit {
            terminal.draw(|frame| ui::render(frame, &mut app))?;
            let mut dirty = false;
            if event::poll(Duration::from_millis(250))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                dirty |= app.handle_key(key);
            }
            if let Some(name) = app.take_pending_new_list() {
                match create_list(&name) {
                    Ok((path, store)) => app.add_list(name, path, store),
                    Err(e) => app.status = Some(format!("Could not create list: {e:#}")),
                }
            }
            let now = Local::now().naive_local();
            app.tick(now.time());
            dirty |= app.check_date_rollover(now.date());
            if dirty {
                storage::save(&app.path, &app.store)?;
            }
        }
        Ok(())
    });

    // Always persist every list on the way out, even if the loop failed,
    // then surface the error.
    app.store.prune();
    for (path, store) in app.all_lists() {
        storage::save(path, store).with_context(|| format!("saving {}", path.display()))?;
    }
    result
}

/// Creates the data file for a new list and records it in config.toml
/// (preserving comments). An orphaned `<name>.json` already on disk is
/// adopted; a corrupt one is an error, so it is never overwritten.
fn create_list(name: &str) -> Result<(std::path::PathBuf, model::Store)> {
    let path = storage::data_path_for(name)?;
    let store = storage::load(&path)?;
    config::add_list(&config::config_path()?, name)?;
    storage::save(&path, &store)?; // materialize the file right away
    Ok((path, store))
}

/// Every list's `(name, store)` for the non-interactive commands, with
/// carry-over applied in memory (never saved) so `export` and `notify` see
/// the same "today" the TUI would show.
fn load_all() -> Result<(Config, Vec<(String, model::Store)>)> {
    let config = load_config()?;
    let today = Local::now().date_naive();
    let mut lists: Vec<(String, model::Store)> = storage::load_lists(&config)?
        .into_iter()
        .map(|l| (l.name, l.store))
        .collect();
    for (_, store) in &mut lists {
        store.carry_over(today);
    }
    Ok((config, lists))
}

fn export(args: &[String]) -> Result<()> {
    let today = Local::now().date_naive();
    let (range, show_hidden) = cli::parse_export_args(args, today)?;
    let (_, lists) = load_all()?;
    print!(
        "{}",
        cli::export_markdown(&lists, range, today, show_hidden)
    );
    Ok(())
}

fn notify(args: &[String]) -> Result<()> {
    let (config, lists) = load_all()?;
    let window = match args {
        [] => config.notify_window_secs,
        [flag, secs] if flag == "--window" => secs
            .parse()
            .with_context(|| format!("invalid --window value {secs:?}"))?,
        other => bail!("unexpected notify arguments: {}", other.join(" ")),
    };
    let now = Local::now().naive_local();
    for (list, task) in cli::due_reminders(&lists, now, window) {
        let time = task
            .remind
            .map(|t| t.format("%H:%M").to_string())
            .unwrap_or_default();
        // A hidden task's text and note never appear in a popup notification.
        let text = if task.hidden {
            "Hidden task"
        } else {
            &task.text
        };
        let summary = format!("⏰ {time} — {text}");
        let note = if task.hidden { &None } else { &task.note };
        let body = match (note, lists.len() > 1) {
            (Some(note), true) => format!("{note}\n[{list}]"),
            (Some(note), false) => note.clone(),
            (None, true) => format!("[{list}]"),
            (None, false) => String::new(),
        };
        cli::send_notification(&summary, &body)?;
        println!("notified: {summary}");
    }
    Ok(())
}
