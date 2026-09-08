# tui-do-list

A daily to-do list for the terminal, modelled on a paper notebook.

Every day gets its own page. You write down what you want to do, tick things
off as you go, and anything you did not finish is carried over to the next day
automatically — the same "migration" habit from bullet journaling, minus the
rewriting. Past pages keep only what you actually completed, so browsing back
shows a clean log of what got done.

Built in Rust with [ratatui](https://ratatui.rs). Single binary, one JSON file
per list, no daemon, no network.

```
 Mon, 31 Aug 2026 · Today  [personal] work   h/l days · t today   ⏰ next 14:30 Send the invoice
 > [ ] ! Send the invoice (3d) ⏰ 14:30
   [x]   Standup ↻ weekdays
   [ ] · Buy milk ≡
   [ ]   Review pull request #42

 note ─────────────────────────────────────────────────────────────────────────
 semi-skimmed, two litres
 1/4 done      a add · e edit · d del · space done · p prio · r remind · / search · w review · ? help · :q quit
```

## Features

- **One page per day** — navigate with `←`/`→`, jump back with `t`, or pick a
  day in the **calendar** (`c`).
- **Automatic carry-over** — unfinished tasks from past days move to the front
  of today's list on startup (and at midnight if you leave the app open). Each
  carried task shows how old it is, e.g. `(3d)`.
- **Plan ahead** — add tasks to a future day; **postpone** a task to tomorrow
  with `>` (from the viewed day) or **move it to any date** with `m`.
- **Reminders** — `r` sets a time (`14:30`, `14h30`, `14`). The task shows
  `⏰ 14:30`, the header shows today's next reminder, and when the time passes
  it turns red and a message flashes in the footer. A carried-over reminder
  fires again at its time each day until you complete the task. Optional
  desktop notifications via a systemd timer (see below).
- **Recurring tasks** — `R` cycles daily → weekdays → weekly → monthly. When
  you complete one, its next occurrence is created on the right day.
- **Notes** — `n` attaches a note to a task; it appears in a panel while the
  task is selected and is marked with `≡` in the list.
- **Hidden tasks** — `H` masks a task's text as `••••••••` everywhere on
  screen (list, review, search, delete prompts, desktop notifications); `v`
  reveals hidden tasks until you press it again. This is screen privacy for
  shoulder-surfing, **not encryption** — the JSON data file still contains the
  text in plain form.
- **Priorities** — `p` cycles none → low (`·`) → high (`!`).
- **Sort view** — `s` toggles between your manual order and "overdue reminders
  and high priority first, done last".
- **Focus view** — `f` hides completed tasks so the page shows only what is
  left (`hide_done = true` starts that way). Progress and history are
  unaffected; it is a view, not a filter on the data.
- **Search** — `/` searches text and notes across every day; `Enter` jumps to
  the hit.
- **Weekly review** — `w` shows your streak, completions per day for the last
  7 days, what is still open, and the most-postponed tasks.
- **Multiple lists** — e.g. `personal` and `work`, each in its own file.
  `L` opens a **list picker** showing every list with its open-task count and
  a `+ New list` row; `Tab` cycles, `N` (or `:newlist NAME`) creates one from
  inside the app.
- **Split view** — `|` shows the next list in a read-only pane on the right,
  for the same day and under the same sort and focus settings. `Tab` moves
  the active list along, so the pane always shows what comes next. Needs a
  terminal at least 60 columns wide; `split = true` starts that way.
- **Undo delete** (`u`), optional **delete confirmation**, a **progress line**
  in the footer, and **configurable keys and colours** with five built-in
  **theme presets** (`default`, `nord`, `dawn`, `matrix`, `slate`).
- **Export to Markdown** — `tui-do-list export --week`.
- **Safe storage** — every change is written atomically.

## Install

Requires a Rust toolchain (1.88 or newer, edition 2024).

```sh
git clone <this repository>
cd tui-do-list
./build.sh install        # runs checks + tests, then cargo install --path .
```

`build.sh install` tells you if `~/.cargo/bin` is missing from your `PATH`.
Then run `tui-do-list`.

To try it without touching your real data:

```sh
TUI_DO_LIST_FILE=/tmp/todo-playground.json tui-do-list
```

## Keys

Defaults; every key in the first table can be changed in the config file.

### Task list

| Key | Action |
|---|---|
| `j` / `k`, `↓` / `↑` | move the selection |
| `g` / `G` | first / last task |
| `a` | add a task |
| `e` | edit the selected task |
| `space` / `x` / `Enter` | toggle done (a recurring task spawns its next occurrence) |
| `d` | delete (`u` undoes, restoring it to its original day and position) |
| `J` / `K` | move the task down / up (manual order only) |
| `p` | cycle priority: none → low → high |
| `r` | set a reminder time (empty clears it) |
| `R` | cycle repeat: daily → weekdays → weekly → monthly → none |
| `n` | edit the task's note (empty clears it) |
| `H` | hide/unhide the task (masks its text on screen) |
| `v` | reveal hidden tasks for this session |
| `>` | postpone to the next day |
| `m` | move to a date — pick it in the calendar |
| `s` | toggle the priority sort |
| `f` | focus: hide / show completed tasks |
| `/` | search all days |
| `w` | weekly review and stats |
| `h` / `l`, `←` / `→` | previous / next day |
| `t` | jump back to today |
| `c` | open the calendar |
| `Tab` | switch to the next list |
| `L` | list picker: switch to any list, or create one from the `+ New list` row |
| `\|` | split view: the next list read-only in a right-hand pane |
| `N` | create a new list (prompts for the name) |
| `?` | key reference (scrolls with `j`/`k`) |
| `:` | command line — `:q` (also `:q!`, `:wq`) quits, `:w` saves now, `:newlist NAME` creates a list |
| `Ctrl-C` | quit directly (everything is always saved) |

Plain `q`/`Esc` do **not** quit from the task list (popups still close with
them). To restore direct quitting, set `quit = "q esc"` under `[keys]` in the
config.

### In the calendar

| Key | Action |
|---|---|
| `h` / `l`, `←` / `→` | previous / next day |
| `j` / `k`, `↓` / `↑` | next / previous week |
| `[` / `]`, `PgUp` / `PgDn` | previous / next month |
| `t` | today |
| `Enter` | jump to that day (or move the task there, after `m`) |
| `Esc` | close |

Days with open tasks are yellow, fully completed days green, today underlined.

### In the list picker

| Key | Action |
|---|---|
| `j` / `k`, `↓` / `↑`, `Tab` | move between lists |
| `g` / `G` | first list / the `+ New list` row |
| `Enter` | switch to the list, or open the new-list prompt on the last row |
| `N` | new-list prompt directly |
| `Esc`, `q`, `L` | close |

### In search, review and help

`/` opens a live search: type to filter, `↑`/`↓` pick a result, `Enter` jumps
to it, `Esc` closes. Review (`w`) and help (`?`) scroll with `j`/`k` and
`PgUp`/`PgDn`; `Esc` closes.

### While typing

`←`/`→`/`Home`/`End` move the cursor, `Backspace`/`Delete` erase, `Enter`
saves, `Esc` cancels. An empty task text cancels; an empty reminder or note
clears it. Reminder formats: `14:30`, `14h30`, `1430`, `14h`, `14`.

## How days work

- A task belongs to the date it is listed under and remembers the date it was
  **created**. The two differ once it has been carried over or postponed.
- On startup, every unfinished task from any day *before* today is moved to the
  front of today's list, oldest first. Today's own tasks follow.
- Past days keep only their completed tasks, so browsing back shows what you
  got done. The weekly review uses each task's completion date.
- Future days are never touched by carry-over, so they work as a planner.
  Postponing (`>`, `m`) and recurring tasks put tasks there for you.
- Reminders are compared against the clock only on today's page. A carried-over
  task keeps its reminder and shows it as overdue until you finish it.
- If the app is open across midnight it notices within a second, switches the
  view to the new day and carries tasks over.

## Configuration

Optional file at `~/.config/tui-do-list/config.toml` (or `$TUI_DO_LIST_CONFIG`).
See [`config.example.toml`](config.example.toml) for every setting with
comments. Summary:

```toml
lists = ["personal", "work"]   # Tab cycles; each is <name>.json in the data dir
                                # (`N` / `:newlist NAME` in the app appends here)
sort_by_priority = false        # start with the sort switched on
hide_done = false               # start in focus view (completed tasks hidden)
split = false                   # start with the read-only side pane open
confirm_delete = false          # ask y/n before deleting
notify_window_secs = 60         # how far back `tui-do-list notify` looks
theme = "default"               # default, nord, dawn, matrix, slate

[colors]                        # ratatui names, "#rrggbb" or ANSI index;
high = "red"                    # applied on top of the theme preset:
overdue = "light_red"           # high, low, today, other_day, reminder,
                                # overdue, accent, age, repeat, done_day

[keys]                          # space-separated; replaces the defaults
add = "i"
toggle = "space enter"
```

A malformed file, an unknown theme, colour or key name, or a key bound twice
is reported at startup instead of being silently ignored.

## Desktop notifications (optional)

The TUI shows reminders itself. If you also want a desktop notification when
the app is closed, `tui-do-list notify` sends one (via `notify-send`) for every
reminder that became due in the last `notify_window_secs`. Run it every
minute with the bundled systemd user timer:

```sh
./build.sh install-timer                       # copies contrib/*.service|timer and enables it
systemctl --user disable --now tui-do-list-notify.timer   # to remove
```

## Command line

```
tui-do-list                       open the TUI
tui-do-list export [RANGE]        print tasks as Markdown
                                (--day [DATE] | --week | --all; --show-hidden unmasks hidden tasks)
tui-do-list notify [--window S]   notify reminders due in the last S seconds
tui-do-list --help | --version
```

Export example:

```markdown
## Mon, 31 Aug 2026

- [ ] Send the invoice **!** ⏰ 14:30
  > ask about the PO number
- [x] Standup ↻ weekdays
```

## Data files

| | Path |
|---|---|
| default | `$XDG_DATA_HOME/tui-do-list/<list>.json` (usually `~/.local/share/tui-do-list/tasks.json`) |
| override | `TUI_DO_LIST_FILE=/path/to/tasks.json` (single list) |

Each file is rewritten after every change using write-to-temp-then-rename, so
a crash or power cut can never leave it half-written (the data is fsynced
before the rename). A corrupt file is reported as an error and never
overwritten. A lock file (`<list>.lock`, removed on exit) keeps a second
instance from silently overwriting your changes; if the app crashed and the
lock is stale it is taken over automatically.

```json
{
  "version": 1,
  "next_id": 4,
  "days": {
    "2026-08-31": [
      {
        "id": 1,
        "text": "Send the invoice",
        "done": false,
        "priority": "high",
        "created": "2026-08-28",
        "remind": "14:30:00",
        "note": "ask about the PO number",
        "repeat": "weekdays"
      }
    ]
  }
}
```

`completed`, `remind`, `note`, `repeat` and `hidden` are present only when
set. Files written by older versions load unchanged. Note that `hidden` only
masks a task on screen — the text is stored unencrypted in this file.

## Development

```sh
./build.sh            # fmt check + clippy + tests + release build
./build.sh install    # …then cargo install
./build.sh quick      # release build only
./build.sh clean
```

```
src/
  main.rs     entry point, subcommand dispatch, event loop, save on exit
  cli.rs      `export` and `notify`
  config.rs   config.toml: lists, Theme (colours), Keymap (Action ↔ keys)
  model.rs    Task / Store: carry-over, recurrence, search, sort, stats
  storage.rs  data-file paths per list, load(), atomic save()
  app.rs      App state, modes, key handling, undo, list switching
  ui.rs       rendering: header, list, note panel, footer, popups
contrib/      systemd user units for desktop notifications
```

The model, config and CLI layers are pure and unit-tested; the UI only reads
the `App`.

## License

MIT
