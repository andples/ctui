# ctui

A minimal terminal calendar and task list. Run `ctui` for the TUI; give it
arguments and it's a plain command-line tool that does the same things.
Your data stays on your machine as standard iCalendar files: no accounts,
no server, no Google.

```sh
alias ct=ctui

ct add pay rent fri !               # a task, due Friday, priority !
ct add dentist sat 2-3pm @health!   # an event (a time range makes it one), in a new list
ct add "standup 9:30 15m every weekday @work"
ct ls                               # open tasks due by today, and undated ones
ct done rent                        # by words, or by number from the last list
ct ag                               # agenda: the next 7 days
ct cal                              # this month at a glance
ct undo                             # every change can be undone
ct                                  # the TUI
```

## Build

```sh
cargo build --release               # target/release/ctui
cargo install --path .              # into ~/.cargo/bin
```

`ctui` is the binary. Use `alias ct=ctui` rather than installing a `ct`
binary: Arch's `ct` package (the Flatcar config transpiler) uses that name.

## The quick-add grammar

One grammar everywhere: `ct add`, the TUI's add bar (`a`), edit (`e`), move
(`m`) and go-to-date (`g`). Words that aren't tokens make up the title, in
any order. In the TUI, recognised tokens are coloured as you type and the
line underneath says what will be created, so you never have to guess.

| You type | You get |
|---|---|
| `buy milk` | a task with no date |
| `pay rent fri !` | due Friday, priority `!` (up to `!!!`) |
| `call mum tom 6pm` | due tomorrow at 18:00 |
| `dentist sat 2-3pm #health` | an **event**: a time range makes one |
| `standup 9:30 15m every weekday` | an event with a length, repeating |
| `trip oct 20-22` / `trip oct 30 to nov 2` | an all-day event over several days |
| `birthday allday dec 3 every year` | a yearly all-day event |
| `gym every mon,wed 7am` | repeats on those days, starting at the next one |
| `rent every month on 1` | monthly on the 1st |
| `report next fri remind 1h` | with a reminder an hour before |
| `"fri" night` | quotes keep words as the title |

- **Dates:** `today` `tod` `tomorrow` `tom` `mon`…`sun` `next fri` `oct 20`
  `20 oct` `20/10` (order from `date_order`) `2026-10-20` `+3d` `+2w` `+1m`
  `in 3 days` `next week` `eom` `eow`. Dates resolve forward: `fri` on a
  Friday is today, `mar 3` in October is next March.
- **Times:** `3pm` `3:30pm` `15:00` `9.30` `noon`. A time alone means today.
  `on`/`at`/`by`/`due` in front are fine (`at 3pm`, `due fri`).
- **Ranges and lengths:** `3-4pm` `14:00-15:30` `3pm - 4pm` `3pm 1h` `for 45m`.
  A length only counts next to a time, so `read 30m` stays a task title.
- **Task or event?** It's an event if it has a range, a length, a span or
  `allday`; otherwise a task. `ct add event …` / `ct add task …` (TUI: `A`)
  force one.
- **Lists:** `#work` or `@work` (shells treat `#` at the start of a word as a
  comment, so `@` is easier unquoted). Fuzzy: `#wo` is enough. `#gym!`
  creates a list. Without one, items go to `default_list` (`personal`).
- **Repeats:** `every day` `every 2 weeks` `every other week` `every weekday`
  `every mon,wed` `every tue and thu` `every month` `every month on 15`
  `every year`. Imported rules the grammar can't type (BYSETPOS, EXDATE…)
  display correctly and are kept; editing shows them as `every custom`.

## The TUI

Three views: **Agenda** (tasks and events together, by day), **Month** and
**Tasks** (a checklist per list). The bottom line always shows the keys that
do something right now; `?` shows them all, plus the grammar.

| Key | |
|---|---|
| `j` `k` / arrows | move (in Month: `h j k l` move the day, `J` `K` pick in the day's list) |
| `1` `2` `3` / `tab` | Agenda / Month / Tasks |
| `a` / `A` | add / add an event (prefilled with the selected day or list: just type the title) |
| `x` / `space` | tick off (`space` on a parent opens its subtasks) |
| `e` / `enter` | edit as text (in Month, `enter` opens the day in the Agenda) |
| `E` | edit the raw `.ics` in `$EDITOR` |
| `d` | delete. No confirmation: `u` brings it back |
| `u` / `ctrl+r` | undo / redo (shared with `ct undo` in a shell) |
| `m` | move to a date, a time or a list: `fri`, `3pm`, `#work` |
| `+` `-` | one day later / earlier |
| `>` `<` | make a subtask of the task above / top-level again (Tasks view) |
| `J` `K` | reorder (Tasks view, switches to manual order) |
| `H` `L` / `←` `→` | previous / next list (Tasks view) |
| `s` | sort: due / priority / manual (Tasks view) |
| `t` `[` `]` `g` | today, previous / next week or month, go to a date |
| `T` | show undated tasks in the Agenda |
| `/` | filter as you type; `esc` clears |
| `c` | colour themes, previewed live as you move |
| `:` | the command line: any `ct` command (`:import ~/takeout.zip`, `:theme nord`, `:set week_start sunday`, `:ls all`) |
| `?` / `q` | help / quit |

A repeating event asks once whether a change is for **this** one or **all**
of them. The mouse works too (click to select, click a checkbox to tick, click
a day or a tab, scroll); turn it off with `mouse = false`. Changes made by
other programs (Syncthing, vdirsyncer, an editor) show up within two seconds.

## Your data

Plain iCalendar in a vdir: one folder per list, one `.ics` file per item.

```
~/.local/share/ctui/
  personal/
    displayname        the list's name as shown (optional)
    color              its colour, #rrggbb (optional)
    3f9c…@ctui.ics     one task (VTODO) or event (VEVENT)
```

- Anything ctui doesn't understand (attendees, descriptions, other apps'
  `X-` properties) is written back untouched when you edit an item.
- Writes are atomic. Deleted files are kept in `~/.local/state/ctui/trash`
  for 30 days.
- It's the layout `vdirsyncer` uses, so pointing `data_dir` at a vdirsyncer
  storage syncs with any CalDAV server (Radicale, Nextcloud, Fastmail,
  iCloud) without any sync code in ctui. Syncthing or git work as well.

## Leaving Google Calendar

1. Export: Google Calendar → Settings → Import & export → **Export**, or
   [Google Takeout](https://takeout.google.com) with *Calendar* (and *Tasks*,
   for Google Tasks) selected.
2. Import (one way into ctui):

   ```sh
   ct import ~/Downloads/takeout-2026….zip --dry-run   # what would happen
   ct import ~/Downloads/takeout-2026….zip
   ```

   Each calendar becomes a list, named after it (and coloured, when the
   export says how). Takeout's
   `Tasks.json` comes along too. `ct import file.ics work` puts everything
   in one list.
3. Re-importing a newer export is safe: items are matched by their UID,
   items you haven't touched here are updated, ones you've edited or deleted
   in ctui are left alone (and listed), and nothing is duplicated.
4. When you're happy, delete the calendar in Google if you want to.

`ct export [list] > all.ics` writes one standard `.ics` back out, so leaving
ctui is just as easy.

## Reminders (no daemon)

Add `remind 10m` to an item (or set `default_reminder = { work = "10m" }`).
`ct remind` prints and sends a desktop notification (`notify-send`, or
`osascript` on macOS) for every reminder due since it last ran. Run it from
a timer; without one there are no reminders and nothing in the background.

systemd (user):

```ini
# ~/.config/systemd/user/ctui-remind.service
[Service]
Type=oneshot
ExecStart=%h/.cargo/bin/ctui remind

# ~/.config/systemd/user/ctui-remind.timer
[Timer]
OnCalendar=minutely
[Install]
WantedBy=timers.target
```

```sh
systemctl --user enable --now ctui-remind.timer
```

cron: `* * * * * ~/.cargo/bin/ctui remind`

## Settings

`~/.config/ctui/config.toml`. Every key is optional. `ct set` (or `:set` in
the TUI) lists them and changes one; it's saved immediately.

```toml
theme        = "default"      # ct theme lists them
default_view = "agenda"       # agenda | month | tasks
views        = ["agenda", "month", "tasks"]
week_start   = "monday"
date_order   = "dmy"          # dmy | mdy | ymd, for typed dates like 20/10
time_format  = "24h"          # 24h | 12h
agenda_days  = 7
hide_done_after = "1d"        # 0 | 12h | 1d | 7d | never
overdue_collapse = 5          # overdue tasks shown before "+N more"
default_list = "personal"
mouse        = true
data_dir     = "~/Sync/calendar"   # e.g. a Syncthing or vdirsyncer folder
list_colors  = "file"         # file: a list's color file wins; theme: always the theme
default_reminder = { work = "10m" }

[keys]                        # action = keys; replaces that action's defaults
done = ["x", "space"]
quit = ["q", "ctrl+q"]
```

Action names are listed in the help screen (`?`), next to each key.

## Themes

39 themes ship in `themes/`: `default` (your terminal's own colours, so it
matches any terminal theme), `light`, 12 well-known palettes (Catppuccin,
Dracula, Everforest, gruvbox, Kanagawa, Nord, One Dark, Rosé Pine,
Solarized, Tokyo Night) and 25 from Monkeytype. A theme is five colours and
a palette for lists:

```toml
name = "nord"

[colors]
bg    = "#2e3440"
fg    = "#d8dee9"
sub   = "#616e88"   # times, hints, done items
main  = "#88c0d0"   # selection, today, headings
error = "#bf616a"   # overdue
lists = ["#88c0d0", "#a3be8c", "#d08770", "#b48ead", "#ebcb8b", "#81a1c1"]
```

Drop your own into `~/.config/ctui/themes/`. Each list keeps its palette
slot from the first time ctui sees it, so colours don't shuffle; pin one with
`ct lists color work 3`, or give it a fixed colour with
`ct lists color work #a3be8c`.

The Monkeytype themes are GPL-3.0, so they're loaded from a themes folder at
run time rather than compiled in: packages install them to
`share/ctui/themes/`, and a build from this repo finds `themes/` next to it.
`cargo build --features monkeytype-themes` compiles them in.

## Scripting

Every read command takes `--json` (`ct ls --json`, `ct ag --json`,
`ct cal --json`, `ct lists --json`). Exit codes: 0 ok, 1 error or no match,
2 a reference matched several items. Output is plain when piped and respects
`NO_COLOR`. `ctui` with no arguments and no terminal prints the agenda.
`ct completions bash|zsh|fish` prints shell completions (for the alias in
bash: `complete -o default -F _ctui ct`).

## Credits

25 of the themes in `themes/` (`serika-dark`, `8008`, `iceberg-dark`, …) are
adapted from [Monkeytype](https://github.com/monkeytypegame/monkeytype)'s themes
by the Monkeytype contributors, licensed under
[GPL-3.0](https://github.com/monkeytypegame/monkeytype/blob/master/LICENSE); those
files keep that license. [`themes/MONKEYTYPE.md`](themes/MONKEYTYPE.md) lists them
and explains how the colours were mapped. Other palette credits are in
[`themes/CREDITS.md`](themes/CREDITS.md).

See [DESIGN.md](DESIGN.md) for the plan and the reasons behind it.
