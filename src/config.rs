//! Optional `config.toml`: lists, colours, key bindings and behaviour flags.
//! Everything has a default, so the file may be absent or partial.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, anyhow, bail};
use directories::ProjectDirs;
use ratatui::crossterm::event::KeyCode;
use ratatui::style::Color;
use serde::Deserialize;

pub const ENV_CONFIG: &str = "TUI_DO_LIST_CONFIG";
pub const DEFAULT_LIST: &str = "tasks";

/// Raw file contents. See `config.example.toml` for the documented shape.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Named lists; each is stored as `<name>.json`. `Tab` cycles through them.
    pub lists: Vec<String>,
    /// Start with the priority/overdue sort switched on (`s` toggles it).
    pub sort_by_priority: bool,
    /// Ask `y/n` before deleting a task.
    pub confirm_delete: bool,
    /// Seconds before "now" that `tui-do-list notify` looks back for reminders.
    pub notify_window_secs: u64,
    pub colors: HashMap<String, String>,
    pub keys: HashMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            lists: vec![DEFAULT_LIST.to_string()],
            sort_by_priority: false,
            confirm_delete: false,
            notify_window_secs: 60,
            colors: HashMap::new(),
            keys: HashMap::new(),
        }
    }
}

pub fn config_path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os(ENV_CONFIG) {
        return Ok(PathBuf::from(p));
    }
    let dirs = ProjectDirs::from("", "", "tui-do-list")
        .context("could not determine a config directory for this user")?;
    Ok(dirs.config_dir().join("config.toml"))
}

/// A missing file is the default config; a malformed one is an error.
pub fn load(path: &Path) -> Result<Config> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let config: Config =
        toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
    if config.lists.is_empty() {
        bail!("{}: `lists` must contain at least one name", path.display());
    }
    for (i, name) in config.lists.iter().enumerate() {
        let prior: Vec<&str> = config.lists[..i].iter().map(String::as_str).collect();
        if let Some(err) = list_name_error(name, &prior) {
            bail!("{}: {err}", path.display());
        }
    }
    Ok(config)
}

/// Why `name` cannot be used as a list name (`None` when it can). A duplicate
/// would load the same file into two independent stores, and the save-on-exit
/// of the stale copy would clobber the fresh one.
pub fn list_name_error(name: &str, existing: &[&str]) -> Option<String> {
    if name.is_empty() || name.contains(['/', '\\']) {
        Some(format!("invalid list name {name:?}"))
    } else if existing.contains(&name) {
        Some(format!("duplicate list name {name:?}"))
    } else {
        None
    }
}

/// Appends `name` to `lists` in the config file, creating the file when
/// missing and preserving comments and unrelated settings. When the key is
/// absent the in-memory default is written out first, so the lists already in
/// use keep loading. A name that is already present is left alone.
pub fn add_list(path: &Path, name: &str) -> Result<()> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let mut doc: toml_edit::DocumentMut = raw
        .parse()
        .with_context(|| format!("parsing {}", path.display()))?;
    if doc.get("lists").is_none() {
        let defaults = toml_edit::Array::from_iter(Config::default().lists);
        doc.insert("lists", toml_edit::value(defaults));
    }
    let lists = doc["lists"]
        .as_array_mut()
        .with_context(|| format!("{}: `lists` is not an array", path.display()))?;
    if !lists.iter().any(|v| v.as_str() == Some(name)) {
        lists.push(name);
    }
    write_atomic(path, doc.to_string().as_bytes())
}

/// Same temp-file + fsync + rename discipline as the data files, so a crash
/// mid-write can never truncate the user's config.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let tmp = path.with_extension(format!("toml.tmp{}", std::process::id()));
    let mut file =
        std::fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("writing {}", tmp.display()))?;
    file.sync_all()
        .with_context(|| format!("syncing {}", tmp.display()))?;
    drop(file);
    std::fs::rename(&tmp, path)
        .with_context(|| format!("moving {} to {}", tmp.display(), path.display()))?;
    Ok(())
}

// --- colours -----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub high: Color,
    pub low: Color,
    pub today: Color,
    pub other_day: Color,
    pub reminder: Color,
    pub overdue: Color,
    pub accent: Color,
    pub age: Color,
    pub repeat: Color,
    pub done_day: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            high: Color::Red,
            low: Color::DarkGray,
            today: Color::Green,
            other_day: Color::Magenta,
            reminder: Color::Cyan,
            overdue: Color::Red,
            accent: Color::Yellow,
            age: Color::Yellow,
            repeat: Color::Blue,
            done_day: Color::Green,
        }
    }
}

impl Theme {
    pub const NAMES: &[&str] = &[
        "high",
        "low",
        "today",
        "other_day",
        "reminder",
        "overdue",
        "accent",
        "age",
        "repeat",
        "done_day",
    ];

    /// Applies `[colors]` overrides. Values are ratatui colour names
    /// (`red`, `light_blue`, `gray`…), `#rrggbb`, or an ANSI index.
    pub fn from_config(colors: &HashMap<String, String>) -> Result<Self> {
        let mut theme = Self::default();
        for (name, value) in colors {
            let color = Color::from_str(value)
                .map_err(|_| anyhow!("[colors] {name}: unknown colour {value:?}"))?;
            let slot = match name.as_str() {
                "high" => &mut theme.high,
                "low" => &mut theme.low,
                "today" => &mut theme.today,
                "other_day" => &mut theme.other_day,
                "reminder" => &mut theme.reminder,
                "overdue" => &mut theme.overdue,
                "accent" => &mut theme.accent,
                "age" => &mut theme.age,
                "repeat" => &mut theme.repeat,
                "done_day" => &mut theme.done_day,
                _ => bail!(
                    "[colors] unknown entry {name:?} (valid: {})",
                    Self::NAMES.join(", ")
                ),
            };
            *slot = color;
        }
        Ok(theme)
    }
}

// --- keys --------------------------------------------------------------------

/// Everything a key can do on the task list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Quit,
    Help,
    Calendar,
    Down,
    Up,
    First,
    Last,
    PrevDay,
    NextDay,
    Today,
    Add,
    Edit,
    Toggle,
    Delete,
    Undo,
    MoveDown,
    MoveUp,
    Priority,
    Reminder,
    Repeat,
    Note,
    Hide,
    Reveal,
    Command,
    Search,
    Review,
    Sort,
    Postpone,
    MoveToDate,
    NextList,
    NewList,
}

impl Action {
    /// `(action, config name, default keys, description)`.
    pub const TABLE: &[(Action, &str, &[KeyCode], &str)] = &[
        (
            Action::Down,
            "down",
            &[KeyCode::Char('j'), KeyCode::Down],
            "move selection down",
        ),
        (
            Action::Up,
            "up",
            &[KeyCode::Char('k'), KeyCode::Up],
            "move selection up",
        ),
        (Action::First, "first", &[KeyCode::Char('g')], "first task"),
        (Action::Last, "last", &[KeyCode::Char('G')], "last task"),
        (Action::Add, "add", &[KeyCode::Char('a')], "add a task"),
        (
            Action::Edit,
            "edit",
            &[KeyCode::Char('e')],
            "edit selected task",
        ),
        (
            Action::Toggle,
            "toggle",
            &[KeyCode::Char(' '), KeyCode::Char('x'), KeyCode::Enter],
            "toggle done",
        ),
        (
            Action::Delete,
            "delete",
            &[KeyCode::Char('d')],
            "delete task",
        ),
        (
            Action::Undo,
            "undo",
            &[KeyCode::Char('u')],
            "undo last delete",
        ),
        (
            Action::MoveDown,
            "move_down",
            &[KeyCode::Char('J')],
            "move task down",
        ),
        (
            Action::MoveUp,
            "move_up",
            &[KeyCode::Char('K')],
            "move task up",
        ),
        (
            Action::Priority,
            "priority",
            &[KeyCode::Char('p')],
            "cycle priority: none → low → high",
        ),
        (
            Action::Reminder,
            "reminder",
            &[KeyCode::Char('r')],
            "set a reminder time (empty clears it)",
        ),
        (
            Action::Repeat,
            "repeat",
            &[KeyCode::Char('R')],
            "cycle repeat: daily → weekdays → weekly → monthly",
        ),
        (
            Action::Note,
            "note",
            &[KeyCode::Char('n')],
            "edit the task's note",
        ),
        (
            Action::Hide,
            "hide",
            &[KeyCode::Char('H')],
            "hide/unhide the task (mask its text)",
        ),
        (
            Action::Reveal,
            "reveal",
            &[KeyCode::Char('v')],
            "reveal hidden tasks for this session",
        ),
        (
            Action::Postpone,
            "postpone",
            &[KeyCode::Char('>')],
            "postpone to the next day",
        ),
        (
            Action::MoveToDate,
            "move_to_date",
            &[KeyCode::Char('m')],
            "move to a date (pick in calendar)",
        ),
        (
            Action::Sort,
            "sort",
            &[KeyCode::Char('s')],
            "toggle priority sort",
        ),
        (
            Action::Search,
            "search",
            &[KeyCode::Char('/')],
            "search all days",
        ),
        (
            Action::Review,
            "review",
            &[KeyCode::Char('w')],
            "weekly review & stats",
        ),
        (
            Action::PrevDay,
            "prev_day",
            &[KeyCode::Char('h'), KeyCode::Left],
            "previous day",
        ),
        (
            Action::NextDay,
            "next_day",
            &[KeyCode::Char('l'), KeyCode::Right],
            "next day",
        ),
        (
            Action::Today,
            "today",
            &[KeyCode::Char('t')],
            "jump to today",
        ),
        (
            Action::Calendar,
            "calendar",
            &[KeyCode::Char('c')],
            "open the calendar",
        ),
        (
            Action::NextList,
            "next_list",
            &[KeyCode::Tab],
            "switch to the next list",
        ),
        (
            Action::NewList,
            "new_list",
            &[KeyCode::Char('N')],
            "create a new list (also :newlist NAME)",
        ),
        (
            Action::Help,
            "help",
            &[KeyCode::Char('?')],
            "toggle this help",
        ),
        (
            Action::Command,
            "command",
            &[KeyCode::Char(':')],
            "command line — :q quit, :w save",
        ),
        (
            Action::Quit,
            "quit",
            &[],
            "quit directly (unbound by default — use :q, or Ctrl-C)",
        ),
    ];
}

/// Key → action lookup for the task list.
#[derive(Debug, Clone)]
pub struct Keymap {
    map: HashMap<KeyCode, Action>,
    /// Bindings in declaration order, so hints show the primary key first.
    bindings: Vec<(Action, Vec<KeyCode>)>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::from_config(&HashMap::new()).expect("default keymap is valid")
    }
}

impl Keymap {
    /// Builds the map from defaults, replacing an action's keys when `[keys]`
    /// names it. Values are space-separated key names (`"space x enter"`).
    pub fn from_config(keys: &HashMap<String, String>) -> Result<Self> {
        let mut map: HashMap<KeyCode, Action> = HashMap::new();
        let mut bindings = Vec::new();
        for (action, name, defaults, _) in Action::TABLE {
            let codes: Vec<KeyCode> = match keys.get(*name) {
                Some(spec) => spec
                    .split_whitespace()
                    .map(parse_key)
                    .collect::<Result<_>>()
                    .with_context(|| format!("[keys] {name}"))?,
                None => defaults.to_vec(),
            };
            // An empty value unbinds the action (Quit ships unbound: use :q).
            bindings.push((*action, codes.clone()));
            for code in codes {
                if let Some(other) = map.insert(code, *action) {
                    let other_name = Action::TABLE
                        .iter()
                        .find(|(a, ..)| a == &other)
                        .map(|(_, n, ..)| *n)
                        .unwrap_or("?");
                    bail!(
                        "[keys] {name}: {} is already bound to {other_name}",
                        key_name(code)
                    );
                }
            }
        }
        for name in keys.keys() {
            if !Action::TABLE.iter().any(|(_, n, ..)| n == name) {
                bail!("[keys] unknown action {name:?}");
            }
        }
        Ok(Self { map, bindings })
    }

    pub fn action(&self, code: KeyCode) -> Option<Action> {
        self.map.get(&code).copied()
    }

    /// Keys bound to `action`, for the help screen and footer.
    pub fn keys_for(&self, action: Action) -> String {
        self.bindings
            .iter()
            .find(|(a, _)| *a == action)
            .map(|(_, codes)| {
                codes
                    .iter()
                    .map(|k| key_name(*k))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default()
    }
}

/// Parses `a`, `A`, `space`, `enter`, `tab`, `esc`, `up`, `pgup`, `f5`, …
pub fn parse_key(spec: &str) -> Result<KeyCode> {
    let mut chars = spec.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Ok(KeyCode::Char(c));
    }
    Ok(match spec.to_ascii_lowercase().as_str() {
        "space" => KeyCode::Char(' '),
        "enter" | "return" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "esc" | "escape" => KeyCode::Esc,
        "backspace" => KeyCode::Backspace,
        "del" | "delete" => KeyCode::Delete,
        "ins" | "insert" => KeyCode::Insert,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pgup" | "pageup" => KeyCode::PageUp,
        "pgdn" | "pagedown" => KeyCode::PageDown,
        f if f.starts_with('f') && f[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)) => {
            KeyCode::F(f[1..].parse().unwrap())
        }
        _ => bail!("unknown key {spec:?}"),
    })
}

pub fn key_name(code: KeyCode) -> String {
    match code {
        KeyCode::Char(' ') => "space".into(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "enter".into(),
        KeyCode::Tab => "tab".into(),
        KeyCode::BackTab => "backtab".into(),
        KeyCode::Esc => "esc".into(),
        KeyCode::Backspace => "backspace".into(),
        KeyCode::Delete => "del".into(),
        KeyCode::Insert => "ins".into(),
        KeyCode::Up => "↑".into(),
        KeyCode::Down => "↓".into(),
        KeyCode::Left => "←".into(),
        KeyCode::Right => "→".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        KeyCode::PageUp => "pgup".into(),
        KeyCode::PageDown => "pgdn".into(),
        KeyCode::F(n) => format!("f{n}"),
        other => format!("{other:?}").to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn default_keymap_binds_every_action_without_conflicts() {
        let km = Keymap::default();
        for (action, ..) in Action::TABLE {
            if *action != Action::Quit {
                assert!(!km.keys_for(*action).is_empty(), "{action:?} has no key");
            }
        }
        assert_eq!(km.action(KeyCode::Char(':')), Some(Action::Command));
        assert_eq!(
            km.action(KeyCode::Char('q')),
            None,
            "q no longer quits directly"
        );
        assert_eq!(km.action(KeyCode::Esc), None);
        assert_eq!(km.action(KeyCode::Char('a')), Some(Action::Add));
        assert_eq!(km.action(KeyCode::Tab), Some(Action::NextList));
        assert_eq!(km.action(KeyCode::Char('N')), Some(Action::NewList));
        assert_eq!(km.action(KeyCode::F(1)), None);
    }

    #[test]
    fn keymap_overrides_and_rejects_conflicts() {
        let km = Keymap::from_config(&map(&[("add", "i ins"), ("quit", "Q")])).unwrap();
        assert_eq!(km.action(KeyCode::Char('i')), Some(Action::Add));
        assert_eq!(km.action(KeyCode::Insert), Some(Action::Add));
        assert_eq!(km.action(KeyCode::Char('a')), None);
        assert_eq!(km.action(KeyCode::Char('Q')), Some(Action::Quit));
        assert_eq!(km.action(KeyCode::Char('q')), None);

        let err = Keymap::from_config(&map(&[("add", "d")])).unwrap_err();
        assert!(err.to_string().contains("already bound to"), "{err}");
        assert!(Keymap::from_config(&map(&[("fly", "f")])).is_err());
        let km = Keymap::from_config(&map(&[("add", "")])).unwrap();
        assert_eq!(km.action(KeyCode::Char('a')), None, "empty value unbinds");
        assert!(Keymap::from_config(&map(&[("add", "ctrl+x")])).is_err());
    }

    #[test]
    fn key_names_round_trip() {
        for spec in [
            "a", "Z", "/", "space", "enter", "tab", "esc", "pgup", "f5", "home",
        ] {
            let code = parse_key(spec).unwrap();
            assert_eq!(parse_key(&key_name(code)).unwrap(), code, "{spec}");
        }
        assert_eq!(parse_key("up").unwrap(), KeyCode::Up);
        assert!(parse_key("f13").is_err());
    }

    #[test]
    fn theme_overrides_and_validates() {
        let theme =
            Theme::from_config(&map(&[("high", "#ff8800"), ("today", "light_blue")])).unwrap();
        assert_eq!(theme.high, Color::Rgb(255, 136, 0));
        assert_eq!(theme.today, Color::LightBlue);
        assert_eq!(theme.low, Theme::default().low);
        assert!(Theme::from_config(&map(&[("high", "not-a-colour")])).is_err());
        assert!(Theme::from_config(&map(&[("border", "red")])).is_err());
    }

    #[test]
    fn add_list_creates_updates_and_preserves_the_file() {
        let dir = std::env::temp_dir().join(format!("tui-do-list-addlist-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        // Missing file: created, with the implicit default list materialized.
        add_list(&path, "work").unwrap();
        assert_eq!(load(&path).unwrap().lists, [DEFAULT_LIST, "work"]);

        // Comments and unrelated settings survive; sections stay valid TOML.
        std::fs::write(
            &path,
            "# my config\nlists = [\"personal\"]\nconfirm_delete = true\n\n[colors]\nhigh = \"red\"\n",
        )
        .unwrap();
        add_list(&path, "work").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("# my config"), "{raw}");
        assert!(raw.contains("confirm_delete = true"), "{raw}");
        let cfg = load(&path).unwrap();
        assert_eq!(cfg.lists, ["personal", "work"]);
        assert!(cfg.confirm_delete);
        assert_eq!(cfg.colors["high"], "red");

        // Adding a name that is already present changes nothing.
        add_list(&path, "work").unwrap();
        assert_eq!(load(&path).unwrap().lists, ["personal", "work"]);

        // A `lists` that is not an array is an error, not an overwrite.
        std::fs::write(&path, "lists = \"oops\"\n").unwrap();
        assert!(add_list(&path, "work").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_name_rules() {
        assert!(list_name_error("work", &["tasks"]).is_none());
        assert!(list_name_error("", &[]).unwrap().contains("invalid"));
        assert!(list_name_error("a/b", &[]).unwrap().contains("invalid"));
        assert!(list_name_error("a\\b", &[]).unwrap().contains("invalid"));
        assert!(
            list_name_error("tasks", &["tasks"])
                .unwrap()
                .contains("duplicate")
        );
    }

    #[test]
    fn config_parses_and_validates() {
        let cfg: Config = toml::from_str(
            r#"
            lists = ["personal", "work"]
            confirm_delete = true
            [colors]
            high = "red"
            [keys]
            add = "i"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.lists, ["personal", "work"]);
        assert!(cfg.confirm_delete);
        assert!(!cfg.sort_by_priority);
        assert_eq!(cfg.keys["add"], "i");
        assert!(toml::from_str::<Config>("nope = 1").is_err());

        let dir = std::env::temp_dir().join(format!("tui-do-list-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            load(&dir.join("missing.toml")).unwrap().lists,
            [DEFAULT_LIST]
        );
        let bad = dir.join("bad.toml");
        std::fs::write(&bad, "lists = []").unwrap();
        assert!(load(&bad).is_err());
        std::fs::write(&bad, "lists = [\"a/b\"]").unwrap();
        assert!(load(&bad).is_err());
        std::fs::write(&bad, "lists = [\"a\", \"b\", \"a\"]").unwrap();
        let err = load(&bad).unwrap_err().to_string();
        assert!(err.contains("duplicate list name"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
