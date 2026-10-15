# Security

Report a vulnerability privately through GitHub's "Report a vulnerability"
(Security tab), not in a public issue.

## What ctui trusts and what it doesn't

ctui reads files other people wrote: imported `.ics` files and Takeout zips,
calendars synced into the data folder by vdirsyncer or Syncthing, and
requests from AI agents over MCP. All of that is treated as untrusted.

- **No panics on bad input.** `tests/fuzz.rs` throws mangled iCalendar, quick-add
  text and repeat rules at the parser, recurrence expansion and writer; release
  builds abort on a panic, so this matters. Nesting is capped at 16 levels,
  durations at ten years, years at 1–9999, and repeat rules are checked before
  they reach the `rrule` crate (which can panic on some malformed rules).
- **No terminal escapes.** Titles and list names have control characters and
  bidirectional overrides replaced when they're read, so a calendar can't write
  escape sequences to your terminal (or your clipboard, via OSC 52).
- **No writes outside the data folder.** File names come from sanitised UIDs;
  list names are checked; the undo history refuses any path that isn't a file
  directly inside a list folder. Writes are atomic; deletes go to a trash
  folder kept for 30 days.
- **Imports read in memory** (no extraction to disk), with a 200 MB cap per zip
  entry.
- **MCP** (`ctui mcp`) can only reach the calendar: no file paths, no shell, no
  network. It speaks over stdio to the agent that started it. `[ai] write =
  false` makes it read-only and `[ai] enabled = false` turns it off entirely.
  Item text is returned as data, and the server tells agents not to follow
  instructions found in it.
- **Processes ctui starts:** `$VISUAL`/`$EDITOR` (for `E`), `notify-send` or
  `osascript` (reminders, with the text passed as arguments, never spliced
  into a script) and `claude` (only for `ctui mcp install claude`).
- **No `unsafe` code** in ctui itself. Dependencies are checked with
  `cargo audit` (clean as of 0.1.1).

ctui has no network code, accounts or telemetry.
