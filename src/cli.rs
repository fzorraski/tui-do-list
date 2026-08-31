//! Non-interactive subcommands: `export` (Markdown) and `notify` (desktop
//! notifications for due reminders, meant to be run by a timer).

use std::fmt::Write as _;
use std::process::Command;

use anyhow::{Context, Result, bail};
use chrono::{Days, NaiveDate, NaiveDateTime, TimeDelta};

use crate::model::{Priority, Store, Task};

pub const USAGE: &str = "\
tui-do-list — a daily to-do list for the terminal

USAGE
    tui-do-list                       open the TUI
    tui-do-list export [RANGE] [--show-hidden]
                                    print tasks as Markdown
    tui-do-list notify [--window S]   desktop-notify reminders due in the last S seconds
    tui-do-list --help | --version

EXPORT RANGE
    --day [YYYY-MM-DD]   one day (default: today)
    --week               the last 7 days
    --all                everything
    --show-hidden        include the real text of hidden tasks

ENVIRONMENT
    TUI_DO_LIST_FILE        data file (single list; overrides `lists` in the config)
    TUI_DO_LIST_CONFIG      config file (default: ~/.config/tui-do-list/config.toml)
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportRange {
    Day(NaiveDate),
    Week,
    All,
}

pub fn parse_export_args(args: &[String], today: NaiveDate) -> Result<(ExportRange, bool)> {
    let show_hidden = args.iter().any(|a| a == "--show-hidden");
    let args: Vec<&String> = args.iter().filter(|a| *a != "--show-hidden").collect();
    let range: Result<ExportRange> = match args.as_slice() {
        [] => Ok(ExportRange::Day(today)),
        [flag] if *flag == "--day" => Ok(ExportRange::Day(today)),
        [flag, date] if *flag == "--day" => {
            Ok(ExportRange::Day(date.parse().with_context(|| {
                format!("invalid date {date:?}, expected YYYY-MM-DD")
            })?))
        }
        [flag] if *flag == "--week" => Ok(ExportRange::Week),
        [flag] if *flag == "--all" => Ok(ExportRange::All),
        other => bail!(
            "unexpected export arguments: {}",
            other
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        ),
    };
    Ok((range?, show_hidden))
}

fn task_line(task: &Task, show_hidden: bool) -> String {
    let masked = task.hidden && !show_hidden;
    let text = if masked {
        crate::model::MASK
    } else {
        task.text.as_str()
    };
    let mut line = format!("- [{}] {text}", if task.done { "x" } else { " " });
    match task.priority {
        Priority::High => line.push_str(" **!**"),
        Priority::Low => line.push_str(" _(low)_"),
        Priority::None => {}
    }
    if let Some(remind) = task.remind {
        let _ = write!(line, " ⏰ {}", remind.format("%H:%M"));
    }
    if let Some(repeat) = task.repeat {
        let _ = write!(line, " ↻ {}", repeat.label());
    }
    if let Some(note) = &task.note
        && !masked
    {
        for note_line in note.lines() {
            let _ = write!(line, "\n  > {note_line}");
        }
    }
    line
}

/// Renders the chosen range of every list as Markdown.
pub fn export_markdown(
    lists: &[(String, Store)],
    range: ExportRange,
    today: NaiveDate,
    show_hidden: bool,
) -> String {
    let mut out = String::new();
    for (name, store) in lists {
        if lists.len() > 1 {
            let _ = writeln!(out, "# {name}\n");
        }
        let dates: Vec<NaiveDate> = match range {
            ExportRange::Day(date) => vec![date],
            ExportRange::Week => (0..7).rev().map(|i| today - Days::new(i)).collect(),
            ExportRange::All => store.days.keys().copied().collect(),
        };
        let mut wrote_any = false;
        for date in dates {
            let tasks = store.tasks(date);
            if tasks.is_empty() && !matches!(range, ExportRange::Day(_)) {
                continue;
            }
            wrote_any = true;
            let _ = writeln!(out, "## {}\n", date.format("%a, %d %b %Y"));
            if tasks.is_empty() {
                out.push_str("_No tasks._\n\n");
                continue;
            }
            for task in tasks {
                let _ = writeln!(out, "{}", task_line(task, show_hidden));
            }
            out.push('\n');
        }
        if !wrote_any {
            out.push_str("_No tasks._\n\n");
        }
    }
    out.trim_end().to_string() + "\n"
}

/// Today's unfinished tasks whose reminder time fell in `(now - window, now]`.
pub fn due_reminders(
    lists: &[(String, Store)],
    now: NaiveDateTime,
    window_secs: u64,
) -> Vec<(String, Task)> {
    let start = now - TimeDelta::seconds(window_secs as i64);
    let today = now.date();
    lists
        .iter()
        .flat_map(|(name, store)| {
            store
                .tasks(today)
                .iter()
                .filter(|t| !t.done)
                .filter(|t| {
                    t.remind
                        .map(|r| today.and_time(r))
                        .is_some_and(|at| start < at && at <= now)
                })
                .map(move |t| (name.clone(), t.clone()))
        })
        .collect()
}

/// Sends one desktop notification via `notify-send`.
pub fn send_notification(summary: &str, body: &str) -> Result<()> {
    let status = Command::new("notify-send")
        // "--" stops option parsing so a note beginning with "-" can neither
        // fail the command nor inject notify-send options.
        .args([
            "--app-name=tui-do-list",
            "--urgency=normal",
            "--",
            summary,
            body,
        ])
        .status()
        .context("running notify-send (is libnotify installed?)")?;
    if !status.success() {
        bail!("notify-send exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveTime;

    fn d(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 8, day).unwrap()
    }

    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    fn sample() -> Store {
        let mut s = Store::default();
        s.add(d(30), "yesterday done");
        s.toggle(d(30), 0, d(30));
        s.add(d(31), "Buy milk");
        s.cycle_priority(d(31), 0);
        s.cycle_priority(d(31), 0);
        s.set_reminder(d(31), 0, Some(t(14, 30)));
        s.set_note(d(31), 0, "semi-skimmed\ntwo litres");
        s.add(d(31), "Standup");
        s.cycle_repeat(d(31), 1);
        s.toggle(d(31), 1, d(31));
        s
    }

    #[test]
    fn export_day_renders_markdown() {
        let lists = vec![("tasks".to_string(), sample())];
        let md = export_markdown(&lists, ExportRange::Day(d(31)), d(31), false);
        assert_eq!(
            md,
            "## Mon, 31 Aug 2026\n\n\
             - [ ] Buy milk **!** ⏰ 14:30\n  > semi-skimmed\n  > two litres\n\
             - [x] Standup ↻ daily\n"
        );
        let empty = export_markdown(&lists, ExportRange::Day(d(1)), d(31), false);
        assert_eq!(empty, "## Sat, 01 Aug 2026\n\n_No tasks._\n");
    }

    #[test]
    fn export_week_and_all_skip_empty_days_and_name_lists() {
        let lists = vec![
            ("personal".to_string(), sample()),
            ("work".to_string(), Store::default()),
        ];
        let md = export_markdown(&lists, ExportRange::Week, d(31), false);
        assert!(md.starts_with("# personal\n\n## Sun, 30 Aug 2026\n\n- [x] yesterday done\n"));
        assert!(md.contains("## Mon, 31 Aug 2026"));
        assert!(!md.contains("29 Aug"));
        assert!(md.contains("# work\n\n_No tasks._"));
        // The spawned occurrence for tomorrow appears only in --all.
        assert!(!md.contains("01 Sep"));
        let all = export_markdown(&lists, ExportRange::All, d(31), false);
        assert!(all.contains("## Tue, 01 Sep 2026\n\n- [ ] Standup ↻ daily"));
    }

    #[test]
    fn export_args() {
        let today = d(31);
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(
            parse_export_args(&s(&[]), today).unwrap(),
            (ExportRange::Day(today), false)
        );
        assert_eq!(
            parse_export_args(&s(&["--day"]), today).unwrap(),
            (ExportRange::Day(today), false)
        );
        assert_eq!(
            parse_export_args(&s(&["--day", "2026-08-01"]), today).unwrap(),
            (ExportRange::Day(d(1)), false)
        );
        assert_eq!(
            parse_export_args(&s(&["--week"]), today).unwrap(),
            (ExportRange::Week, false)
        );
        assert_eq!(
            parse_export_args(&s(&["--all"]), today).unwrap(),
            (ExportRange::All, false)
        );
        assert!(parse_export_args(&s(&["--day", "yesterday"]), today).is_err());
        assert!(parse_export_args(&s(&["--month"]), today).is_err());
        assert_eq!(
            parse_export_args(&s(&["--week", "--show-hidden"]), today).unwrap(),
            (ExportRange::Week, true)
        );
        assert_eq!(
            parse_export_args(&s(&["--show-hidden"]), today).unwrap(),
            (ExportRange::Day(today), true)
        );
    }

    #[test]
    fn export_masks_hidden_tasks_unless_requested() {
        let mut s = Store::default();
        s.add(d(31), "therapy");
        s.set_note(d(31), 0, "Dr. Silva");
        s.toggle_hidden(d(31), 0);
        let lists = vec![("tasks".to_string(), s)];
        let md = export_markdown(&lists, ExportRange::Day(d(31)), d(31), false);
        assert!(md.contains("••••••••"), "{md}");
        assert!(!md.contains("therapy") && !md.contains("Dr. Silva"));
        let md = export_markdown(&lists, ExportRange::Day(d(31)), d(31), true);
        assert!(md.contains("therapy") && md.contains("Dr. Silva"));
    }

    #[test]
    fn due_reminders_fire_for_carried_over_tasks_once_carry_over_ran() {
        // `notify`/`export` apply carry_over after loading (see main.rs), so a
        // reminder on an unfinished task from a past day fires today.
        let mut s = Store::default();
        s.add(d(30), "take medication");
        s.set_reminder(d(30), 0, Some(t(9, 0)));
        let mut lists = vec![("tasks".to_string(), s)];
        let at = d(31).and_hms_opt(9, 0, 0).unwrap();
        assert!(due_reminders(&lists, at, 60).is_empty(), "raw file: silent");
        lists[0].1.carry_over(d(31));
        let due = due_reminders(&lists, at, 60);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].1.text, "take medication");
    }

    #[test]
    fn due_reminders_respect_the_window_and_done_state() {
        let lists = vec![("tasks".to_string(), sample())];
        let at = |h, m, s| d(31).and_hms_opt(h, m, s).unwrap();
        assert!(due_reminders(&lists, at(14, 29, 59), 60).is_empty());
        let due = due_reminders(&lists, at(14, 30, 0), 60);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].0, "tasks");
        assert_eq!(due[0].1.text, "Buy milk");
        assert_eq!(due_reminders(&lists, at(14, 30, 59), 60).len(), 1);
        assert!(due_reminders(&lists, at(14, 31, 0), 60).is_empty());
        // Wider window catches it later; other days never notify.
        assert_eq!(due_reminders(&lists, at(14, 34, 0), 300).len(), 1);
        let tomorrow = NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
        assert!(due_reminders(&lists, tomorrow.and_hms_opt(14, 30, 0).unwrap(), 60).is_empty());
    }
}
