//! Pure data model: tasks grouped by day, plus the mutations the UI performs.
//! Nothing here touches the terminal or the filesystem.

use std::collections::BTreeMap;

use chrono::{Datelike, Days, Months, NaiveDate, NaiveTime, Weekday};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    #[default]
    None,
    Low,
    High,
}

impl Priority {
    pub fn next(self) -> Self {
        match self {
            Priority::None => Priority::Low,
            Priority::Low => Priority::High,
            Priority::High => Priority::None,
        }
    }
}

/// How often a task comes back after it is completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Repeat {
    Daily,
    Weekdays,
    Weekly,
    Monthly,
}

impl Repeat {
    /// Cycles none → daily → weekdays → weekly → monthly → none.
    pub fn cycle(current: Option<Repeat>) -> Option<Repeat> {
        match current {
            None => Some(Repeat::Daily),
            Some(Repeat::Daily) => Some(Repeat::Weekdays),
            Some(Repeat::Weekdays) => Some(Repeat::Weekly),
            Some(Repeat::Weekly) => Some(Repeat::Monthly),
            Some(Repeat::Monthly) => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Repeat::Daily => "daily",
            Repeat::Weekdays => "weekdays",
            Repeat::Weekly => "weekly",
            Repeat::Monthly => "monthly",
        }
    }

    /// The date of the next occurrence after `from`.
    pub fn next_date(self, from: NaiveDate) -> NaiveDate {
        match self {
            Repeat::Daily => from + Days::new(1),
            Repeat::Weekdays => {
                let mut next = from + Days::new(1);
                while matches!(next.weekday(), Weekday::Sat | Weekday::Sun) {
                    next = next + Days::new(1);
                }
                next
            }
            Repeat::Weekly => from + Days::new(7),
            Repeat::Monthly => from
                .checked_add_months(Months::new(1))
                .unwrap_or(from + Days::new(30)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: u64,
    pub text: String,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub priority: Priority,
    /// The day the task was first written down. Differs from the day it is
    /// listed under once it has been carried over.
    pub created: NaiveDate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed: Option<NaiveDate>,
    /// Time of day to be reminded, shown next to the task and flagged when overdue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remind: Option<NaiveTime>,
    /// Free-text note shown in a panel when the task is selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// When set, completing the task creates its next occurrence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<Repeat>,
    /// Screen privacy: the text is masked in the UI until revealed. The data
    /// file still stores it in plain text — this is NOT encryption.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// What a hidden task's text is shown as while masked.
pub const MASK: &str = "••••••••";

impl Task {
    /// True when the reminder time has passed without the task being done.
    pub fn is_overdue(&self, listed: NaiveDate, today: NaiveDate, now: NaiveTime) -> bool {
        !self.done
            && self
                .remind
                .is_some_and(|r| listed < today || (listed == today && r <= now))
    }
}

pub const STORE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Store {
    pub version: u32,
    pub next_id: u64,
    /// One list per day, in manual order. Days with no tasks are pruned on save.
    pub days: BTreeMap<NaiveDate, Vec<Task>>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            next_id: 1,
            days: BTreeMap::new(),
        }
    }
}

/// Summary numbers for the review screen.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Stats {
    /// Consecutive days (ending today or yesterday) with at least one task completed.
    pub streak: u32,
    pub done_last_7_days: usize,
    /// Unfinished tasks listed today or earlier.
    pub open: usize,
}

impl Store {
    pub fn tasks(&self, date: NaiveDate) -> &[Task] {
        self.days.get(&date).map(Vec::as_slice).unwrap_or(&[])
    }

    fn day_mut(&mut self, date: NaiveDate) -> &mut Vec<Task> {
        self.days.entry(date).or_default()
    }

    /// Appends a task to `date`'s list and returns its index.
    pub fn add(&mut self, date: NaiveDate, text: impl Into<String>) -> usize {
        let task = Task {
            id: self.next_id,
            text: text.into(),
            done: false,
            priority: Priority::None,
            created: date,
            completed: None,
            remind: None,
            note: None,
            repeat: None,
            hidden: false,
        };
        self.next_id += 1;
        let day = self.day_mut(date);
        day.push(task);
        day.len() - 1
    }

    pub fn edit(&mut self, date: NaiveDate, idx: usize, text: impl Into<String>) {
        if let Some(task) = self.day_mut(date).get_mut(idx) {
            task.text = text.into();
        }
    }

    /// Flips `done`; `on` is the date recorded as the completion date.
    ///
    /// Completing a recurring task creates its next occurrence and returns
    /// that date; un-completing removes the spawned copy again — but only if
    /// it is still pristine (never one the user has since customized).
    pub fn toggle(&mut self, date: NaiveDate, idx: usize, on: NaiveDate) -> Option<NaiveDate> {
        let task = self.day_mut(date).get_mut(idx)?;
        task.done = !task.done;
        task.completed = task.done.then_some(on);
        let repeat = task.repeat?;
        let template = task.clone();
        let next = repeat.next_date(date);
        if template.done {
            // The spawned copy may since have been carried past `next`, so
            // dedup against every later day, not just `next` itself.
            if self.find_occurrence(&template, next).is_none() {
                let occurrence = Task {
                    id: self.next_id,
                    done: false,
                    completed: None,
                    created: next,
                    ..template
                };
                self.next_id += 1;
                self.day_mut(next).push(occurrence);
            }
            Some(next)
        } else {
            if let Some((day, pos)) = self.find_occurrence(&template, next) {
                let copy = &self.tasks(day)[pos];
                let pristine = copy.priority == template.priority
                    && copy.remind == template.remind
                    && copy.note == template.note;
                if pristine {
                    self.day_mut(day).remove(pos);
                    self.prune();
                }
            }
            None
        }
    }

    /// The first open task on `from` or later matching `template`'s
    /// text and repeat — i.e. a (possibly carried) spawned occurrence.
    fn find_occurrence(&self, template: &Task, from: NaiveDate) -> Option<(NaiveDate, usize)> {
        self.days.range(from..).find_map(|(day, tasks)| {
            tasks
                .iter()
                .position(|t| !t.done && t.text == template.text && t.repeat == template.repeat)
                .map(|pos| (*day, pos))
        })
    }

    pub fn remove(&mut self, date: NaiveDate, idx: usize) -> Option<Task> {
        let day = self.days.get_mut(&date)?;
        (idx < day.len()).then(|| day.remove(idx))
    }

    /// Inserts at `idx`, clamped to the end of the list (used by undo).
    pub fn insert(&mut self, date: NaiveDate, idx: usize, task: Task) -> usize {
        let day = self.day_mut(date);
        let idx = idx.min(day.len());
        day.insert(idx, task);
        idx
    }

    /// Moves a task to the end of another day's list; returns its new index.
    pub fn move_task(&mut self, from: NaiveDate, idx: usize, to: NaiveDate) -> Option<usize> {
        let task = self.remove(from, idx)?;
        self.prune();
        let day = self.day_mut(to);
        day.push(task);
        Some(day.len() - 1)
    }

    /// Swaps with the previous task; returns the new index.
    pub fn move_up(&mut self, date: NaiveDate, idx: usize) -> usize {
        let day = self.day_mut(date);
        if idx > 0 && idx < day.len() {
            day.swap(idx, idx - 1);
            idx - 1
        } else {
            idx
        }
    }

    /// Swaps with the next task; returns the new index.
    pub fn move_down(&mut self, date: NaiveDate, idx: usize) -> usize {
        let day = self.day_mut(date);
        if idx + 1 < day.len() {
            day.swap(idx, idx + 1);
            idx + 1
        } else {
            idx
        }
    }

    pub fn set_reminder(&mut self, date: NaiveDate, idx: usize, remind: Option<NaiveTime>) {
        if let Some(task) = self.day_mut(date).get_mut(idx) {
            task.remind = remind;
        }
    }

    /// Sets or clears (on empty text) the note.
    pub fn set_note(&mut self, date: NaiveDate, idx: usize, note: &str) {
        if let Some(task) = self.day_mut(date).get_mut(idx) {
            let note = note.trim();
            task.note = (!note.is_empty()).then(|| note.to_string());
        }
    }

    pub fn cycle_priority(&mut self, date: NaiveDate, idx: usize) {
        if let Some(task) = self.day_mut(date).get_mut(idx) {
            task.priority = task.priority.next();
        }
    }

    /// Flips screen-privacy masking; returns the new state.
    pub fn toggle_hidden(&mut self, date: NaiveDate, idx: usize) -> bool {
        match self.day_mut(date).get_mut(idx) {
            Some(task) => {
                task.hidden = !task.hidden;
                task.hidden
            }
            None => false,
        }
    }

    pub fn cycle_repeat(&mut self, date: NaiveDate, idx: usize) -> Option<Repeat> {
        let task = self.day_mut(date).get_mut(idx)?;
        task.repeat = Repeat::cycle(task.repeat);
        task.repeat
    }

    /// `(done, total)` for the given day.
    pub fn progress(&self, date: NaiveDate) -> (usize, usize) {
        let tasks = self.tasks(date);
        (tasks.iter().filter(|t| t.done).count(), tasks.len())
    }

    /// The earliest unfinished reminder on `date`, split into overdue
    /// (`<= now`) and upcoming (`> now`). Only meaningful for today.
    pub fn reminders(&self, date: NaiveDate, now: NaiveTime) -> (Option<&Task>, Option<&Task>) {
        let pending = self
            .tasks(date)
            .iter()
            .filter(|t| !t.done && t.remind.is_some());
        let overdue = pending
            .clone()
            .filter(|t| t.remind <= Some(now))
            .min_by_key(|t| t.remind);
        let upcoming = pending
            .filter(|t| t.remind > Some(now))
            .min_by_key(|t| t.remind);
        (overdue, upcoming)
    }

    /// Display order for `date` with overdue reminders first, then priority
    /// (high → low → none), then reminder time, then manual order. Done tasks
    /// sink to the bottom.
    pub fn sorted_indices(&self, date: NaiveDate, today: NaiveDate, now: NaiveTime) -> Vec<usize> {
        let tasks = self.tasks(date);
        let mut order: Vec<usize> = (0..tasks.len()).collect();
        order.sort_by_key(|&i| {
            let t = &tasks[i];
            (
                t.done,
                !t.is_overdue(date, today, now),
                std::cmp::Reverse(t.priority),
                t.remind.is_none(),
                t.remind,
                i,
            )
        });
        order
    }

    /// Case-insensitive substring search over text and notes, newest day first.
    pub fn search(&self, query: &str) -> Vec<(NaiveDate, usize)> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        let query = query.as_str();
        self.days
            .iter()
            .rev()
            .flat_map(|(date, tasks)| {
                tasks.iter().enumerate().filter_map(move |(idx, t)| {
                    let hit = t.text.to_lowercase().contains(query)
                        || t.note
                            .as_deref()
                            .is_some_and(|n| n.to_lowercase().contains(query));
                    hit.then_some((*date, idx))
                })
            })
            .collect()
    }

    /// Tasks completed on `date`, wherever they are listed.
    pub fn completed_on(&self, date: NaiveDate) -> Vec<&Task> {
        self.days
            .values()
            .flatten()
            .filter(|t| t.completed == Some(date))
            .collect()
    }

    /// Unfinished tasks listed today or earlier, oldest first.
    pub fn open_tasks(&self, today: NaiveDate) -> Vec<(NaiveDate, &Task)> {
        let mut open: Vec<(NaiveDate, &Task)> = self
            .days
            .range(..=today)
            .flat_map(|(date, tasks)| tasks.iter().filter(|t| !t.done).map(move |t| (*date, t)))
            .collect();
        open.sort_by_key(|(_, t)| t.created);
        open
    }

    pub fn stats(&self, today: NaiveDate) -> Stats {
        let (done, total) = self.progress(today);
        let mut day = if total > 0 && done == total {
            today
        } else {
            today - Days::new(1)
        };
        let mut streak = 0;
        while !self.completed_on(day).is_empty() {
            streak += 1;
            day = day - Days::new(1);
        }
        let done_last_7_days = (0..7)
            .map(|i| self.completed_on(today - Days::new(i)).len())
            .sum();
        Stats {
            streak,
            done_last_7_days,
            open: self.open_tasks(today).len(),
        }
    }

    /// Moves every unfinished task from days before `today` to the front of
    /// `today`'s list (oldest first, relative order kept). Future days are left
    /// alone. Returns how many tasks were moved.
    pub fn carry_over(&mut self, today: NaiveDate) -> usize {
        let mut moved = Vec::new();
        for (_, tasks) in self.days.range_mut(..today) {
            let (undone, done): (Vec<_>, Vec<_>) = tasks.drain(..).partition(|t| !t.done);
            *tasks = done;
            moved.extend(undone);
        }
        let count = moved.len();
        if count > 0 {
            let day = self.day_mut(today);
            moved.append(day);
            *day = moved;
        }
        self.prune();
        count
    }

    /// Drops days that no longer hold any tasks.
    pub fn prune(&mut self) {
        self.days.retain(|_, tasks| !tasks.is_empty());
    }
}

/// Parses a reminder time typed by the user: `14:30`, `14h30`, `1430`, `14h` or `14`.
pub fn parse_time(input: &str) -> Option<NaiveTime> {
    let s = input.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }
    let separators = s.chars().filter(|c| !c.is_ascii_digit()).count();
    let (h, m) = match separators {
        // "1430" / "930" / "14" / "9"
        0 => match s.len() {
            1..=2 => (s.parse().ok()?, 0),
            3..=4 => {
                let (h, m) = s.split_at(s.len() - 2);
                (h.parse().ok()?, m.parse().ok()?)
            }
            _ => return None,
        },
        // "14:30" / "14h30" / "9:05" / "14h" — minutes must be two digits,
        // so "14:3" is rejected instead of being misread as 01:43.
        1 => {
            let (hour, minute) = s.split_once(|c: char| !c.is_ascii_digit())?;
            if hour.is_empty() || hour.len() > 2 {
                return None;
            }
            let m = match minute.len() {
                0 => 0,
                2 => minute.parse().ok()?,
                _ => return None,
            };
            (hour.parse().ok()?, m)
        }
        _ => return None,
    };
    NaiveTime::from_hms_opt(h, m, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    fn d(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 8, day).unwrap()
    }

    fn sep(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, day).unwrap()
    }

    fn texts(store: &Store, date: NaiveDate) -> Vec<&str> {
        store.tasks(date).iter().map(|t| t.text.as_str()).collect()
    }

    #[test]
    fn add_assigns_increasing_ids_and_appends() {
        let mut s = Store::default();
        assert_eq!(s.add(d(31), "a"), 0);
        assert_eq!(s.add(d(31), "b"), 1);
        let tasks = s.tasks(d(31));
        assert_eq!(tasks[0].id, 1);
        assert_eq!(tasks[1].id, 2);
        assert_eq!(tasks[1].created, d(31));
        assert_eq!(s.next_id, 3);
    }

    #[test]
    fn toggle_records_and_clears_completion_date() {
        let mut s = Store::default();
        s.add(d(31), "a");
        assert_eq!(s.toggle(d(31), 0, d(31)), None);
        assert!(s.tasks(d(31))[0].done);
        assert_eq!(s.tasks(d(31))[0].completed, Some(d(31)));
        s.toggle(d(31), 0, d(31));
        assert!(!s.tasks(d(31))[0].done);
        assert_eq!(s.tasks(d(31))[0].completed, None);
        // Out-of-range index is a no-op, not a panic.
        assert_eq!(s.toggle(d(31), 7, d(31)), None);
    }

    #[test]
    fn remove_then_insert_restores_original_position() {
        let mut s = Store::default();
        for t in ["a", "b", "c"] {
            s.add(d(31), t);
        }
        let removed = s.remove(d(31), 1).unwrap();
        assert_eq!(texts(&s, d(31)), ["a", "c"]);
        assert!(s.remove(d(31), 5).is_none());
        assert_eq!(s.insert(d(31), 1, removed), 1);
        assert_eq!(texts(&s, d(31)), ["a", "b", "c"]);
        // Insert index beyond the end is clamped.
        let last = s.remove(d(31), 0).unwrap();
        assert_eq!(s.insert(d(31), 99, last), 2);
        assert_eq!(texts(&s, d(31)), ["b", "c", "a"]);
    }

    #[test]
    fn move_task_between_days_keeps_created_date() {
        let mut s = Store::default();
        s.add(d(30), "a");
        s.add(d(31), "b");
        assert_eq!(s.move_task(d(30), 0, d(31)), Some(1));
        assert!(!s.days.contains_key(&d(30)));
        assert_eq!(texts(&s, d(31)), ["b", "a"]);
        assert_eq!(s.tasks(d(31))[1].created, d(30));
        assert_eq!(s.move_task(d(31), 9, sep(1)), None);
    }

    #[test]
    fn move_up_down_respect_bounds() {
        let mut s = Store::default();
        for t in ["a", "b", "c"] {
            s.add(d(31), t);
        }
        assert_eq!(s.move_up(d(31), 0), 0);
        assert_eq!(s.move_down(d(31), 2), 2);
        assert_eq!(texts(&s, d(31)), ["a", "b", "c"]);
        assert_eq!(s.move_down(d(31), 0), 1);
        assert_eq!(texts(&s, d(31)), ["b", "a", "c"]);
        assert_eq!(s.move_up(d(31), 2), 1);
        assert_eq!(texts(&s, d(31)), ["b", "c", "a"]);
    }

    #[test]
    fn cycle_priority_wraps() {
        let mut s = Store::default();
        s.add(d(31), "a");
        let prio = |s: &Store| s.tasks(d(31))[0].priority;
        assert_eq!(prio(&s), Priority::None);
        s.cycle_priority(d(31), 0);
        assert_eq!(prio(&s), Priority::Low);
        s.cycle_priority(d(31), 0);
        assert_eq!(prio(&s), Priority::High);
        s.cycle_priority(d(31), 0);
        assert_eq!(prio(&s), Priority::None);
    }

    #[test]
    fn notes_set_and_clear() {
        let mut s = Store::default();
        s.add(d(31), "a");
        s.set_note(d(31), 0, "  call after 3pm ");
        assert_eq!(s.tasks(d(31))[0].note.as_deref(), Some("call after 3pm"));
        s.set_note(d(31), 0, "   ");
        assert_eq!(s.tasks(d(31))[0].note, None);
    }

    #[test]
    fn progress_counts_done_tasks() {
        let mut s = Store::default();
        assert_eq!(s.progress(d(31)), (0, 0));
        s.add(d(31), "a");
        s.add(d(31), "b");
        s.toggle(d(31), 1, d(31));
        assert_eq!(s.progress(d(31)), (1, 2));
    }

    #[test]
    fn parse_time_accepts_common_formats() {
        for (input, expected) in [
            ("14:30", t(14, 30)),
            (" 9:05 ", t(9, 5)),
            ("14h30", t(14, 30)),
            ("14H", t(14, 0)),
            ("1430", t(14, 30)),
            ("930", t(9, 30)),
            ("9", t(9, 0)),
            ("00:00", t(0, 0)),
            ("23:59", t(23, 59)),
        ] {
            assert_eq!(parse_time(input), Some(expected), "input {input:?}");
        }
        for input in [
            "", "   ", "24:00", "12:60", "abc", "1:2:3", "14:30pm", "123456", "14:3", "10:5",
            "14h3", "1:305", ":30",
        ] {
            assert_eq!(parse_time(input), None, "input {input:?}");
        }
    }

    #[test]
    fn reminders_split_into_overdue_and_upcoming() {
        let mut s = Store::default();
        s.add(d(31), "no reminder");
        s.add(d(31), "late");
        s.set_reminder(d(31), 1, Some(t(9, 0)));
        s.add(d(31), "later");
        s.set_reminder(d(31), 2, Some(t(15, 0)));
        s.add(d(31), "soon");
        s.set_reminder(d(31), 3, Some(t(14, 0)));
        s.add(d(31), "done");
        s.set_reminder(d(31), 4, Some(t(8, 0)));
        s.toggle(d(31), 4, d(31));

        let (overdue, upcoming) = s.reminders(d(31), t(12, 0));
        assert_eq!(overdue.map(|x| x.text.as_str()), Some("late"));
        assert_eq!(upcoming.map(|x| x.text.as_str()), Some("soon"));
        // Exactly at the reminder time counts as due.
        let (overdue, _) = s.reminders(d(31), t(14, 0));
        assert_eq!(overdue.map(|x| x.text.as_str()), Some("late"));
        let (_, upcoming) = s.reminders(d(31), t(23, 0));
        assert!(upcoming.is_none());
        s.set_reminder(d(31), 1, None);
        assert_eq!(s.tasks(d(31))[1].remind, None);
    }

    #[test]
    fn repeat_next_dates() {
        let fri = d(28); // Fri 28 Aug 2026
        assert_eq!(Repeat::Daily.next_date(fri), d(29));
        assert_eq!(Repeat::Weekdays.next_date(fri), d(31)); // skips the weekend
        assert_eq!(Repeat::Weekdays.next_date(d(31)), sep(1));
        assert_eq!(Repeat::Weekly.next_date(fri), sep(4));
        assert_eq!(Repeat::Monthly.next_date(d(31)), sep(30)); // clamps to month end
        assert_eq!(Repeat::cycle(None), Some(Repeat::Daily));
        assert_eq!(Repeat::cycle(Some(Repeat::Monthly)), None);
    }

    #[test]
    fn completing_a_recurring_task_spawns_the_next_occurrence() {
        let mut s = Store::default();
        s.add(d(31), "standup");
        s.set_reminder(d(31), 0, Some(t(9, 30)));
        s.cycle_priority(d(31), 0);
        assert_eq!(s.cycle_repeat(d(31), 0), Some(Repeat::Daily));
        assert_eq!(s.cycle_repeat(d(31), 0), Some(Repeat::Weekdays));

        assert_eq!(s.toggle(d(31), 0, d(31)), Some(sep(1)));
        let next = &s.tasks(sep(1))[0];
        assert_eq!(next.text, "standup");
        assert!(!next.done);
        assert_eq!(next.created, sep(1));
        assert_eq!(next.remind, Some(t(9, 30)));
        assert_eq!(next.priority, Priority::Low);
        assert_eq!(next.repeat, Some(Repeat::Weekdays));
        assert_ne!(next.id, s.tasks(d(31))[0].id);

        // Toggling twice more does not duplicate; un-completing removes it.
        s.toggle(d(31), 0, d(31));
        assert!(!s.days.contains_key(&sep(1)));
        s.toggle(d(31), 0, d(31));
        s.toggle(d(31), 0, d(31));
        s.toggle(d(31), 0, d(31));
        assert_eq!(s.tasks(sep(1)).len(), 1);
    }

    #[test]
    fn hidden_flag_toggles_and_is_inherited_by_occurrences() {
        let mut s = Store::default();
        s.add(d(31), "therapy");
        assert!(s.toggle_hidden(d(31), 0));
        assert!(s.tasks(d(31))[0].hidden);
        s.cycle_repeat(d(31), 0); // daily
        s.toggle(d(31), 0, d(31));
        assert!(s.tasks(sep(1))[0].hidden, "spawned occurrence stays hidden");
        assert!(!s.toggle_hidden(d(31), 0));
        assert!(!s.toggle_hidden(d(31), 9), "out of range is a no-op");
    }

    #[test]
    fn untoggle_keeps_a_customized_occurrence() {
        let mut s = Store::default();
        s.add(d(31), "gym");
        s.cycle_repeat(d(31), 0);
        s.toggle(d(31), 0, d(31)); // spawns on Sep 1
        s.set_note(sep(1), 0, "bring towel");
        s.toggle(d(31), 0, d(31)); // un-complete: the customized copy survives
        assert_eq!(s.tasks(sep(1)).len(), 1);
        assert_eq!(s.tasks(sep(1))[0].note.as_deref(), Some("bring towel"));
        // A pristine copy is still cleaned up as before.
        s.days.remove(&sep(1));
        s.toggle(d(31), 0, d(31));
        s.toggle(d(31), 0, d(31));
        assert!(!s.days.contains_key(&sep(1)));
    }

    #[test]
    fn recurring_toggle_deduplicates_across_carry_over() {
        let open_count = |s: &Store| -> usize {
            s.days
                .values()
                .flatten()
                .filter(|t| t.text == "standup" && !t.done)
                .count()
        };
        let mut s = Store::default();
        s.add(d(28), "standup"); // Fri
        s.cycle_repeat(d(28), 0);
        s.toggle(d(28), 0, d(28)); // spawns Sat 29
        assert_eq!(s.tasks(d(29)).len(), 1);
        s.carry_over(d(31)); // Sat's copy is carried to Mon 31
        assert_eq!(open_count(&s), 1);

        // Un-toggle finds the carried copy (not just on the 29th) and removes it.
        s.toggle(d(28), 0, d(28));
        assert_eq!(open_count(&s), 1, "only the un-completed original remains");
        assert!(!s.days.contains_key(&d(31)));
        // Re-toggle spawns exactly one open copy again.
        s.toggle(d(28), 0, d(28));
        assert_eq!(open_count(&s), 1);
        // Toggling while the copy sits on a later day must not duplicate.
        s.carry_over(d(31));
        s.toggle(d(28), 0, d(28));
        s.toggle(d(28), 0, d(28));
        assert_eq!(open_count(&s), 1);
    }

    #[test]
    fn sorted_indices_put_overdue_and_high_priority_first() {
        let mut s = Store::default();
        s.add(d(31), "plain"); // 0
        s.add(d(31), "done high"); // 1
        s.cycle_priority(d(31), 1);
        s.cycle_priority(d(31), 1);
        s.toggle(d(31), 1, d(31));
        s.add(d(31), "high"); // 2
        s.cycle_priority(d(31), 2);
        s.cycle_priority(d(31), 2);
        s.add(d(31), "overdue"); // 3
        s.set_reminder(d(31), 3, Some(t(8, 0)));
        s.add(d(31), "later reminder"); // 4
        s.set_reminder(d(31), 4, Some(t(18, 0)));
        s.add(d(31), "low"); // 5
        s.cycle_priority(d(31), 5);

        assert_eq!(s.sorted_indices(d(31), d(31), t(12, 0)), [3, 2, 5, 4, 0, 1]);
    }

    #[test]
    fn search_is_case_insensitive_and_newest_first() {
        let mut s = Store::default();
        s.add(d(28), "Call the Accountant");
        s.add(d(31), "buy milk");
        s.add(d(31), "email");
        s.set_note(d(31), 1, "ask the accountant about VAT");
        assert_eq!(s.search("ACCOUNTANT"), [(d(31), 1), (d(28), 0)]);
        assert_eq!(s.search("milk"), [(d(31), 0)]);
        assert!(s.search("  ").is_empty());
        assert!(s.search("zzz").is_empty());
    }

    #[test]
    fn stats_count_streak_and_recent_completions() {
        let mut s = Store::default();
        for day in [28, 29, 30] {
            s.add(d(day), "x");
            s.toggle(d(day), 0, d(day));
        }
        s.add(d(31), "open since 26");
        s.tasks(d(31)); // today has an open task, so the streak ends yesterday
        s.days.get_mut(&d(31)).unwrap()[0].created = d(26);
        let stats = s.stats(d(31));
        assert_eq!(stats.streak, 3);
        assert_eq!(stats.done_last_7_days, 3);
        assert_eq!(stats.open, 1);
        assert_eq!(s.completed_on(d(29)).len(), 1);
        assert_eq!(s.open_tasks(d(31))[0].1.created, d(26));

        // Finishing today extends the streak to 4.
        s.toggle(d(31), 0, d(31));
        assert_eq!(s.stats(d(31)).streak, 4);
        // A gap breaks it.
        s.days.remove(&d(29));
        assert_eq!(s.stats(d(31)).streak, 2);
    }

    #[test]
    fn carry_over_moves_only_undone_past_tasks_to_front_of_today() {
        let mut s = Store::default();
        s.add(d(28), "old-undone");
        s.add(d(28), "old-done");
        s.toggle(d(28), 1, d(28));
        s.add(d(30), "yesterday-undone");
        s.add(d(31), "today-existing");
        s.add(d(31), "today-done");
        s.toggle(d(31), 1, d(31));
        let future = sep(5);
        s.add(future, "future");

        assert_eq!(s.carry_over(d(31)), 2);
        assert_eq!(
            texts(&s, d(31)),
            [
                "old-undone",
                "yesterday-undone",
                "today-existing",
                "today-done"
            ]
        );
        // Past days keep only what was completed; empty days are pruned.
        assert_eq!(texts(&s, d(28)), ["old-done"]);
        assert!(!s.days.contains_key(&d(30)));
        // Future planning is untouched, and the original creation date survives.
        assert_eq!(texts(&s, future), ["future"]);
        assert_eq!(s.tasks(d(31))[0].created, d(28));
        // Running it again is a no-op.
        assert_eq!(s.carry_over(d(31)), 0);
    }

    #[test]
    fn carry_over_with_nothing_to_move_does_not_create_today() {
        let mut s = Store::default();
        s.add(d(28), "done");
        s.toggle(d(28), 0, d(28));
        assert_eq!(s.carry_over(d(31)), 0);
        assert!(!s.days.contains_key(&d(31)));
    }
}
