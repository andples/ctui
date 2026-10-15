# ctui — design plan

A minimal terminal calendar and task list. One binary, `ctui` (alias it as
`ct`). Run with no arguments and you get the TUI. Run with arguments and it's a
plain command-line tool that does the same things. Everything stays on your
machine: no accounts, no server, no Google. You can bring a Google calendar
over once with `ct import`.

Status (2026-10-16): M0–M9 are done (first frame 21 ms and a 5.8 MB binary
with 2,000 items); ctui is MIT and 0.1.0 is on crates.io. Where the build differs from this plan, the
plan was updated.

---

## 1. Principles

These decide every argument later. When a feature conflicts with them, the
feature loses.

1. **Usable first.** Someone opening it for the first time can add, tick off
   and find things without reading docs. The key line always shows the keys
   for where you are, `?` shows the rest, and every action can be undone.
2. **One grammar everywhere.** The quick-add language (`dentist fri 3pm 1h #health`)
   is the same in `ct add`, the TUI's add bar and the `:` command line. Learn
   it once.
3. **CLI and TUI are equals.** Anything you can do in the TUI you can do with a
   command, and the other way round. Both are thin layers over one core.
4. **No bloat.** No daemon, no plugins, no server, no account, no notes or
   markdown editor, no kanban, no time tracking. A small binary that starts
   instantly.
5. **Your data is plain files in an open format.** Standard iCalendar
   (`.ics`), one file per item (§4). It's readable by other tools and syncable
   with Syncthing, git or vdirsyncer, and there's never lock-in, ours included.
6. **Customizable, not configurable to death.** Theme, keys, date/time format,
   week start, which views exist and the default view. Sensible defaults mean
   most people never open the config.
7. **Nothing to save.** Every change is written immediately. No "save?"
   prompts, no lost edits.

Inspiration taken from ttyp: the `:` command palette with fuzzy completion, the
key-hint line, TOML themes, one look for every list (` › ` marker), config
saved on change, a UI-free core, and time injected (`*_at(now)`) so tests are
deterministic. Where ctui should do better than ttyp: fewer screens, every key
visible in context, undo for everything, and a live preview of what you're
typing.

---

## 2. Concepts (only three)

| Concept | What it is | iCalendar |
|---|---|---|
| **Task** | Something to tick off. Optional due date or time, priority, repeat, subtasks. | `VTODO` |
| **Event** | Something that happens at a time: timed or all-day, optional repeat. | `VEVENT` |
| **List** | A named group (`personal`, `work`) with a colour. Tasks and events both belong to one. | a vdir collection (a folder) |

That's all of them. There are no tags, projects, contexts, areas or
priorities beyond `!`/`!!`/`!!!`. `#name` picks the list.

Subtasks: a task can have checklist children, one level deep (`RELATED-TO`).
They're shown collapsed under the parent with a `2/5` count.

---

## 3. Views

Three views. `1` `2` `3` (or `tab`) switches between them. Each can be hidden in
the config, and the default view is configurable.

### 3.1 Today (default): tasks and calendar together

(Called Agenda until 2026-10-16; the owner renamed it. Old config values
`agenda`, `agenda_days` and `ct ag` still work.)

```
 ctui                                         Wed 15 Oct 2026
 ─────────────────────────────────────────────────────────────
 Overdue
   ☐  renew passport                 due Mon 13      #personal
 Today · Wed 15
   09:30–10:00  standup                               #work
 › ☐  pay rent                                       #personal !
   14:00–15:00  dentist                               #health
   ☑  buy milk                                       #personal
 Tomorrow · Thu 16
   ☐  send invoice                   17:00            #work
 Sat 18
   all day      Sam's birthday                        #personal
 ─────────────────────────────────────────────────────────────
 a add  x done  e edit  d delete  u undo  / search  ? help
```

- Overdue tasks are at the top (they collapse after N, configurable). Then
  today, then the next days that have anything in them. Empty days are
  skipped. Undated tasks are reached with `T` (or the Tasks view).
- Events and tasks are mixed in time order. Timed items come first, then
  all-day ones, then tasks due that day.
- Done items stay visible (dimmed) for the rest of the day, so you see what you
  got done. After that they drop out (`hide_done_after`).

### 3.2 Month: the calendar

```
 October 2026                      │ Wed 15
  Mo  Tu  We  Th  Fr  Sa  Su       │ 09:30 standup
            1   2   3   4          │ 14:00 dentist
   5   6   7   8   9  10· 11       │ ☐ pay rent
  12  13• 14 [15]•16• 17  18•      │ ☑ buy milk
  19  20  21  22  23  24  25       │
  26  27  28  29  30  31           │
```

- A grid with dots for busy days (in the list colour, and denser when
  busier), plus a pane for the selected day. On narrow terminals the pane
  moves below the grid; on wide ones the cells get room for the first event
  titles.
- `h/j/k/l` or the arrows move the day, `J`/`K` pick an item in the day's
  list (for `x`, `e`, `d`…), `[`/`]` change month, `t` is today,
  `g` goes to a date (typed with the same grammar: `g dec 24`, `g +2w`),
  `enter` opens that day in the Agenda.

### 3.3 Tasks: the checklist

```
 personal (4)   work (2)   health (1)   all (7)
 ─────────────────────────────────────────────
 › ☐  pay rent                     today     !
   ☐  renew passport               Mon 13 ⚠
   ▸ ☐  plan trip  2/5
   ☐  call grandma
```

- Tabs per list (`H`/`L` or `←`/`→` on the tab row), plus an "all" tab.
  Sorting by due date, then priority, then created, is the default;
  `s` cycles through due / priority / manual.
- `space` expands subtasks. `J`/`K` reorders when the sort is manual. `>` and
  `<` indent a task under the one above, or outdent it.

### 3.4 Search: one search across everything

**Decision (owner, 2026-10-16):** `/` searches every task and event, on
every page, instead of filtering the page you're on (the old per-page
filter is gone).

```
 Search everything

 › ┃ dentist              Sat 14:00–15:00          #health
   ☐ dentist bill         tomorrow                 #home
   ┃ dentist              Tue 3 Mar 09:00–09:30    #health
   ☑ book dentist         Mon 12 Oct               #health
 ─────────────────────────────────────────────────────────────
 /dentist
   4 matches · enter jumps to it · esc goes back
```

- Any date past or future, any list, undated tasks, done ones too (dimmed,
  ranked lower). Every query word must be in the title or the `#list`.
  An empty query shows a hint, not everything.
- Ranked by how well the title matches, then open before done, then the
  date nearest today (upcoming first, then undated, then past). The list
  stops at 200 with `+N more`.
- A repeating event shows its next occurrence from today, or its last one
  if the series has ended.
- `↑` `↓` (and `ctrl+n` `ctrl+p`, since letters go into the query) pick a
  result; `enter` or a click jumps to it; `esc` goes back to where you were.
- **Jumping:** an undated task (or a subtask) opens in Tasks on the "all"
  tab, its parent expanded. A dated task or an event opens on the Today
  page starting at its day, with that exact entry (that occurrence)
  selected. If that page is turned off, another visible page is used. A
  done item that's past `hide_done_after` isn't on any page, so a toast
  says so.
- The query logic is `core::query::search`, shared with `ct search`.

**Not in v1:** an hour-by-hour week grid. In a terminal the agenda is more
readable. It might come later as a fourth, optional view if it's actually
missed.

---

## 4. Storage

**Decision (owner, 2026-10-15): iCalendar files in a vdir.** One `.ics` file
per item, one folder per list:

```
~/.local/share/ctui/            (data_dir, configurable)
  personal/
    displayname                 the list's name as shown (optional)
    color                       the list's colour, e.g. #88c0d0 (optional)
    3f9c…e1.ics                 one VTODO or VEVENT
  work/
    …
```

Why this over SQLite, JSON or our own format:

- **Sync for free, without Google.** It's exactly the layout `vdirsyncer`
  uses, so pointing `data_dir` at a vdirsyncer storage syncs with any CalDAV
  server (Radicale, Nextcloud, Fastmail, iCloud). That's zero sync code in
  ctui. Syncthing or git also work, and conflicts are per item, never one big
  file.
- **Import and export are trivial**, because the native format *is* the
  exchange format.
- **Works alongside other tools:** khal, todoman, Thunderbird (via a CalDAV
  server).
- **No lock-in**, ctui included.

The `displayname` and `color` files follow vdirsyncer's `metasync` convention,
so a calendar's name and colour on a CalDAV server arrive in ctui (and the
other way round) with no extra code. There's no ctui-only metadata file.

Rules:

- **Round-trip safety.** Properties ctui doesn't understand are kept
  untouched on write. Editing a title in ctui must never delete another
  client's data. This needs tests with real-world files (Google, Apple,
  Thunderbird, Nextcloud exports).
- **Atomic writes:** write `uid.ics.tmp` → fsync → rename.
- **External changes:** the TUI re-scans when files change (poll the dir
  mtimes every ~2 s while open). That's cheap and needs no watcher dependency.
- **Index:** load everything into memory at start-up. Thousands of small files
  parse in milliseconds. If that ever stops being true, add a cache
  (`state_dir/index.bin`, keyed by mtime) rather than switching formats.
- **Times:** timed items store `TZID` with the local zone's IANA name. All-day
  items use `VALUE=DATE`. Floating times (no zone) from imports, and unknown
  zone names (Outlook's Windows names), are shown as local. ctui doesn't write
  VTIMEZONE blocks for its own items (khal, Radicale and Thunderbird resolve
  IANA names); imported items keep the VTIMEZONEs they came with.
- **Deleting** removes the file and keeps a copy in `state_dir/trash/` so
  undo works. Trash is emptied after 30 days.

Other paths (XDG via `directories`):

| | Path | Contents |
|---|---|---|
| config | `~/.config/ctui/config.toml`, `themes/*.toml` | settings, user themes |
| data | `~/.local/share/ctui/` | the vdir (above) |
| state | `~/.local/state/ctui/` | undo journal, trash, last listing (for CLI numbers) |

---

## 5. The quick-add grammar

Used by `ct add`, the TUI add bar (`a`), edit (`e`, prefilled) and `g` (go to
date). Words that aren't tokens make up the title. Order doesn't matter.

| Token | Meaning | Examples |
|---|---|---|
| date | due date (task) or day (event) | `today` `tod` `tomorrow` `tom` `fri` `next fri` `oct 20` `20/10` `2026-10-20` `+3d` `+2w` `eom` |
| time | due time, or start time | `3pm` `15:00` `9.30` `noon` |
| range / length | **makes it an event** | `3-4pm` `14:00-15:30` `3pm 1h` `for 45m` |
| `allday` / date span | all-day event | `allday sat` `oct 20-22` |
| `#list` / `@list` | list (fuzzy, must exist, or `#new!` to create it). `@` because shells read `#` as a comment | `#work` `@pers` |
| `!` `!!` `!!!` | priority | |
| `every …` | repeat | `every day` `every weekday` `every 2 weeks` `every month on 1` `every year` |
| `"…"` | literal title text, so `"fri"` stays a word | |

**Task or event?** It's an event if it has a time range, a length or `allday`.
Otherwise it's a task. `ct add task …` / `ct add event …` (TUI: `A` = event)
force one or the other. No other rules to learn.

**Live preview.** While you type in the add bar, the line under it shows the
parse result: `task · due Fri 17 Oct 15:00 · #work · !` or
`event · Sat 18 Oct 14:00–15:00 · #health`. Unrecognised tokens just become
title text. The preview is what makes the grammar safe to use.

The parser is written in-house (no NLP dependency) and is small, strict and
heavily tested. Dates resolve forward: `fri` on a Friday means today, and
`next fri` means a week later. Week start and date order (`20/10` vs `10/20`)
come from the config.

---

## 6. CLI

```
ct                              open the TUI
ctui --view today|month|tasks   open the TUI on that page (piped: print it)
ctui init bash|zsh|fish         print the shortcuts (ct, ctt, ctm, cttasks) for an rc file
ct add <text>                   add (task or event, by the grammar)
ct ls [today|week|overdue|all|#list|<query>]     list tasks (default: open, due ≤ today + undated)
ct today [days]                 Today: overdue, today and the next days (aliases: ag, agenda)
ct search <words>               every task and event, any date, done too (alias: find; §3.4)
ct cal [month]                  print a month grid (like `cal`, with dots)
ct done <ref>…                  tick off (recurring tasks advance)
ct undone <ref>
ct edit <ref> [text]            re-parse text onto the item, or open $EDITOR on the .ics
ct mv <ref> <date|#list>        reschedule or move
ct rm <ref>…
ct undo                         undo the last change (from the CLI or the TUI)
ct lists                        list lists; `ct lists add|rename|rm`
ct import <file.ics> [#list]    one-way import (§9)
ct export [#list] > out.ics
ct remind                       print/notify reminders due now (§8); for a timer, not a daemon
```

- **Refs:** the numbers from the last `ls`/`ag`/`search` output (saved in the state dir,
  like taskwarrior's working set), *or* text matched against open items.
  `ct done milk` works when exactly one item matches. Otherwise it lists the
  matches and changes nothing.
- **Output:** plain aligned text, coloured only on a TTY, with `NO_COLOR`
  respected. `--json` on every read command for scripting. Exit codes mean
  something (0 ok, 1 no match/error, 2 ambiguous).
- Every change prints one line saying what happened:
  `added task 4 · due Fri 17 Oct · #work`, `done 4 · pay rent (next: 15 Nov)`.
- Shell completions (`ct completions bash|zsh|fish`) from clap, including list
  names.

### 6.1 Shortcuts

**Decision (owner, 2026-10-16): built-in shortcuts, three letters unless
taken.** `eval "$(ctui init zsh)"` (bash the same, fish:
`ctui init fish | source`) defines `ct` → `ctui` and one alias per page,
each `ctui --view <page>`:

- A page's name is `ct` + its first letter, or `ct` + the whole name if
  that's taken: Today `ctt`, Month `ctm`, Tasks `cttasks` (Today has `ctt`).
- Taken means an earlier page claimed it (worked out in Rust,
  `shortcuts::candidates()`, the one place the rule lives; the snippets for
  all three shells are generated from it), or the shell already has a
  command, function or alias of that name. That second check runs in the
  snippet when the shell starts (`alias` / `command -v`, fish `functions -q`
  / `type -q`), not when `ctui init` ran, so installing something later
  wins. An alias with exactly our body counts as ours, so re-sourcing the rc
  file (or an existing `alias ct=ctui`) is fine.
- `ct` has no fallback: if a different `ct` exists (Arch's `extra/ct`
  installs `/usr/bin/ct`), the snippet leaves it alone and you keep `ctui`.
  The page names still work, since they call `ctui` directly.
- Words after a shortcut are a normal command (`ctm add milk` = `ct add
  milk`): `--view` is ignored when a command is given. Keeping them plain
  aliases means no wrapper logic and completion works through the alias.
- `views` is only checked at run time: a hidden page opened with `--view`
  falls back to the first page that's on, with a toast, like `1` `2` `3`.
- Piped, `--view` prints the page's CLI equivalent (`ct today`, `ct cal`,
  `ct ls`), so the shortcuts are useful in scripts and `watch` too.
- In the TUI, `:ctt`, `:ctm`, `:cttasks`, `:cttoday` and `:ctmonth` switch
  pages, and complete.
- `ctui init` prints without loading the config or the store, because it
  runs at every shell start.

---

## 7. TUI keys

Vim keys and arrows both work everywhere. The bottom line always shows the
keys that do something right now. `?` opens the full list, generated from the
keymap, so it can't go out of date.

| Key | Action |
|---|---|
| `j/k` `↓/↑` | move |
| `1` `2` `3` / `tab` | Today / Month / Tasks |
| `a` / `A` | add (grammar; `A` forces an event); prefilled with the selected day or list |
| `x` / `space` | tick off / un-tick (on a parent: expand/collapse) |
| `e` / `enter` | edit: the add bar prefilled with the item as grammar text |
| `E` | edit in `$EDITOR` (the raw `.ics`) for the rare fields |
| `d` | delete (no confirm: undo is one key away, and a toast says "deleted · u undo") |
| `u` / `ctrl+r` | undo / redo |
| `m` | move: date or `#list`, same grammar |
| `+` `-` | reschedule +1 day / −1 day (the most common edit, so it gets one key) |
| `>` `<` | indent under the task above / outdent (Tasks view) |
| `c` | theme picker (same as `:theme`) |
| `t` | jump to today |
| `[` `]` | previous / next period |
| `g` | go to a date |
| `/` | search every task and event (§3.4); `enter` jumps to it, `esc` goes back. Config name `search` (`filter` still works) |
| `:` | command line: every CLI command works here (`:import ~/cal.ics`, `:theme nord`) |
| `?` | help |
| `q` | quit |

- **Remappable:** `[keys]` in the config maps action names to keys. `?` and
  the hint line pick up the changes.
- **Mouse** (optional, on by default): click to select, click a day, scroll,
  click the checkbox to tick it.
- **Toasts:** a one-line message at the bottom after every change, with the
  undo hint. They disappear on the next key.
- **Resizing:** layouts degrade step by step. Panes stack, then the list
  columns drop, then only titles are left. Nothing breaks at 40×12.

---

## 8. Reminders (no daemon)

`VALARM` on items (grammar: `remind 10m`, or `default_reminder` per list).
`ct remind` prints and `notify-send`s anything due since its last run, then
remembers where it got to. The README shows a systemd user timer and a cron
line that run it every minute. If you don't set either up, there are no
reminders and no background process. Inside the TUI, items due now are
highlighted.

---

## 9. Importing (one way into ctui)

`ct import file.ics [#list]`:

- Reads `.ics` files (Google Takeout / Google Calendar "Export", Apple,
  Outlook, Proton…) and directories of them, plus `.zip` Takeout archives
  (they contain one `.ics` per calendar).
- Each source calendar becomes a list unless `#list` is given.
- **Idempotent:** items are keyed by their `UID`. Re-importing a newer export
  updates items that are unchanged locally and skips ones you've edited in
  ctui (reported as such). It never creates duplicates.
- `--dry-run` shows what would be added, updated or skipped.
- The README has a short "Leaving Google Calendar" section covering Takeout,
  `ct import`, and deleting the Google calendar if you want.

Also: Google **Tasks** can't be exported as `.ics` (Takeout gives JSON).
Importing Takeout's `Tasks.json` is a small extra parser, worth it for a
complete exit. It's in milestone M6.

Export (`ct export`) writes a single standard `.ics`, so leaving ctui is just
as easy.

---

## 10. Customization

`config.toml`, with every key optional (`#[serde(default)]`). `:set key value`
changes a setting from inside the TUI and saves it right away.

```toml
theme        = "default"        # or a file in themes/
default_view = "today"          # today | month | tasks
views        = ["today", "month", "tasks"]
week_start   = "monday"
date_order   = "dmy"            # dmy | mdy | ymd (for typed dates like 20/10)
time_format  = "24h"            # 24h | 12h
days_ahead   = 7            # days the Today page shows
hide_done_after = "1d"          # done items stay visible this long
default_list = "personal"
mouse        = true
data_dir     = "~/.local/share/ctui"   # point at a Syncthing/vdirsyncer folder

[keys]                          # action = key(s); only what you change
done = ["x", "space"]
```

### 10.1 Themes

Colour matters here: it's how you tell lists apart at a glance. So themes are
first-class, but each one is still just a small TOML file.

```toml
name = "nord"

[colors]
bg    = "#2e3440"   # background ("reset" = the terminal's own)
fg    = "#d8dee9"   # text
sub   = "#616e88"   # secondary: times, hints, done items, borders
main  = "#88c0d0"   # accent: selection, today, headings
error = "#bf616a"   # overdue, delete toasts

# Lists without their own colour take these, in order.
lists = ["#88c0d0", "#a3be8c", "#d08770", "#b48ead", "#ebcb8b", "#81a1c1"]
```

- **Five colours plus a list palette.** Everything else is derived at run
  time, so a theme stays five lines. The selected row's background is `main`
  blended about 15% into `bg`. Done items use `sub`, overdue items use
  `error`, today uses `main`, and borders are `sub` blended halfway into
  `bg`. A theme can still set `select`, `done`, `overdue`, `today` or
  `border` itself to override what's derived.
- **Colours** can be `#rrggbb`, or ANSI names (`red`, `bright-black`, …,
  `reset`) that follow the terminal's palette. When a colour is an ANSI name,
  blending isn't possible, so the selected row is shown in bold with the
  ` › ` marker instead of a background.
- **List colours, in priority order:**
  1. the list's `color` file (set with `ct lists color work #hex`, or synced
     from CalDAV)
  2. a palette slot pinned with `ct lists color work 3`, which follows theme
     changes
  3. otherwise, the next unused slot of the theme's `lists`, given the first
     time ctui sees the list and remembered in the state dir, so colours
     never shuffle when a list is added. They repeat after the palette runs
     out.

  `list_colors = "theme"` in the config ignores `color` files, so everything
  follows the theme.
- **Switching:** `:theme <name>` (fuzzy, with Tab completion) applies and
  saves it straight away. `:theme` alone opens a picker that previews each
  theme on the live screen as you move through it, then `enter` keeps it and
  `esc` goes back. `ct theme ls` / `ct theme <name>` do the same from the
  shell.
- **Your own themes:** drop a file into `~/.config/ctui/themes/`. A user file
  with the same name as a built-in replaces it. A broken file is reported
  once and the default is used, so it never crashes.
- **What ships** (`themes/` in the repo, 39 themes):
  - `default`: the terminal's own 16 colours, so it matches any terminal
    theme. It's the default theme.
  - `light`: ctui's own.
  - 12 from well-known palettes (Catppuccin Latte/Mocha, Dracula, Everforest,
    gruvbox, Kanagawa, Nord, One Dark, Rosé Pine, Solarized Dark/Light,
    Tokyo Night). Their accent colours are the list palette. Credited in
    `themes/CREDITS.md`.
  - 25 adapted from **[Monkeytype](https://github.com/monkeytypegame/monkeytype)**
    (Serika, Serika Dark, 8008, Iceberg, Nautilus…), by way of ttyp. They're
    GPL-3.0 and keep that license, with the header in each file and the full
    list and mapping in `themes/MONKEYTYPE.md`. Monkeytype has no list
    colours, so those are derived from each theme's `main` (OKLCH hue steps,
    at least 3:1 contrast on `bg`).
- **Built in:** ctui is MIT, so only `default`, `light` and the 12 MIT palettes
  are compiled in; the Monkeytype files load from a themes folder.

---

## 11. Recurrence

- RRULE via the `rrule` crate. Only the grammar's common cases can be typed
  (`every …`). Anything an import brings in (EXDATE, BYSETPOS…) is displayed
  correctly and kept intact.
- **Recurring events:** expanded on the fly for the visible range. Editing one
  asks only once: `this` / `all` (two keys). "This and following" is
  deliberately left out because it's the classic source of calendar bugs.
  Editing just "this" writes a `RECURRENCE-ID` override.
- **Recurring tasks:** ticking one advances `DUE` to the next occurrence and
  records the completion (todoman's approach), so the list never fills with
  copies.

---

## 12. Architecture

A single crate, `lib.rs` plus `main.rs`, split into a workspace only if a
second consumer ever appears.

```
src/
  main.rs          clap: no command → tui::run (or the page as text when piped), else cli::run
  core/            UI-free. The only place that changes data.
    ical.rs        a small RFC 5545 reader/writer that keeps every property
    time.rs        Moment (date / floating / zoned), durations, DST-safe math
    model.rs       Item {Task|Event} with Fields; writes back only what changed
    store.rs       vdir load/save/reload, atomic writes, trash, commit()
    undo.rs        journal of file contents before/after, shared by CLI and TUI
    parse.rs       the grammar (§5) → Draft (+ spans for colouring, preview text)
    ops.rs         add, edit, done, move, shift, delete, subtasks, lists… → commit
    query.rs       agenda, day/days, tasks, busy days, matching, search
    recur.rs       expansion (rrule crate), overrides, next due, describe()
    import.rs      ics / folders / zip / Tasks.json → plan → apply; export
    remind.rs      alarms due in a window, last-run state
  mcp.rs           the MCP server: tools, fields → Draft, install, skill
  cli.rs           subcommands → core::ops; text and --json output
  session.rs       paths + config + store + journal + theme, shared by both
  shortcuts.rs     `ctui init`: the page shortcut rule and the bash/zsh/fish snippets
  theme.rs         themes, colours, list palette slots
  config.rs        Config, Paths, :set
  tui/
    app.rs         App state, rows, dispatch(Action), input, mouse, `:` commands
    keys.rs        keymap (defaults + [keys]), help table
    view.rs        drawing only
    tests.rs       TestBackend tests
```

Conventions (from ttyp, where they worked):

- Every change, from the CLI or the TUI, is an `Op` applied through
  `ops::apply`, which writes the file and the undo entry. Nothing else
  touches the store.
- In the TUI, every state change goes through `App::dispatch(Action)`.
- Functions that depend on "now" take it as an argument (`now: DateTime<Tz>`), so
  "today", "overdue" and the date parser are all testable.
- Views only render; they never change state.

### Dependencies (kept short on purpose)

`ratatui`, `crossterm`, `clap` (+ `clap_complete`), `serde`, `serde_json`
(`--json`, state files, Tasks.json), `toml`, `directories`, `anyhow`, `rrule`,
`chrono` + `chrono-tz` (needed by `rrule`), `iana-time-zone` (the local
zone's name for TZID), `uuid`, `zip` (deflate only). The iCalendar
reader/writer is hand-written (`core/ical.rs`, ~200 lines) because perfect
round-tripping of unknown properties mattered more than a crate's typed
model. No async runtime, no file-watch crate, no NLP crate.

### Targets

- Cold start to first frame: **< 50 ms** with 2,000 items.
- Release binary: **< 6 MB** stripped.
- Every keypress redraws in under one frame. No spinners anywhere.

---

## 13. Testing

- `core` gets unit tests for everything, with injected `now`. The parser gets
  a table of (input, now) → expected `Draft`, with lots of cases including
  DST changes, month ends and leap days.
- iCalendar round-trip fixtures (real exports from Google, Apple, Thunderbird
  and Nextcloud, anonymised): parse → write → parse must be identical,
  unknown properties included.
- The CLI is tested end to end against a temp data dir (`--data-dir`,
  `--config-dir`, `--state-dir` flags, as in ttyp).
- TUI: snapshot tests of each view rendered to a ratatui `TestBackend` at a
  few sizes, plus headless tmux runs for manual checks (private socket:
  `tmux -L ctui …`).
- `cargo clippy -- -D warnings` and `cargo fmt` stay clean.

---

## 14. Milestones

Each one ends with something usable.

| | Milestone | Done when |
|---|---|---|
| **M0** | Skeleton | clap with an empty TUI, config/paths, CI (test + clippy) |
| **M1** | Tasks core + CLI | model, vdir store with `.ics` round-trip, grammar for tasks, `add/ls/done/undone/rm/edit/mv/undo`, refs, `--json` |
| **M2** | Tasks TUI | Tasks view, add bar with live preview, all §7 keys for tasks, undo toasts, hints, `?`, filter (now search, §3.4), mouse |
| **M3** | Events + Agenda | events in model and grammar (ranges, all-day), Agenda view, `ct ag` |
| **M4** | Month view | grid, dots, day pane, `g`, `[` `]`, `ct cal`, responsive layouts |
| **M5** | Recurrence | `every …`, expansion, advance-on-done, this/all edits |
| **M6** | Import/export | `.ics`/dir/zip import, idempotent by UID, `--dry-run`, Tasks.json, export, the "Leaving Google" guide |
| **M7** | Customization | keymap remapping, theme loading + live-preview picker + list colours (§10.1), `:set`, view list, date/time formats |
| **M8** | Reminders | VALARM, `ct remind`, systemd timer example |
| **M9** | Polish → 1.0 | performance targets checked, README, completions, then public release (crates.io / Homebrew / AUR, as with ttyp) when the owner decides |

M1 to M3 are the core: after M3 it's already a daily-usable agenda and task
list.

---

## 15. Agents (MCP)

Added 2026-10-16 at the owner's request; all decisions below are theirs.

- `ctui mcp` serves MCP over stdio (hand-written JSON-RPC on `serde_json`, no
  new dependency). Tools: `today`, `search`, `tasks`, `month`, `get`, `lists`;
  with writing on, `add`, `edit`, `done`, `move`, `delete`, `undo`, `redo`.
  Items are addressed by id (and `occurrence` for one occurrence of a repeating
  event).
- **Agents are treated like a person:** same ops, same undo history, no
  agent labels or special toasts.
- **Structured fields, not grammar text** (owner's option B). `mcp::apply_fields`
  turns fields into a `parse::Draft` and calls `ops::add_draft` /
  `ops::edit_draft`, the same backend the grammar uses. **Sync check:**
  `apply_fields` destructures `Draft` exhaustively, so adding a grammar field
  breaks the build until MCP handles it; tests assert that fields and the
  equivalent grammar text create identical items, that the schema lists exactly
  the fields accepted, and that `ops::draft_from_item` → `edit_draft` changes
  nothing.
- **Settings:** `[ai] enabled` (the kill switch: everything refuses to run) and
  `[ai] write` (read-only when false), both default true, read on every request
  so they apply to a running server.
- **Setup:** `ctui mcp install claude` runs `claude mcp add --scope user` and
  writes the skill (`agents/SKILL.md`, compiled in) to
  `~/.claude/skills/ctui/`; `codex` and `json` print config to paste.
  `ctui mcp skill` prints the skill.
- Not done (no-bloat): notifications/subscriptions for agents that watch the
  calendar, and setting a parent (subtasks) from MCP.

---

## 16. Future plans (parked)

Parked on 2026-10-16. The owner's rule for everything here: **a companion
app is not bloat as long as the main ctui isn't harmed.** That means it lives
in its own repo (or its own crate), talks to ctui only through the vdir files,
CalDAV, the CLI's `--json` or MCP, and adds no dependency, setting or code
path to the ctui binary. If something here would need a change in ctui
itself, that change has to stand on its own merits.

1. **Verify sync, then document it (do first).** Run Radicale + vdirsyncer
   locally and check ctui's files round-trip cleanly: timezones (ctui writes
   IANA `TZID`s without VTIMEZONE blocks), list names and colours (metasync),
   repeating events with overrides, tasks with subtasks. Fix what breaks, then
   write a "Sync with your phone" guide. Maybe a `ct sync` shortcut that runs
   vdirsyncer (only if it stays a thin wrapper).
2. **Phone, the cheap way.** A CalDAV server (Radicale self-hosted, or
   Fastmail), with the phone's own Calendar and Reminders apps (iOS: a CalDAV
   account; Android: DAVx⁵ + Tasks.org). Note that iCloud can't carry tasks:
   upgraded Reminders aren't reachable over CalDAV.
3. **A ctui phone app** (Swift, like upomo) if the built-in apps aren't good
   enough. It would talk CalDAV to the same server and use the same quick-add
   grammar idea, with no backend of its own.
4. **A web app/site.** Either a landing page for ctui, or a web client over
   the same CalDAV server. Same rule: a separate project, no server-side state
   that ctui depends on.

---

## 17. Non-goals

- Two-way sync with Google, or sync code inside ctui (CalDAV sync is
  vdirsyncer's job; companion apps are §16, outside the binary).
- A background daemon or tray icon.
- Plugins or a scripting language. The CLI and `--json` are the extension
  points.
- Notes, markdown, attachments, time tracking, kanban, pomodoro, habits,
  sharing or invites (RSVP).
- An hour-grid week view (maybe later, §3).
- Nesting subtasks deeper than one level.

---

## 18. Open questions

1. ~~License~~: **MIT** (owner, 2026-10-16). The Monkeytype themes are never
   compiled in or put in the crate; they load from a themes folder at run time.
   0.1.0 went to crates.io the same day.
2. Should `ct` itself be installed as a binary name? The plan says no: Arch's
   `extra/ct` (Flatcar config transpiler) also installs `/usr/bin/ct`, so
   `ctui init` defines `ct` as an alias, and only when no other `ct` exists
   (§6.1).
