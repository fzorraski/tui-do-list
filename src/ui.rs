//! Rendering. Reads the `App` and draws; never mutates the store.

use chrono::{Days, NaiveDate, NaiveTime};
use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::calendar::{CalendarEventStore, Monthly};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Wrap,
};

use crate::app::{App, Mode, Target};
use crate::config::{Action, Theme};
use crate::model::{Priority, Task};

pub fn render(frame: &mut Frame, app: &mut App) {
    let input_height = u16::from(matches!(app.mode, Mode::Insert(_) | Mode::Command(_)));
    let note = app.selected_task().and_then(|idx| {
        let task = &app.store.tasks(app.date)[idx];
        if task.hidden && !app.reveal {
            None // never show a hidden task's note while masked
        } else {
            task.note.clone()
        }
    });
    let note_height = note
        .as_deref()
        .map_or(0, |n| note_panel_height(n, frame.area().width));
    let [header, body, note_area, input, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(note_height),
        Constraint::Length(input_height),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    render_header(frame, header, app);
    render_list(frame, body, app);
    if let Some(note) = note {
        render_note(frame, note_area, &note);
    }
    if let Mode::Insert(editor) = &app.mode {
        let label = match editor.target {
            Target::NewTask => "new",
            Target::EditTask(_) => "edit",
            Target::Reminder(_) => "remind at (HH:MM)",
            Target::Note(_) => "note",
            Target::Search => "search",
            Target::Command => ":",
            Target::NewList => "new list",
        };
        let line = Line::from(vec![
            Span::styled(
                format!(" {label}> "),
                Style::new().fg(app.theme.accent).bold(),
            ),
            Span::raw(editor.text()),
        ]);
        frame.render_widget(Paragraph::new(line), input);
        // Cursor position: prefix width + chars before the cursor.
        let x = input.x + (label.chars().count() as u16 + 3) + editor.cursor as u16;
        frame.set_cursor_position((x.min(input.right().saturating_sub(1)), input.y));
    }
    if let Mode::Command(editor) = &app.mode {
        let line = Line::from(vec![
            Span::styled(" :", Style::new().fg(app.theme.accent).bold()),
            Span::raw(editor.text()),
        ]);
        frame.render_widget(Paragraph::new(line), input);
        let x = input.x + 2 + editor.cursor as u16;
        frame.set_cursor_position((x.min(input.right().saturating_sub(1)), input.y));
    }
    render_footer(frame, footer, app);
    match app.mode {
        Mode::Help { scroll } => render_help(frame, frame.area(), app, scroll),
        Mode::Calendar { cursor, moving } => {
            render_calendar(frame, frame.area(), app, cursor, moving.is_some());
        }
        Mode::Search { .. } => render_search(frame, frame.area(), app),
        Mode::Review { scroll } => render_review(frame, frame.area(), app, scroll),
        _ => {}
    }
}

fn relative_label(date: NaiveDate, today: NaiveDate) -> String {
    match (date - today).num_days() {
        0 => "Today".into(),
        -1 => "Yesterday".into(),
        1 => "Tomorrow".into(),
        n if n < 0 => format!("{} days ago", -n),
        n => format!("+{n} days"),
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let [popup] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(popup);
    popup
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let label = relative_label(app.date, app.today);
    let label_style = if app.date == app.today {
        Style::new().fg(theme.today).bold()
    } else {
        Style::new().fg(theme.other_day)
    };
    let mut spans = vec![
        Span::styled(
            format!(" {}", app.date.format("%a, %d %b %Y")),
            Style::new().bold(),
        ),
        Span::raw(" · "),
        Span::styled(label, label_style),
    ];
    let names = app.list_names();
    if names.len() > 1 {
        spans.push(Span::styled(
            format!("  [{}]", names[0]),
            Style::new().fg(theme.accent).bold(),
        ));
        spans.push(Span::styled(
            format!(" {}", names[1..].join(" ")),
            Style::new().dim(),
        ));
    }
    if app.sorted {
        spans.push(Span::styled("  ⇅ sorted", Style::new().dim()));
    }
    spans.push(Span::styled(
        format!(
            "   {}/{} days · {} today",
            app.first_key(Action::PrevDay),
            app.first_key(Action::NextDay),
            app.first_key(Action::Today)
        ),
        Style::new().dim(),
    ));
    // Reminders only make sense against the real clock, i.e. for today.
    if app.date == app.today {
        let (overdue, upcoming) = app.store.reminders(app.today, app.now);
        let reminder = overdue
            .map(|t| (t, "overdue", Style::new().fg(theme.overdue).bold()))
            .or_else(|| upcoming.map(|t| (t, "next", Style::new().fg(theme.reminder))));
        if let Some((task, word, style)) = reminder {
            let time = task
                .remind
                .map(|t| t.format("%H:%M").to_string())
                .unwrap_or_default();
            spans.push(Span::styled(
                format!("   ⏰ {word} {time} {}", app.display_text(task)),
                style,
            ));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn task_item(
    task: &Task,
    viewed: NaiveDate,
    today: NaiveDate,
    now: NaiveTime,
    theme: &Theme,
    masked: bool,
) -> ListItem<'static> {
    let checkbox = if task.done { "[x] " } else { "[ ] " };
    let (marker, marker_style) = match task.priority {
        Priority::High => ("! ", Style::new().fg(theme.high).bold()),
        Priority::Low => ("· ", Style::new().fg(theme.low)),
        Priority::None => ("  ", Style::new()),
    };
    let text_style = if task.done {
        Style::new().dim().add_modifier(Modifier::CROSSED_OUT)
    } else if task.priority == Priority::High {
        Style::new().fg(theme.high)
    } else {
        Style::new()
    };
    let mut spans = vec![
        Span::raw(checkbox),
        Span::styled(marker, marker_style),
        Span::styled(
            if masked {
                crate::model::MASK.to_string()
            } else {
                task.text.clone()
            },
            text_style,
        ),
    ];
    if let Some(repeat) = task.repeat {
        spans.push(Span::styled(
            format!(" ↻ {}", repeat.label()),
            Style::new().fg(theme.repeat),
        ));
    }
    if task.note.is_some() {
        spans.push(Span::styled(" ≡", Style::new().dim()));
    }
    let age = (viewed - task.created).num_days();
    if age > 0 && !task.done {
        spans.push(Span::styled(
            format!(" ({age}d)"),
            Style::new().fg(theme.age).dim(),
        ));
    }
    if let Some(remind) = task.remind {
        let style = if task.done {
            Style::new().dim()
        } else if task.is_overdue(viewed, today, now) {
            Style::new().fg(theme.overdue).bold()
        } else {
            Style::new().fg(theme.reminder)
        };
        spans.push(Span::styled(
            format!(" ⏰ {}", remind.format("%H:%M")),
            style,
        ));
    }
    ListItem::new(Line::from(spans))
}

fn render_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let tasks = app.store.tasks(app.date);
    if tasks.is_empty() {
        let add = app.first_key(Action::Add);
        let msg = if app.date == app.today {
            format!("No tasks — press {add} to add one")
        } else {
            format!("Nothing recorded for this day — press {add} to add a task")
        };
        let [centered] = Layout::vertical([Constraint::Length(1)])
            .flex(Flex::Center)
            .areas(area);
        frame.render_widget(Paragraph::new(msg).dim().centered(), centered);
        return;
    }
    let items: Vec<ListItem> = app
        .visible()
        .into_iter()
        .map(|i| {
            let masked = tasks[i].hidden && !app.reveal;
            task_item(&tasks[i], app.date, app.today, app.now, &app.theme, masked)
        })
        .collect();
    let list = List::new(items)
        .block(Block::new().padding(Padding::horizontal(1)))
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");
    let mut state = ListState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

/// One line for the border plus up to four wrapped lines of text.
fn note_panel_height(note: &str, width: u16) -> u16 {
    let inner = usize::from(width.saturating_sub(4)).max(1);
    let lines: usize = note
        .lines()
        .map(|l| l.chars().count().div_ceil(inner).max(1))
        .sum();
    1 + lines.clamp(1, 4) as u16
}

fn render_note(frame: &mut Frame, area: Rect, note: &str) {
    let block = Block::new()
        .borders(Borders::TOP)
        .title(" note ")
        .title_style(Style::new().dim())
        .border_style(Style::new().dim())
        .padding(Padding::horizontal(1));
    frame.render_widget(
        Paragraph::new(note).wrap(Wrap { trim: false }).block(block),
        area,
    );
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
    let (done, total) = app.store.progress(app.date);
    let progress_style = if total > 0 && done == total {
        Style::new().fg(app.theme.today).bold()
    } else {
        Style::new().bold()
    };
    let k = |a| app.first_key(a);
    let (right, right_style) = match (&app.mode, &app.status) {
        (Mode::ConfirmDelete(idx), _) => {
            let text = app
                .store
                .tasks(app.date)
                .get(*idx)
                .map(|t| app.display_text(t))
                .unwrap_or_default();
            (
                format!("Delete \"{text}\"? y/n"),
                Style::new().fg(app.theme.overdue).bold(),
            )
        }
        // A status message (e.g. "Invalid time") always wins over key hints.
        (_, Some(status)) => (status.clone(), Style::new().dim()),
        (Mode::Insert(e), _) => (
            match e.target {
                Target::Reminder(_) => "e.g. 14:30 · empty clears · Enter save · Esc cancel",
                Target::Note(_) => "Enter save · empty clears · Esc cancel",
                Target::NewList => "name the new list · Enter create · Esc cancel",
                _ => "Enter save · Esc cancel",
            }
            .to_string(),
            Style::new().dim(),
        ),
        (Mode::Calendar { moving, .. }, _) => (
            format!(
                "h/l day · j/k week · [ ] month · t today · Enter {} · Esc close",
                if moving.is_some() {
                    "move here"
                } else {
                    "jump"
                }
            ),
            Style::new().dim(),
        ),
        (Mode::Search { .. }, _) => (
            "type to search · ↑/↓ select · Enter jump · Esc close".to_string(),
            Style::new().dim(),
        ),
        (Mode::Command(_), _) => (
            ":q quit · :w save · :newlist NAME · Esc cancel".to_string(),
            Style::new().dim(),
        ),
        (Mode::Help { .. } | Mode::Review { .. }, _) => {
            ("j/k scroll · Esc close".to_string(), Style::new().dim())
        }
        _ => (
            format!(
                "{} add · {} edit · {} del · {} done · {} prio · {} search · {} help · :q quit",
                k(Action::Add),
                k(Action::Edit),
                k(Action::Delete),
                k(Action::Toggle),
                k(Action::Priority),
                k(Action::Search),
                k(Action::Help),
            ),
            Style::new().dim(),
        ),
    };
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Length(12), Constraint::Min(0)]).areas(area);
    frame.render_widget(
        Paragraph::new(Span::styled(
            format!(" {done}/{total} done"),
            progress_style,
        )),
        left_area,
    );
    frame.render_widget(
        Paragraph::new(Span::styled(right, right_style)).right_aligned(),
        right_area,
    );
}

fn render_help(frame: &mut Frame, area: Rect, app: &App, scroll: u16) {
    let key_style = Style::new().fg(app.theme.accent).bold();
    let mut lines: Vec<Line> = Action::TABLE
        .iter()
        .map(|(action, _, _, desc)| {
            Line::from(vec![
                Span::styled(format!("{:>16}  ", app.keymap.keys_for(*action)), key_style),
                Span::raw(*desc),
            ])
        })
        .collect();
    let extra: &[(&str, &str)] = &[
        ("", ""),
        (
            "Calendar",
            "h/l day · j/k week · [ ] month · t today · Enter · Esc",
        ),
        (
            "Typing",
            "←/→ Home End · Backspace/Del · Enter save · Esc cancel",
        ),
        ("Search", "type · ↑/↓ select · Enter jump · Esc close"),
        ("Config", "keys and colours: see config.example.toml"),
    ];
    lines.extend(extra.iter().map(|(k, v)| {
        Line::from(vec![
            Span::styled(format!("{k:>16}  "), Style::new().dim()),
            Span::styled(*v, Style::new().dim()),
        ])
    }));
    let height = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
    let popup = centered(area, 74, height);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).scroll((scroll, 0)).block(
            Block::bordered()
                .title(" Keys ")
                .padding(Padding::horizontal(1)),
        ),
        popup,
    );
}

fn render_search(frame: &mut Frame, area: Rect, app: &App) {
    let Mode::Search { editor, selected } = &app.mode else {
        return;
    };
    let results = app.search_results();
    let width = area.width.saturating_sub(4).min(80);
    let height = (results.len() as u16 + 4)
        .max(6)
        .min(area.height.saturating_sub(2));
    let popup = centered(area, width, height);
    frame.render_widget(Clear, popup);
    let block = Block::bordered()
        .title(" Search all days ")
        .padding(Padding::horizontal(1));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let [input, _, list_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("/ ", Style::new().fg(app.theme.accent).bold()),
            Span::raw(editor.text()),
        ])),
        input,
    );
    frame.set_cursor_position((
        (input.x + 2 + editor.cursor as u16).min(input.right().saturating_sub(1)),
        input.y,
    ));

    if results.is_empty() {
        let msg = if editor.buf.is_empty() {
            "Type to search task text and notes"
        } else {
            "No matches"
        };
        frame.render_widget(Paragraph::new(msg).dim(), list_area);
        return;
    }
    let items: Vec<ListItem> = results
        .iter()
        .map(|&(date, idx)| {
            let task = &app.store.tasks(date)[idx];
            let date_style = if date == app.today {
                Style::new().fg(app.theme.today)
            } else {
                Style::new().fg(app.theme.other_day)
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{}  ", date.format("%a %d %b")), date_style),
                Span::raw(if task.done { "[x] " } else { "[ ] " }),
                Span::styled(
                    app.display_text(task),
                    if task.done {
                        Style::new().dim()
                    } else {
                        Style::new()
                    },
                ),
            ]))
        })
        .collect();
    let list = List::new(items)
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");
    let mut state = ListState::default().with_selected(Some(*selected));
    frame.render_stateful_widget(list, list_area, &mut state);
}

fn render_review(frame: &mut Frame, area: Rect, app: &App, scroll: u16) {
    let theme = &app.theme;
    let stats = app.store.stats(app.today);
    let avg = stats.done_last_7_days as f32 / 7.0;
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!(
                "Streak: {} day{}",
                stats.streak,
                if stats.streak == 1 { "" } else { "s" }
            ),
            Style::new().fg(theme.today).bold(),
        ),
        Span::raw("   ·   "),
        Span::styled(
            format!(
                "{} done in the last 7 days ({avg:.1}/day)",
                stats.done_last_7_days
            ),
            Style::new().bold(),
        ),
        Span::raw("   ·   "),
        Span::styled(
            format!("{} open", stats.open),
            if stats.open > 0 {
                Style::new().fg(theme.age).bold()
            } else {
                Style::new().bold()
            },
        ),
    ])];
    let postponed: Vec<String> = app
        .store
        .open_tasks(app.today)
        .into_iter()
        .filter(|(_, t)| t.created < app.today)
        .take(3)
        .map(|(_, t)| {
            format!(
                "{} ({}d)",
                app.display_text(t),
                (app.today - t.created).num_days()
            )
        })
        .collect();
    lines.push(Line::from(vec![
        Span::styled("Most postponed: ", Style::new().dim()),
        Span::styled(
            if postponed.is_empty() {
                "nothing — every task was written today or later".to_string()
            } else {
                postponed.join(" · ")
            },
            Style::new().fg(theme.age),
        ),
    ]));
    lines.push(Line::raw(""));

    for i in 0..7 {
        let date = app.today - Days::new(i);
        let completed = app.store.completed_on(date);
        let open: Vec<&Task> = if i == 0 {
            app.store.tasks(date).iter().filter(|t| !t.done).collect()
        } else {
            Vec::new()
        };
        let mut header = vec![
            Span::styled(format!("{}", date.format("%a %d %b")), Style::new().bold()),
            Span::styled(
                format!(" · {}", relative_label(date, app.today)),
                Style::new().fg(if i == 0 { theme.today } else { theme.other_day }),
            ),
            Span::styled(format!("    {} done", completed.len()), Style::new().dim()),
        ];
        if !open.is_empty() {
            header.push(Span::styled(
                format!(" · {} open", open.len()),
                Style::new().fg(theme.age),
            ));
        }
        lines.push(Line::from(header));
        for t in completed {
            lines.push(Line::from(vec![
                Span::styled("  ✔ ", Style::new().fg(theme.done_day)),
                Span::raw(app.display_text(t)),
            ]));
        }
        for t in open {
            let age = (app.today - t.created).num_days();
            let mut spans = vec![Span::raw("  ○ "), Span::raw(app.display_text(t))];
            if age > 0 {
                spans.push(Span::styled(
                    format!(" ({age}d)"),
                    Style::new().fg(theme.age),
                ));
            }
            lines.push(Line::from(spans));
        }
    }

    let width = area.width.saturating_sub(4).min(80);
    let height = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
    let popup = centered(area, width, height);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false })
            .block(
                Block::bordered()
                    .title(" Weekly review ")
                    .padding(Padding::horizontal(1)),
            ),
        popup,
    );
}

/// Converts a chrono date to the `time` crate's date used by the calendar
/// widget. `None` for dates outside `time`'s ±9999 year range, which chrono
/// (and therefore a hand-edited data file) can represent.
fn to_time_date(date: NaiveDate) -> Option<time::Date> {
    use chrono::Datelike;
    time::Date::from_ordinal_date(date.year(), date.ordinal() as u16).ok()
}

fn render_calendar(frame: &mut Frame, area: Rect, app: &App, cursor: NaiveDate, moving: bool) {
    let theme = &app.theme;
    // Style each day that carries information: tasks, today, and the cursor.
    let mut events = CalendarEventStore::default();
    let mut dates: Vec<NaiveDate> = app.store.days.keys().copied().collect();
    dates.push(app.today);
    dates.push(cursor);
    for date in dates {
        let Some(time_date) = to_time_date(date) else {
            continue; // out of the widget's range; nothing to highlight
        };
        let (done, total) = app.store.progress(date);
        let mut style = match total {
            0 => Style::new(),
            _ if done == total => Style::new().fg(theme.done_day),
            _ => Style::new().fg(theme.accent).bold(),
        };
        if date == app.today {
            style = style.underlined();
        }
        if date == cursor {
            style = style.add_modifier(Modifier::REVERSED);
        }
        events.add(time_date, style);
    }
    let Some(display_date) = to_time_date(cursor) else {
        let popup = centered(area, 50, 4);
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new("This date is outside the calendar's range (years ±9999).")
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title(" Calendar ")),
            popup,
        );
        return;
    };
    let monthly = Monthly::new(display_date, events)
        .show_month_header(Style::new().bold())
        .show_weekdays_header(Style::new().dim())
        .show_surrounding(Style::new().dim());

    // 21 columns of calendar + padding + borders; 8 rows of calendar + info + hint.
    let popup = centered(area, 50, 13);
    frame.render_widget(Clear, popup);
    let block = Block::bordered()
        .title(if moving {
            " Move task to… "
        } else {
            " Calendar "
        })
        .padding(Padding::horizontal(1));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let [cal_area, _, info_area, hint_area] = Layout::vertical([
        Constraint::Length(8),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let [cal_area] = Layout::horizontal([Constraint::Length(monthly.width())])
        .flex(Flex::Center)
        .areas(cal_area);
    frame.render_widget(monthly, cal_area);

    let (done, total) = app.store.progress(cursor);
    let summary = match total {
        0 => "no tasks".to_string(),
        _ => format!("{} open · {done} done", total - done),
    };
    let info = Line::from(vec![
        Span::styled(cursor.format("%a %d %b").to_string(), Style::new().bold()),
        Span::raw(" · "),
        Span::styled(
            relative_label(cursor, app.today),
            Style::new().fg(theme.other_day),
        ),
        Span::raw(" · "),
        Span::raw(summary),
    ]);
    frame.render_widget(Paragraph::new(info).centered(), info_area);
    frame.render_widget(
        Paragraph::new(if moving {
            "Enter move here · [ ] month · Esc"
        } else {
            "Enter jump · [ ] month · Esc"
        })
        .dim()
        .centered(),
        hint_area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_time_date_is_fallible_for_extreme_years() {
        assert!(to_time_date(NaiveDate::from_ymd_opt(2026, 8, 31).unwrap()).is_some());
        assert!(to_time_date(NaiveDate::from_ymd_opt(12000, 1, 1).unwrap()).is_none());
    }
}
