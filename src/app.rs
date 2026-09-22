//! Application state and key handling. Every key press is turned into a
//! mutation on the active `Store`; the caller decides whether to persist.

use std::mem;
use std::path::{Path, PathBuf};

use chrono::{Days, Months, NaiveDate, NaiveTime};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::{Action, Keymap, Theme};
use crate::model::{Store, Task, parse_time};

/// What the text being typed is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    NewTask,
    EditTask(usize),
    Reminder(usize),
    Note(usize),
    Search,
    Command,
    NewList,
}

/// A minimal single-line editor with a character cursor.
#[derive(Debug)]
pub struct Editor {
    pub buf: Vec<char>,
    pub cursor: usize,
    pub target: Target,
}

impl Editor {
    fn new(text: &str, target: Target) -> Self {
        let buf: Vec<char> = text.chars().collect();
        Self {
            cursor: buf.len(),
            buf,
            target,
        }
    }

    pub fn text(&self) -> String {
        self.buf.iter().collect()
    }

    fn handle(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.buf.insert(self.cursor, c);
                self.cursor += 1;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.buf.remove(self.cursor);
            }
            KeyCode::Delete if self.cursor < self.buf.len() => {
                self.buf.remove(self.cursor);
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.buf.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.buf.len(),
            _ => {}
        }
    }
}

#[derive(Debug)]
pub enum Mode {
    Normal,
    Insert(Editor),
    Help {
        scroll: u16,
    },
    /// Month view; `cursor` is the day under the cursor. With `moving`, Enter
    /// moves that task (index in the current day) instead of jumping.
    Calendar {
        cursor: NaiveDate,
        moving: Option<usize>,
    },
    /// Live search across all days; `selected` indexes the result list.
    Search {
        editor: Editor,
        selected: usize,
    },
    Review {
        scroll: u16,
    },
    /// Vim-style command line opened with `:` — `:q` quits, `:w` saves.
    Command(Editor),
    ConfirmDelete(usize),
    /// List picker; `selected` indexes `list_names()` order, with one extra
    /// row after the last list for "+ New list".
    Lists {
        selected: usize,
    },
}

/// One named list with its own data file.
#[derive(Debug)]
pub struct ListSlot {
    pub name: String,
    pub path: PathBuf,
    pub store: Store,
}

pub const MAX_SEARCH_RESULTS: usize = 50;

pub struct App {
    /// The active list's tasks.
    pub store: Store,
    pub list_name: String,
    pub path: PathBuf,
    /// Inactive lists, in the order `Tab` cycles through them.
    others: Vec<ListSlot>,
    /// The day currently being viewed.
    pub date: NaiveDate,
    /// The real calendar date; carry-over targets this.
    pub today: NaiveDate,
    /// Current wall-clock time, refreshed every tick; drives reminder state.
    pub now: NaiveTime,
    /// Position in the *visible* order (see `visible`).
    pub selected: usize,
    pub mode: Mode,
    undo: Vec<(NaiveDate, usize, Task)>,
    pub status: Option<String>,
    pub quit: bool,
    /// When true, hidden tasks show their real text (session-only).
    pub reveal: bool,
    /// Show overdue/high-priority first instead of manual order.
    pub sorted: bool,
    /// Focus view: completed tasks are left out of the day view.
    pub hide_done: bool,
    /// Show the next list in a read-only pane beside the active one.
    pub split: bool,
    /// Carry-over changed an inactive list; the main loop saves them.
    others_dirty: bool,
    pub confirm_delete: bool,
    pub keymap: Keymap,
    pub theme: Theme,
    /// False when `$TUI_DO_LIST_FILE` pins a single file: the list set then
    /// comes from the environment, not config.toml, so `:newlist` is refused.
    pub lists_mutable: bool,
    /// A list the user asked to create; the main loop performs the IO
    /// (config.toml + data file) and calls `add_list`.
    pending_new_list: Option<String>,
}

impl App {
    /// Single unnamed list, for tests.
    #[cfg(test)]
    pub fn new(store: Store, today: NaiveDate) -> Self {
        Self::with_lists(
            vec![ListSlot {
                name: crate::config::DEFAULT_LIST.to_string(),
                path: PathBuf::new(),
                store,
            }],
            today,
        )
    }

    /// The first list becomes active. `lists` must not be empty.
    pub fn with_lists(mut lists: Vec<ListSlot>, today: NaiveDate) -> Self {
        assert!(!lists.is_empty(), "at least one list is required");
        let active = lists.remove(0);
        let mut store = active.store;
        let carried = store.carry_over(today);
        let status = (carried > 0).then(|| format!("Carried over {carried} unfinished task(s)"));
        // Inactive lists are carried over too, so the side pane and the list
        // picker's counts show the same "today" the list would on activation.
        let mut others_dirty = false;
        for l in &mut lists {
            others_dirty |= l.store.carry_over(today) > 0;
        }
        Self {
            store,
            list_name: active.name,
            path: active.path,
            others: lists,
            date: today,
            today,
            now: NaiveTime::MIN,
            selected: 0,
            mode: Mode::Normal,
            undo: Vec::new(),
            status,
            quit: false,
            reveal: false,
            sorted: false,
            hide_done: false,
            split: false,
            others_dirty,
            confirm_delete: false,
            keymap: Keymap::default(),
            theme: Theme::default(),
            lists_mutable: true,
            pending_new_list: None,
        }
    }

    /// Every list's `(path, store)`, active first — for saving on exit.
    pub fn all_lists(&self) -> Vec<(&Path, &Store)> {
        let mut all = vec![(self.path.as_path(), &self.store)];
        all.extend(self.others.iter().map(|l| (l.path.as_path(), &l.store)));
        all
    }

    /// List names in cycling order, active first.
    pub fn list_names(&self) -> Vec<&str> {
        let mut names = vec![self.list_name.as_str()];
        names.extend(self.others.iter().map(|l| l.name.as_str()));
        names
    }

    pub fn len(&self) -> usize {
        self.store.tasks(self.date).len()
    }

    /// Indices into `store.tasks(date)` in display order.
    pub fn visible(&self) -> Vec<usize> {
        let mut order = if self.sorted {
            self.store.sorted_indices(self.date, self.today, self.now)
        } else {
            (0..self.len()).collect()
        };
        if self.hide_done {
            let tasks = self.store.tasks(self.date);
            order.retain(|&i| !tasks[i].done);
        }
        order
    }

    /// Number of rows on screen (`len()` minus hidden completed tasks).
    pub fn visible_len(&self) -> usize {
        self.visible().len()
    }

    /// True once when carry-over changed an inactive list since the last call.
    pub fn take_others_dirty(&mut self) -> bool {
        mem::take(&mut self.others_dirty)
    }

    /// The list shown in the side pane: the next one in `Tab` order, when the
    /// split view is on and there is more than one list.
    pub fn side_list(&self) -> Option<(&str, &Store)> {
        self.others
            .first()
            .filter(|_| self.split)
            .map(|l| (l.name.as_str(), &l.store))
    }

    /// `(name, open tasks, is active)` for every list, in `list_names()` order.
    pub fn list_rows(&self) -> Vec<(&str, usize, bool)> {
        let mut rows = vec![(
            self.list_name.as_str(),
            self.store.open_tasks(self.today).len(),
            true,
        )];
        rows.extend(
            self.others
                .iter()
                .map(|l| (l.name.as_str(), l.store.open_tasks(self.today).len(), false)),
        );
        rows
    }

    /// The real index of the selected task, if any.
    pub fn selected_task(&self) -> Option<usize> {
        self.visible().get(self.selected).copied()
    }

    /// The task's text, masked when it is hidden and reveal mode is off.
    pub fn display_text(&self, task: &Task) -> String {
        if task.hidden && !self.reveal {
            crate::model::MASK.to_string()
        } else {
            task.text.clone()
        }
    }

    fn is_masked(&self, idx: usize) -> bool {
        self.store.tasks(self.date)[idx].hidden && !self.reveal
    }

    fn clamp_selection(&mut self) {
        self.selected = self.selected.min(self.visible_len().saturating_sub(1));
    }

    /// Keeps the task at real index `idx` selected after the order changed.
    /// A task that just left the view (completed while `hide_done` is on)
    /// leaves the cursor where it was, clamped to the rows that remain.
    fn select_task(&mut self, idx: usize) {
        match self.visible().iter().position(|&i| i == idx) {
            Some(pos) => self.selected = pos,
            None => self.clamp_selection(),
        }
    }

    /// Search hits for the query being typed (empty outside search mode).
    pub fn search_results(&self) -> Vec<(NaiveDate, usize)> {
        match &self.mode {
            Mode::Search { editor, .. } => {
                let mut hits = self.store.search(&editor.text());
                // While masked, a hidden task must not be discoverable by
                // typing guesses at its text.
                hits.retain(|&(date, idx)| self.reveal || !self.store.tasks(date)[idx].hidden);
                hits.truncate(MAX_SEARCH_RESULTS);
                hits
            }
            _ => Vec::new(),
        }
    }

    /// Advances the clock. Flashes a status message for any of today's
    /// reminders whose time was crossed since the previous tick.
    pub fn tick(&mut self, now: NaiveTime) {
        let prev = self.now;
        self.now = now;
        if now <= prev {
            return;
        }
        let reveal = self.reveal;
        let due: Vec<&str> = self
            .store
            .tasks(self.today)
            .iter()
            .filter(|t| !t.done && t.remind.is_some_and(|r| prev < r && r <= now))
            .map(|t| {
                if t.hidden && !reveal {
                    crate::model::MASK
                } else {
                    t.text.as_str()
                }
            })
            .collect();
        if !due.is_empty() {
            self.status = Some(format!("⏰ Reminder: {}", due.join(", ")));
        }
    }

    /// Re-runs carry-over when the calendar date changes while the app is
    /// open. Returns true when the store changed.
    pub fn check_date_rollover(&mut self, now: NaiveDate) -> bool {
        if now == self.today {
            return false;
        }
        // A pending action that references a task by index would target the
        // wrong task once carry-over reshuffles the lists — cancel it.
        let holds_index = match &self.mode {
            Mode::ConfirmDelete(_)
            | Mode::Calendar {
                moving: Some(_), ..
            } => true,
            Mode::Insert(e) => matches!(
                e.target,
                Target::EditTask(_) | Target::Reminder(_) | Target::Note(_)
            ),
            _ => false,
        };
        if holds_index {
            self.mode = Mode::Normal;
            self.status = Some("New day — pending action cancelled".into());
        }
        let viewing_today = self.date == self.today;
        self.today = now;
        if viewing_today {
            self.date = now;
        }
        let carried = self.store.carry_over(now);
        for other in &mut self.others {
            self.others_dirty |= other.store.carry_over(now) > 0;
        }
        self.clamp_selection();
        if carried > 0 {
            self.status = Some(format!("New day: carried over {carried} task(s)"));
        }
        carried > 0
    }

    /// Handles one key press. Returns true when the active store was mutated.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        self.status = None;
        match self.mode {
            Mode::Normal => self.handle_normal(key),
            Mode::Insert(_) => self.handle_insert(key),
            Mode::Calendar { cursor, moving } => self.handle_calendar(key, cursor, moving),
            Mode::Search { .. } => self.handle_search(key),
            Mode::Command(_) => self.handle_command(key),
            Mode::ConfirmDelete(idx) => {
                self.mode = Mode::Normal;
                if matches!(key.code, KeyCode::Char('y' | 'Y') | KeyCode::Enter) {
                    return self.delete(idx);
                }
                self.status = Some("Cancelled".into());
                false
            }
            Mode::Help { .. } | Mode::Review { .. } => {
                self.handle_scrolling(key);
                false
            }
            Mode::Lists { selected } => self.handle_lists(key, selected),
        }
    }

    fn handle_scrolling(&mut self, key: KeyEvent) {
        let (Mode::Help { scroll } | Mode::Review { scroll }) = &mut self.mode else {
            return;
        };
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => *scroll += 1,
            KeyCode::Char('k') | KeyCode::Up => *scroll = scroll.saturating_sub(1),
            KeyCode::PageDown => *scroll += 10,
            KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
            KeyCode::Char('g') | KeyCode::Home => *scroll = 0,
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q' | '?' | 'w') => {
                self.mode = Mode::Normal;
            }
            _ => {}
        }
    }

    fn handle_normal(&mut self, key: KeyEvent) -> bool {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return false;
        }
        let Some(action) = self.keymap.action(key.code) else {
            return false;
        };
        let date = self.date;
        let task = self.selected_task();
        match (action, task) {
            (Action::Quit, _) => self.quit = true,
            (Action::Help, _) => self.mode = Mode::Help { scroll: 0 },
            (Action::Calendar, _) => {
                self.mode = Mode::Calendar {
                    cursor: date,
                    moving: None,
                }
            }
            (Action::Review, _) => self.mode = Mode::Review { scroll: 0 },
            (Action::Search, _) => {
                self.mode = Mode::Search {
                    editor: Editor::new("", Target::Search),
                    selected: 0,
                }
            }
            (Action::Command, _) => {
                self.mode = Mode::Command(Editor::new("", Target::Command));
            }

            (Action::Down, _) if self.selected + 1 < self.visible_len() => self.selected += 1,
            (Action::Up, _) => self.selected = self.selected.saturating_sub(1),
            (Action::First, _) => self.selected = 0,
            (Action::Last, _) => self.selected = self.visible_len().saturating_sub(1),

            (Action::PrevDay, _) => self.go_to(date - Days::new(1)),
            (Action::NextDay, _) => self.go_to(date + Days::new(1)),
            (Action::Today, _) => self.go_to(self.today),
            (Action::NextList, _) => return self.switch_list(),
            (Action::Lists, _) => self.mode = Mode::Lists { selected: 0 },
            (Action::NewList, _) => self.open_new_list_prompt(),
            (Action::Split, _) => {
                if self.others.is_empty() {
                    self.status = Some(format!(
                        "Only one list — press {} to create another",
                        self.first_key(Action::NewList)
                    ));
                } else {
                    self.split = !self.split;
                    self.status = Some(
                        if self.split {
                            "Split view: the next list is shown read-only on the right"
                        } else {
                            "Split view off"
                        }
                        .into(),
                    );
                }
            }
            (Action::HideDone, _) => {
                self.hide_done = !self.hide_done;
                match task {
                    Some(idx) => self.select_task(idx),
                    None => self.selected = 0,
                }
                self.status = Some(
                    if self.hide_done {
                        "Focus: completed tasks hidden"
                    } else {
                        "Showing completed tasks"
                    }
                    .into(),
                );
            }
            (Action::Sort, _) => {
                self.sorted = !self.sorted;
                match task {
                    Some(idx) => self.select_task(idx),
                    None => self.selected = 0,
                }
                self.status = Some(
                    if self.sorted {
                        "Sorted: overdue and high priority first"
                    } else {
                        "Manual order"
                    }
                    .into(),
                );
            }

            (Action::Add, _) => self.mode = Mode::Insert(Editor::new("", Target::NewTask)),
            (Action::Edit, Some(idx)) => {
                if self.is_masked(idx) {
                    self.status = Some(self.reveal_hint("edit"));
                } else {
                    let text = &self.store.tasks(date)[idx].text;
                    self.mode = Mode::Insert(Editor::new(text, Target::EditTask(idx)));
                }
            }
            (Action::Reminder, Some(idx)) => {
                let current = self.store.tasks(date)[idx]
                    .remind
                    .map(|t| t.format("%H:%M").to_string())
                    .unwrap_or_default();
                self.mode = Mode::Insert(Editor::new(&current, Target::Reminder(idx)));
            }
            (Action::Note, Some(idx)) => {
                if self.is_masked(idx) {
                    self.status = Some(self.reveal_hint("edit the note of"));
                } else {
                    let current = self.store.tasks(date)[idx].note.clone().unwrap_or_default();
                    self.mode = Mode::Insert(Editor::new(&current, Target::Note(idx)));
                }
            }
            (Action::Hide, Some(idx)) => {
                self.status = Some(if self.store.toggle_hidden(date, idx) {
                    format!(
                        "Task hidden — {} reveals hidden tasks",
                        self.first_key(Action::Reveal)
                    )
                } else {
                    "Task visible again".into()
                });
                return true;
            }
            (Action::Reveal, _) => {
                self.reveal = !self.reveal;
                self.status = Some(
                    if self.reveal {
                        "Hidden tasks revealed — toggle again to mask"
                    } else {
                        "Hidden tasks masked"
                    }
                    .into(),
                );
            }
            (Action::Toggle, Some(idx)) => {
                if let Some(next) = self.store.toggle(date, idx, self.today) {
                    self.status = Some(format!("Next occurrence: {}", next.format("%a %d %b")));
                }
                self.select_task(idx);
                return true;
            }
            (Action::Delete, Some(idx)) => {
                if self.confirm_delete {
                    self.mode = Mode::ConfirmDelete(idx);
                } else {
                    return self.delete(idx);
                }
            }
            (Action::Undo, _) => {
                if let Some((day, at, task)) = self.undo.pop() {
                    let at = self.store.insert(day, at, task);
                    self.go_to(day);
                    self.select_task(at);
                    self.status = Some("Restored".into());
                    return true;
                }
                self.status = Some("Nothing to undo".into());
            }
            (Action::MoveDown | Action::MoveUp, Some(idx)) => {
                if self.sorted {
                    let key = self.first_key(Action::Sort);
                    self.status = Some(format!("Turn off sorting ({key}) to reorder"));
                    return false;
                }
                self.selected = if action == Action::MoveDown {
                    self.store.move_down(date, idx)
                } else {
                    self.store.move_up(date, idx)
                };
                return true;
            }
            (Action::Priority, Some(idx)) => {
                self.store.cycle_priority(date, idx);
                self.select_task(idx);
                return true;
            }
            (Action::Repeat, Some(idx)) => {
                self.status = Some(match self.store.cycle_repeat(date, idx) {
                    Some(repeat) => format!("Repeats {}", repeat.label()),
                    None => "Does not repeat".into(),
                });
                return true;
            }
            (Action::Postpone, Some(idx)) => return self.move_to(idx, date + Days::new(1)),
            (Action::MoveToDate, Some(idx)) => {
                self.mode = Mode::Calendar {
                    cursor: date,
                    moving: Some(idx),
                }
            }
            _ => {}
        }
        false
    }

    fn reveal_hint(&self, verb: &str) -> String {
        format!(
            "Hidden task — press {} to reveal before you {verb} it",
            self.first_key(Action::Reveal)
        )
    }

    /// First key bound to `action`, for messages and hints.
    pub fn first_key(&self, action: Action) -> String {
        self.keymap
            .keys_for(action)
            .split_whitespace()
            .next()
            .unwrap_or("?")
            .to_string()
    }

    fn delete(&mut self, idx: usize) -> bool {
        let Some(task) = self.store.remove(self.date, idx) else {
            return false;
        };
        let shown = self.display_text(&task);
        self.status = Some(format!(
            "Deleted \"{shown}\" — press {} to undo",
            self.first_key(Action::Undo)
        ));
        self.undo.push((self.date, idx, task));
        self.store.prune();
        self.clamp_selection();
        true
    }

    fn move_to(&mut self, idx: usize, target: NaiveDate) -> bool {
        let Some(task) = self.store.tasks(self.date).get(idx) else {
            return false;
        };
        let text = self.display_text(task);
        if self.store.move_task(self.date, idx, target).is_none() {
            return false;
        }
        self.status = Some(format!("Moved \"{text}\" to {}", target.format("%a %d %b")));
        self.clamp_selection();
        true
    }

    fn switch_list(&mut self) -> bool {
        if self.others.is_empty() {
            self.status = Some(format!(
                "Only one list — press {} to create another",
                self.first_key(Action::NewList)
            ));
            return false;
        }
        let next = self.others.remove(0);
        let current = self.activate(next);
        self.others.push(current);
        true
    }

    /// Makes `next` the active list and returns the one it replaced. Applies
    /// carry-over to the newly active list and resets the view to today.
    fn activate(&mut self, next: ListSlot) -> ListSlot {
        let current = ListSlot {
            name: mem::replace(&mut self.list_name, next.name),
            path: mem::replace(&mut self.path, next.path),
            store: mem::replace(&mut self.store, next.store),
        };
        let carried = self.store.carry_over(self.today);
        self.undo.clear();
        self.date = self.today;
        self.selected = 0;
        self.status = Some(match carried {
            0 => format!("List: {}", self.list_name),
            n => format!("List: {} · carried over {n} task(s)", self.list_name),
        });
        current
    }

    /// Activates the list at `pos` in `list_names()` order. The lists keep
    /// their cyclic order, so `Tab` continues from the new position.
    fn switch_list_to(&mut self, pos: usize) -> bool {
        if pos == 0 || pos > self.others.len() {
            return false;
        }
        // Cycle before: active, o0 … ok … on. After choosing ok the cycle
        // reads ok, ok+1 … on, active, o0 … ok-1.
        let k = pos - 1;
        let next = self.others.remove(k);
        let previous = self.activate(next);
        self.others.rotate_left(k);
        let at = self.others.len() - k;
        self.others.insert(at, previous);
        true
    }

    /// List picker keys. Mutates the store only when a switch carries tasks.
    fn handle_lists(&mut self, key: KeyEvent, selected: usize) -> bool {
        let rows = self.others.len() + 2; // every list plus "+ New list"
        let next = match key.code {
            KeyCode::Char('j') | KeyCode::Down | KeyCode::Tab => (selected + 1).min(rows - 1),
            KeyCode::Char('k') | KeyCode::Up | KeyCode::BackTab => selected.saturating_sub(1),
            KeyCode::Char('g') | KeyCode::Home => 0,
            KeyCode::Char('G') | KeyCode::End => rows - 1,
            KeyCode::Enter => {
                self.mode = Mode::Normal;
                if selected + 1 == rows {
                    self.open_new_list_prompt();
                    return false;
                }
                return self.switch_list_to(selected);
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.mode = Mode::Normal;
                return false;
            }
            code => {
                match self.keymap.action(code) {
                    Some(Action::Lists) => self.mode = Mode::Normal,
                    Some(Action::NewList) => {
                        self.mode = Mode::Normal;
                        self.open_new_list_prompt();
                    }
                    _ => {}
                }
                return false;
            }
        };
        self.mode = Mode::Lists { selected: next };
        false
    }

    fn open_new_list_prompt(&mut self) {
        if !self.lists_mutable {
            self.status = Some(format!(
                "Lists are fixed while ${} is set",
                crate::storage::ENV_OVERRIDE
            ));
            return;
        }
        self.mode = Mode::Insert(Editor::new("", Target::NewList));
    }

    /// Validates `name` and asks the main loop to create the list.
    fn request_new_list(&mut self, name: &str) {
        if !self.lists_mutable {
            self.open_new_list_prompt(); // sets the explanatory status
            return;
        }
        if let Some(err) = crate::config::list_name_error(name, &self.list_names()) {
            self.status = Some(err);
            return;
        }
        self.pending_new_list = Some(name.to_string());
    }

    /// The list creation requested by the last key press, if any.
    pub fn take_pending_new_list(&mut self) -> Option<String> {
        self.pending_new_list.take()
    }

    /// Adds a just-created list and switches to it. `store` holds the content
    /// of its data file (usually empty; an orphaned file is adopted as-is).
    pub fn add_list(&mut self, name: String, path: PathBuf, store: Store) {
        let existed = !store.days.is_empty();
        self.others.insert(0, ListSlot { name, path, store });
        self.switch_list();
        self.status = Some(if existed {
            format!("Opened existing list \"{}\"", self.list_name)
        } else {
            format!("Created list \"{}\"", self.list_name)
        });
    }

    fn handle_insert(&mut self, key: KeyEvent) -> bool {
        let Mode::Insert(editor) = &mut self.mode else {
            return false;
        };
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                false
            }
            KeyCode::Enter => {
                let text = editor.text().trim().to_string();
                let target = editor.target;
                if let Target::Reminder(_) = target
                    && !text.is_empty()
                    && parse_time(&text).is_none()
                {
                    // Keep the editor open so the user can fix the time.
                    self.status = Some("Invalid time — use HH:MM (empty to clear)".into());
                    return false;
                }
                if target == Target::NewList
                    && !text.is_empty()
                    && let Some(err) = crate::config::list_name_error(&text, &self.list_names())
                {
                    // Keep the editor open so the user can fix the name.
                    self.status = Some(err);
                    return false;
                }
                self.mode = Mode::Normal;
                match target {
                    Target::Reminder(idx) => {
                        self.store.set_reminder(self.date, idx, parse_time(&text));
                        self.select_task(idx);
                    }
                    Target::Note(idx) => self.store.set_note(self.date, idx, &text),
                    Target::EditTask(idx) if !text.is_empty() => {
                        self.store.edit(self.date, idx, text);
                    }
                    Target::NewTask if !text.is_empty() => {
                        let idx = self.store.add(self.date, text);
                        self.select_task(idx);
                    }
                    Target::NewList if !text.is_empty() => {
                        self.pending_new_list = Some(text);
                        return false; // creating a list does not touch the store
                    }
                    _ => return false,
                }
                true
            }
            _ => {
                editor.handle(key);
                false
            }
        }
    }

    fn handle_command(&mut self, key: KeyEvent) -> bool {
        let Mode::Command(editor) = &mut self.mode else {
            return false;
        };
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                false
            }
            KeyCode::Enter => {
                let cmd = editor.text().trim().to_string();
                self.mode = Mode::Normal;
                self.run_command(&cmd)
            }
            _ => {
                editor.handle(key);
                false
            }
        }
    }

    /// Executes a `:` command. Returns true when the store should be saved.
    fn run_command(&mut self, cmd: &str) -> bool {
        let (head, arg) = match cmd.split_once(char::is_whitespace) {
            Some((head, rest)) => (head, rest.trim()),
            None => (cmd, ""),
        };
        match (head, arg) {
            ("", _) => false,
            ("q" | "q!" | "qa" | "quit" | "wq" | "x", "") => {
                self.quit = true;
                false // everything is already saved; main saves again on exit
            }
            ("w" | "write", "") => {
                self.status = Some("Saved".into());
                true // reported as dirty, so the main loop persists now
            }
            ("newlist", "") => {
                self.open_new_list_prompt();
                false
            }
            ("newlist", name) => {
                self.request_new_list(name);
                false
            }
            _ => {
                self.status = Some(format!("Not a command: :{cmd}"));
                false
            }
        }
    }

    fn handle_search(&mut self, key: KeyEvent) -> bool {
        let results = self.search_results();
        let Mode::Search { editor, selected } = &mut self.mode else {
            return false;
        };
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter => {
                let chosen = results.get(*selected).copied();
                self.mode = Mode::Normal;
                if let Some((date, idx)) = chosen {
                    self.go_to(date);
                    self.select_task(idx);
                }
            }
            KeyCode::Down | KeyCode::Tab => {
                if *selected + 1 < results.len() {
                    *selected += 1;
                }
            }
            KeyCode::Up | KeyCode::BackTab => *selected = selected.saturating_sub(1),
            _ => {
                editor.handle(key);
                *selected = 0;
            }
        }
        false
    }

    /// Calendar popup keys. Mutates the store only when moving a task.
    fn handle_calendar(&mut self, key: KeyEvent, cursor: NaiveDate, moving: Option<usize>) -> bool {
        let next = match key.code {
            KeyCode::Char('h') | KeyCode::Left => cursor - Days::new(1),
            KeyCode::Char('l') | KeyCode::Right => cursor + Days::new(1),
            KeyCode::Char('k') | KeyCode::Up => cursor - Days::new(7),
            KeyCode::Char('j') | KeyCode::Down => cursor + Days::new(7),
            KeyCode::Char('[') | KeyCode::PageUp => {
                cursor.checked_sub_months(Months::new(1)).unwrap_or(cursor)
            }
            KeyCode::Char(']') | KeyCode::PageDown => {
                cursor.checked_add_months(Months::new(1)).unwrap_or(cursor)
            }
            KeyCode::Char('t') => self.today,
            KeyCode::Enter => {
                self.mode = Mode::Normal;
                return match moving {
                    Some(idx) if cursor != self.date => self.move_to(idx, cursor),
                    Some(_) => false,
                    None => {
                        self.go_to(cursor);
                        false
                    }
                };
            }
            KeyCode::Esc | KeyCode::Char('c' | 'q' | 'm') => {
                self.mode = Mode::Normal;
                return false;
            }
            _ => cursor,
        };
        self.mode = Mode::Calendar {
            cursor: next,
            moving,
        };
        false
    }

    fn go_to(&mut self, date: NaiveDate) {
        self.date = date;
        self.selected = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Repeat;

    fn d(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 8, day).unwrap()
    }

    fn sep(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, day).unwrap()
    }

    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    fn press(app: &mut App, code: KeyCode) -> bool {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    fn add(app: &mut App, text: &str) {
        press(app, KeyCode::Char('a'));
        type_text(app, text);
        press(app, KeyCode::Enter);
    }

    fn texts(app: &App) -> Vec<String> {
        app.store
            .tasks(app.date)
            .iter()
            .map(|t| t.text.clone())
            .collect()
    }

    #[test]
    fn add_edit_toggle_delete_undo_flow() {
        let mut app = App::new(Store::default(), d(31));
        add(&mut app, "milk");
        add(&mut app, "bread");
        assert_eq!(texts(&app), ["milk", "bread"]);
        assert_eq!(app.selected, 1);

        // Edit with cursor movement: "bread" -> "rye bread".
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Home);
        type_text(&mut app, "rye ");
        press(&mut app, KeyCode::Enter);
        assert_eq!(texts(&app), ["milk", "rye bread"]);

        press(&mut app, KeyCode::Char('k'));
        assert!(press(&mut app, KeyCode::Char(' ')));
        assert!(app.store.tasks(d(31))[0].done);

        assert!(press(&mut app, KeyCode::Char('d')));
        assert_eq!(texts(&app), ["rye bread"]);
        assert!(press(&mut app, KeyCode::Char('u')));
        assert_eq!(texts(&app), ["milk", "rye bread"]);
        assert_eq!(app.selected, 0);
        assert!(!press(&mut app, KeyCode::Char('u')));
    }

    #[test]
    fn empty_or_cancelled_input_adds_nothing() {
        let mut app = App::new(Store::default(), d(31));
        press(&mut app, KeyCode::Char('a'));
        type_text(&mut app, "   ");
        assert!(!press(&mut app, KeyCode::Enter));
        press(&mut app, KeyCode::Char('a'));
        type_text(&mut app, "abc");
        assert!(!press(&mut app, KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.len(), 0);
    }

    #[test]
    fn day_navigation_and_selection_bounds() {
        let mut app = App::new(Store::default(), d(31));
        press(&mut app, KeyCode::Char('j')); // no tasks: stays at 0
        assert_eq!(app.selected, 0);
        press(&mut app, KeyCode::Char('h'));
        assert_eq!(app.date, d(30));
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.date, sep(1));
        press(&mut app, KeyCode::Char('t'));
        assert_eq!(app.date, d(31));
    }

    #[test]
    fn reminder_set_invalid_and_clear() {
        let mut app = App::new(Store::default(), d(31));
        add(&mut app, "dentist");

        press(&mut app, KeyCode::Char('r'));
        type_text(&mut app, "14h30");
        assert!(press(&mut app, KeyCode::Enter));
        assert_eq!(app.store.tasks(d(31))[0].remind, Some(t(14, 30)));

        // Reopening shows the current value; an invalid value keeps the editor open.
        press(&mut app, KeyCode::Char('r'));
        match &app.mode {
            Mode::Insert(e) => assert_eq!(e.text(), "14:30"),
            _ => panic!("editor should be open"),
        }
        type_text(&mut app, "x");
        assert!(!press(&mut app, KeyCode::Enter));
        assert!(matches!(app.mode, Mode::Insert(_)));
        assert!(app.status.as_deref().unwrap_or("").contains("Invalid"));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.store.tasks(d(31))[0].remind, Some(t(14, 30)));

        // Empty clears.
        press(&mut app, KeyCode::Char('r'));
        for _ in 0..5 {
            press(&mut app, KeyCode::Backspace);
        }
        assert!(press(&mut app, KeyCode::Enter));
        assert_eq!(app.store.tasks(d(31))[0].remind, None);
    }

    #[test]
    fn tick_flashes_reminders_that_become_due() {
        let mut app = App::new(Store::default(), d(31));
        app.store.add(d(31), "standup");
        app.store.set_reminder(d(31), 0, Some(t(10, 0)));
        app.store.add(d(31), "done already");
        app.store.set_reminder(d(31), 1, Some(t(10, 0)));
        app.store.toggle(d(31), 1, d(31));

        app.tick(t(9, 59));
        assert_eq!(app.status, None);
        app.tick(t(10, 0));
        assert_eq!(app.status.as_deref(), Some("⏰ Reminder: standup"));
        app.status = None;
        app.tick(t(10, 1));
        assert_eq!(app.status, None, "must not repeat");
    }

    #[test]
    fn calendar_navigates_and_jumps_to_the_chosen_day() {
        let mut app = App::new(Store::default(), d(31));
        let cursor = |app: &App| match app.mode {
            Mode::Calendar { cursor, .. } => cursor,
            _ => panic!("calendar should be open"),
        };
        press(&mut app, KeyCode::Char('c'));
        assert_eq!(cursor(&app), d(31));
        press(&mut app, KeyCode::Char('h'));
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(cursor(&app), d(23));
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(cursor(&app), d(31));
        press(&mut app, KeyCode::Char(']'));
        assert_eq!(cursor(&app), sep(30));
        press(&mut app, KeyCode::Char('['));
        assert_eq!(cursor(&app), d(30)); // 30 Sep -> 30 Aug
        press(&mut app, KeyCode::Char('t'));
        assert_eq!(cursor(&app), d(31));

        // Esc closes without moving; Enter jumps.
        press(&mut app, KeyCode::Char('h'));
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.date, d(31));
        press(&mut app, KeyCode::Char('c'));
        press(&mut app, KeyCode::Char('h'));
        assert!(!press(&mut app, KeyCode::Enter));
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.date, d(30));
    }

    #[test]
    fn postpone_and_move_to_date_via_calendar() {
        let mut app = App::new(Store::default(), d(31));
        add(&mut app, "a");
        add(&mut app, "b");
        press(&mut app, KeyCode::Char('g'));
        assert!(press(&mut app, KeyCode::Char('>')));
        assert_eq!(texts(&app), ["b"]);
        assert_eq!(app.store.tasks(sep(1))[0].text, "a");
        assert_eq!(app.status.as_deref(), Some("Moved \"a\" to Tue 01 Sep"));

        // `m` opens the calendar in move mode; Enter on another day moves.
        press(&mut app, KeyCode::Char('m'));
        assert!(matches!(
            app.mode,
            Mode::Calendar {
                moving: Some(0),
                ..
            }
        ));
        press(&mut app, KeyCode::Char('j'));
        assert!(press(&mut app, KeyCode::Enter));
        assert!(app.store.tasks(d(31)).is_empty());
        assert_eq!(app.store.tasks(sep(7))[0].text, "b");
        assert_eq!(app.date, d(31), "stays on the current day");

        // With no task selected the move keys are ignored.
        press(&mut app, KeyCode::Char('m'));
        assert!(matches!(app.mode, Mode::Normal));
        assert!(!press(&mut app, KeyCode::Char('>')));
    }

    #[test]
    fn notes_repeat_and_priority_statuses() {
        let mut app = App::new(Store::default(), d(31));
        add(&mut app, "gym");
        press(&mut app, KeyCode::Char('n'));
        type_text(&mut app, "bring towel");
        assert!(press(&mut app, KeyCode::Enter));
        assert_eq!(
            app.store.tasks(d(31))[0].note.as_deref(),
            Some("bring towel")
        );
        press(&mut app, KeyCode::Char('n'));
        match &app.mode {
            Mode::Insert(e) => assert_eq!(e.text(), "bring towel"),
            _ => panic!("editor should be open"),
        }
        press(&mut app, KeyCode::Esc);

        assert!(press(&mut app, KeyCode::Char('R')));
        assert_eq!(app.status.as_deref(), Some("Repeats daily"));
        assert_eq!(app.store.tasks(d(31))[0].repeat, Some(Repeat::Daily));
        assert!(press(&mut app, KeyCode::Char(' ')));
        assert_eq!(app.status.as_deref(), Some("Next occurrence: Tue 01 Sep"));
    }

    #[test]
    fn vim_style_command_quit() {
        let mut app = App::new(Store::default(), d(31));
        // Plain q / Esc no longer quit from the task list.
        press(&mut app, KeyCode::Char('q'));
        press(&mut app, KeyCode::Esc);
        assert!(!app.quit);

        press(&mut app, KeyCode::Char(':'));
        assert!(matches!(app.mode, Mode::Command(_)));
        press(&mut app, KeyCode::Esc); // cancel
        assert!(matches!(app.mode, Mode::Normal));
        assert!(!app.quit);

        press(&mut app, KeyCode::Char(':'));
        type_text(&mut app, "nope");
        assert!(!press(&mut app, KeyCode::Enter));
        assert_eq!(app.status.as_deref(), Some("Not a command: :nope"));

        press(&mut app, KeyCode::Char(':'));
        type_text(&mut app, "w");
        assert!(press(&mut app, KeyCode::Enter), ":w reports dirty");
        assert!(!app.quit);

        for cmd in ["q", "q!", "wq"] {
            let mut app = App::new(Store::default(), d(31));
            press(&mut app, KeyCode::Char(':'));
            type_text(&mut app, cmd);
            press(&mut app, KeyCode::Enter);
            assert!(app.quit, ":{cmd} should quit");
        }
    }

    #[test]
    fn hide_masks_text_and_blocks_editors_until_reveal() {
        let mut app = App::new(Store::default(), d(31));
        add(&mut app, "therapy session");
        assert!(press(&mut app, KeyCode::Char('H')));
        let task = app.store.tasks(d(31))[0].clone();
        assert!(task.hidden);
        assert_eq!(app.display_text(&task), crate::model::MASK);

        // Editing and notes are blocked while masked.
        press(&mut app, KeyCode::Char('e'));
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.status.as_deref().unwrap().contains("reveal"));
        press(&mut app, KeyCode::Char('n'));
        assert!(matches!(app.mode, Mode::Normal));

        // Hidden tasks are not discoverable through search while masked.
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "therapy");
        assert!(app.search_results().is_empty());
        press(&mut app, KeyCode::Esc);

        // Reveal: text visible, editing allowed, search finds it.
        press(&mut app, KeyCode::Char('v'));
        assert!(app.reveal);
        let task = app.store.tasks(d(31))[0].clone();
        assert_eq!(app.display_text(&task), "therapy session");
        press(&mut app, KeyCode::Char('e'));
        assert!(matches!(app.mode, Mode::Insert(_)));
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "therapy");
        assert_eq!(app.search_results().len(), 1);
        press(&mut app, KeyCode::Esc);

        // Deleting while masked keeps the text out of the status line.
        press(&mut app, KeyCode::Char('v')); // mask again
        assert!(press(&mut app, KeyCode::Char('d')));
        assert!(!app.status.as_deref().unwrap().contains("therapy"));
        press(&mut app, KeyCode::Char('u'));
        assert!(press(&mut app, KeyCode::Char('H')));
        assert!(!app.store.tasks(d(31))[0].hidden);
    }

    #[test]
    fn sorted_view_maps_selection_to_real_index() {
        let mut app = App::new(Store::default(), d(31));
        add(&mut app, "plain");
        add(&mut app, "urgent");
        press(&mut app, KeyCode::Char('p'));
        press(&mut app, KeyCode::Char('p')); // urgent = high, selected (real 1)
        press(&mut app, KeyCode::Char('s'));
        assert!(app.sorted);
        assert_eq!(app.visible(), [1, 0]);
        assert_eq!(app.selected, 0, "selection follows the task");
        // Toggling acts on "urgent" (the real task under the cursor).
        press(&mut app, KeyCode::Char(' '));
        assert!(app.store.tasks(d(31))[1].done);
        assert_eq!(app.visible(), [0, 1], "done sinks to the bottom");
        assert_eq!(app.selected, 1, "still on urgent");
        assert!(!press(&mut app, KeyCode::Char('J')));
        assert!(app.status.as_deref().unwrap().contains("Turn off sorting"));
        press(&mut app, KeyCode::Char('s'));
        assert!(!app.sorted);
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn confirm_delete_when_enabled() {
        let mut app = App::new(Store::default(), d(31));
        app.confirm_delete = true;
        add(&mut app, "keep me");
        assert!(!press(&mut app, KeyCode::Char('d')));
        assert!(matches!(app.mode, Mode::ConfirmDelete(0)));
        assert!(!press(&mut app, KeyCode::Char('n')));
        assert_eq!(app.len(), 1);
        press(&mut app, KeyCode::Char('d'));
        assert!(press(&mut app, KeyCode::Char('y')));
        assert_eq!(app.len(), 0);
    }

    #[test]
    fn search_jumps_to_the_selected_hit() {
        let mut app = App::new(Store::default(), d(31));
        app.store.add(d(28), "call accountant");
        app.store.toggle(d(28), 0, d(28));
        add(&mut app, "buy milk");
        add(&mut app, "accountant email");

        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "ACCOUNT");
        assert_eq!(app.search_results(), [(d(31), 1), (d(28), 0)]);
        press(&mut app, KeyCode::Down);
        assert!(!press(&mut app, KeyCode::Enter));
        assert_eq!(app.date, d(28));
        assert_eq!(app.selected, 0);
        assert!(matches!(app.mode, Mode::Normal));

        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "zzz");
        assert!(app.search_results().is_empty());
        press(&mut app, KeyCode::Enter); // nothing to jump to
        assert_eq!(app.date, d(28));
    }

    #[test]
    fn switching_lists_saves_state_and_carries_over() {
        let mut work = Store::default();
        work.add(d(30), "late report");
        let lists = vec![
            ListSlot {
                name: "personal".into(),
                path: PathBuf::from("/tmp/p.json"),
                store: Store::default(),
            },
            ListSlot {
                name: "work".into(),
                path: PathBuf::from("/tmp/w.json"),
                store: work,
            },
        ];
        let mut app = App::with_lists(lists, d(31));
        assert_eq!(app.list_names(), ["personal", "work"]);
        add(&mut app, "groceries");
        assert!(app.take_others_dirty(), "inactive list was carried over");
        assert!(!app.take_others_dirty(), "reported once");
        assert!(press(&mut app, KeyCode::Tab));
        assert_eq!(app.list_name, "work");
        assert_eq!(texts(&app), ["late report"], "carried over at startup");
        assert_eq!(app.status.as_deref(), Some("List: work"));
        assert!(press(&mut app, KeyCode::Tab));
        assert_eq!(app.list_name, "personal");
        assert_eq!(texts(&app), ["groceries"]);
        assert_eq!(app.all_lists().len(), 2);

        let mut single = App::new(Store::default(), d(31));
        assert!(!press(&mut single, KeyCode::Tab));
        assert!(single.status.as_deref().unwrap().contains("Only one list"));
    }

    fn three_lists(today: NaiveDate) -> App {
        let mut work = Store::default();
        work.add(today, "report");
        work.add(today, "standup");
        let mut home = Store::default();
        home.add(today, "laundry");
        App::with_lists(
            vec![
                ListSlot {
                    name: "personal".into(),
                    path: PathBuf::from("/tmp/p.json"),
                    store: Store::default(),
                },
                ListSlot {
                    name: "work".into(),
                    path: PathBuf::from("/tmp/w.json"),
                    store: work,
                },
                ListSlot {
                    name: "home".into(),
                    path: PathBuf::from("/tmp/h.json"),
                    store: home,
                },
            ],
            today,
        )
    }

    #[test]
    fn list_picker_switches_directly_and_keeps_cycle_order() {
        let mut app = three_lists(d(31));
        assert_eq!(
            app.list_rows(),
            [
                ("personal", 0, true),
                ("work", 2, false),
                ("home", 1, false)
            ]
        );

        press(&mut app, KeyCode::Char('L'));
        assert!(matches!(app.mode, Mode::Lists { selected: 0 }));
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('j'));
        assert!(matches!(app.mode, Mode::Lists { selected: 2 }));
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.list_name, "home");
        assert_eq!(texts(&app), ["laundry"]);
        // Cyclic order is preserved: Tab continues after "home".
        assert_eq!(app.list_names(), ["home", "personal", "work"]);

        // Enter on the active row is a no-op; the cursor cannot leave the rows.
        press(&mut app, KeyCode::Char('L'));
        press(&mut app, KeyCode::Char('G'));
        assert!(matches!(app.mode, Mode::Lists { selected: 3 }));
        press(&mut app, KeyCode::Char('j'));
        assert!(matches!(app.mode, Mode::Lists { selected: 3 }));
        press(&mut app, KeyCode::Char('g'));
        press(&mut app, KeyCode::Char('k'));
        assert!(matches!(app.mode, Mode::Lists { selected: 0 }));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.list_name, "home");

        // Esc and the picker key itself close it.
        press(&mut app, KeyCode::Char('L'));
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Normal));
        press(&mut app, KeyCode::Char('L'));
        press(&mut app, KeyCode::Char('L'));
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn list_picker_new_list_row_opens_the_prompt() {
        let mut app = three_lists(d(31));
        press(&mut app, KeyCode::Char('L'));
        press(&mut app, KeyCode::Char('G')); // "+ New list"
        press(&mut app, KeyCode::Enter);
        match &app.mode {
            Mode::Insert(e) => assert_eq!(e.target, Target::NewList),
            other => panic!("expected new-list prompt, got {other:?}"),
        }
        type_text(&mut app, "errands");
        assert!(!press(&mut app, KeyCode::Enter));
        assert_eq!(app.take_pending_new_list().as_deref(), Some("errands"));

        // The new-list key works from inside the picker too.
        press(&mut app, KeyCode::Char('L'));
        press(&mut app, KeyCode::Char('N'));
        assert!(matches!(app.mode, Mode::Insert(_)));
        press(&mut app, KeyCode::Esc);

        // With a pinned data file the row is inert and explains why.
        app.lists_mutable = false;
        press(&mut app, KeyCode::Char('L'));
        press(&mut app, KeyCode::Char('G'));
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.status.as_deref().unwrap().contains("fixed"));

        let mut single = App::new(Store::default(), d(31));
        press(&mut single, KeyCode::Tab);
        assert!(single.status.as_deref().unwrap().contains("press N"));
    }

    #[test]
    fn split_shows_the_next_list_read_only() {
        let mut app = three_lists(d(31));
        assert!(app.side_list().is_none(), "off by default");
        assert!(!press(&mut app, KeyCode::Char('|')), "no store change");
        assert!(app.split);
        let (name, store) = app.side_list().unwrap();
        assert_eq!(name, "work");
        assert_eq!(store.tasks(d(31)).len(), 2);
        // Tab moves along the cycle; the pane follows.
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.list_name, "work");
        assert_eq!(app.side_list().unwrap().0, "home");
        press(&mut app, KeyCode::Char('|'));
        assert!(!app.split);
        assert!(app.side_list().is_none());
        assert!(app.status.as_deref().unwrap().contains("off"));

        let mut single = App::new(Store::default(), d(31));
        press(&mut single, KeyCode::Char('|'));
        assert!(!single.split);
        assert!(single.status.as_deref().unwrap().contains("Only one list"));
    }

    #[test]
    fn rollover_carries_inactive_lists_too() {
        let mut app = three_lists(d(31));
        app.take_others_dirty();
        add(&mut app, "mine");
        // Midnight: every list's unfinished tasks move to the new day.
        assert!(app.check_date_rollover(sep(1)));
        assert_eq!(texts(&app), ["mine"]);
        assert!(app.take_others_dirty());
        app.split = true;
        let (_, work) = app.side_list().unwrap();
        assert_eq!(work.tasks(sep(1)).len(), 2);
        assert!(work.tasks(d(31)).is_empty());
    }

    #[test]
    fn hide_done_filters_the_view_and_clamps_the_selection() {
        let mut app = App::new(Store::default(), d(31));
        add(&mut app, "one");
        add(&mut app, "two");
        add(&mut app, "three");
        press(&mut app, KeyCode::Char('k'));
        press(&mut app, KeyCode::Char(' ')); // "two" done
        assert_eq!(app.selected_task(), Some(1));

        press(&mut app, KeyCode::Char('f'));
        assert!(app.hide_done);
        assert_eq!(app.visible(), [0, 2]);
        assert_eq!(app.visible_len(), 2);
        assert_eq!(app.len(), 3, "the store still holds every task");
        assert_eq!(
            app.selected_task(),
            Some(2),
            "cursor stays on a visible row"
        );
        // Movement is bounded by the visible rows, not the store.
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.selected, 1);
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(app.selected, 1);

        // Completing the last row keeps the cursor on the remaining task.
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.visible(), [0]);
        assert_eq!(app.selected_task(), Some(0));
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.visible(), Vec::<usize>::new());
        assert_eq!(app.selected_task(), None);
        assert_eq!(app.store.progress(d(31)), (3, 3));

        // Toggling focus off brings everything back, in order.
        press(&mut app, KeyCode::Char('f'));
        assert!(!app.hide_done);
        assert_eq!(app.visible(), [0, 1, 2]);
        assert!(app.status.as_deref().unwrap().contains("Showing"));

        // Focus combines with the priority sort: done tasks drop out.
        press(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Char('f'));
        assert_eq!(app.visible(), Vec::<usize>::new());
        press(&mut app, KeyCode::Char(' ')); // nothing selected: no-op
        assert_eq!(app.store.progress(d(31)), (3, 3));
    }

    #[test]
    fn newlist_command_validates_then_requests_creation() {
        let mut app = App::new(Store::default(), d(31)); // active list: "tasks"
        add(&mut app, "old task");

        press(&mut app, KeyCode::Char(':'));
        type_text(&mut app, "newlist bad/name");
        press(&mut app, KeyCode::Enter);
        assert!(app.status.as_deref().unwrap().contains("invalid list name"));
        assert_eq!(app.take_pending_new_list(), None);

        press(&mut app, KeyCode::Char(':'));
        type_text(&mut app, "newlist tasks");
        press(&mut app, KeyCode::Enter);
        assert!(app.status.as_deref().unwrap().contains("duplicate"));
        assert_eq!(app.take_pending_new_list(), None);

        press(&mut app, KeyCode::Char(':'));
        type_text(&mut app, "newlist work");
        assert!(!press(&mut app, KeyCode::Enter), "no store change");
        assert_eq!(app.take_pending_new_list().as_deref(), Some("work"));
        assert_eq!(app.take_pending_new_list(), None, "consumed");

        // The main loop answers the request with add_list: switch + status.
        app.add_list(
            "work".into(),
            PathBuf::from("/tmp/w.json"),
            Store::default(),
        );
        assert_eq!(app.list_name, "work");
        assert_eq!(app.len(), 0);
        assert_eq!(app.status.as_deref(), Some("Created list \"work\""));
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.list_name, "tasks");
        assert_eq!(texts(&app), ["old task"]);

        // A non-empty adopted store is reported as opened, not created.
        let mut orphan = Store::default();
        orphan.add(d(31), "left behind");
        app.add_list("old".into(), PathBuf::from("/tmp/o.json"), orphan);
        assert_eq!(app.status.as_deref(), Some("Opened existing list \"old\""));
    }

    #[test]
    fn new_list_prompt_keeps_editor_open_on_bad_names() {
        let mut app = App::new(Store::default(), d(31));
        press(&mut app, KeyCode::Char('N'));
        assert!(matches!(
            app.mode,
            Mode::Insert(Editor {
                target: Target::NewList,
                ..
            })
        ));
        type_text(&mut app, "tasks");
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Insert(_)), "stays open to fix");
        assert!(app.status.as_deref().unwrap().contains("duplicate"));
        for _ in 0..5 {
            press(&mut app, KeyCode::Backspace);
        }
        type_text(&mut app, "home");
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.take_pending_new_list().as_deref(), Some("home"));

        // An empty name just cancels.
        press(&mut app, KeyCode::Char('N'));
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.take_pending_new_list(), None);
    }

    #[test]
    fn new_list_refused_when_lists_are_fixed() {
        let mut app = App::new(Store::default(), d(31));
        app.lists_mutable = false;
        press(&mut app, KeyCode::Char('N'));
        assert!(matches!(app.mode, Mode::Normal), "prompt does not open");
        assert!(app.status.as_deref().unwrap().contains("TUI_DO_LIST_FILE"));

        press(&mut app, KeyCode::Char(':'));
        type_text(&mut app, "newlist work");
        press(&mut app, KeyCode::Enter);
        assert!(app.status.as_deref().unwrap().contains("TUI_DO_LIST_FILE"));
        assert_eq!(app.take_pending_new_list(), None);
    }

    #[test]
    fn help_and_review_scroll_and_close() {
        let mut app = App::new(Store::default(), d(31));
        press(&mut app, KeyCode::Char('w'));
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::PageDown);
        assert!(matches!(app.mode, Mode::Review { scroll: 11 }));
        press(&mut app, KeyCode::Char('k'));
        press(&mut app, KeyCode::Char('g'));
        assert!(matches!(app.mode, Mode::Review { scroll: 0 }));
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Normal));
        press(&mut app, KeyCode::Char('?'));
        assert!(matches!(app.mode, Mode::Help { .. }));
        press(&mut app, KeyCode::Char('?'));
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn rollover_cancels_index_bearing_modes_but_keeps_safe_editors() {
        let mut app = App::new(Store::default(), d(30));
        app.confirm_delete = true;
        add(&mut app, "task");
        press(&mut app, KeyCode::Char('d'));
        assert!(matches!(app.mode, Mode::ConfirmDelete(_)));
        app.check_date_rollover(d(31));
        assert!(
            matches!(app.mode, Mode::Normal),
            "confirm cancelled at midnight"
        );

        // A new-task editor holds no index and survives the rollover.
        let mut app = App::new(Store::default(), d(30));
        press(&mut app, KeyCode::Char('a'));
        type_text(&mut app, "half-typed");
        app.check_date_rollover(d(31));
        assert!(matches!(app.mode, Mode::Insert(_)));
        press(&mut app, KeyCode::Enter);
        assert_eq!(texts(&app), ["half-typed"]);

        // Editing by index is cancelled.
        press(&mut app, KeyCode::Char('e'));
        app.check_date_rollover(sep(1));
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn rollover_carries_tasks_and_follows_today() {
        let mut app = App::new(Store::default(), d(30));
        add(&mut app, "late");
        assert!(app.check_date_rollover(d(31)));
        assert_eq!(app.today, d(31));
        assert_eq!(app.date, d(31));
        assert_eq!(texts(&app), ["late"]);
        assert!(!app.check_date_rollover(d(31)));
    }
}
