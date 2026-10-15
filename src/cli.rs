//! The command line. Every command writes to a `Write`, so the TUI's `:` line
//! runs exactly the same code.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use chrono::{Datelike, Duration, NaiveDate, Weekday};
use clap::{CommandFactory, Parser, Subcommand};
use serde::{Deserialize, Serialize};

use crate::config::{self, SETTINGS};
use crate::core::model::Kind;
use crate::core::ops::{self, Defaults, Rid};
use crate::core::parse::{self, fmt_time};
use crate::core::query::{self, Entry, Opts, Sort};
use crate::core::time::{Moment, add_months, last_of_month, week_start_of};
use crate::core::{import, remind};
use crate::session::Session;
use crate::theme::{self, Color};

#[derive(Parser, Debug)]
#[command(
    name = "ctui",
    version,
    about = "A minimal terminal calendar and task list. No arguments opens the TUI."
)]
pub struct Cli {
    /// Where config.toml and themes/ live.
    #[arg(long, global = true, value_name = "DIR")]
    pub config_dir: Option<PathBuf>,
    /// Where the lists (.ics folders) live.
    #[arg(long, global = true, value_name = "DIR")]
    pub data_dir: Option<PathBuf>,
    /// Where undo, trash and reminder state live.
    #[arg(long, global = true, value_name = "DIR")]
    pub state_dir: Option<PathBuf>,
    /// Open the TUI on this page: today, month or tasks (piped: print it).
    /// Ignored when a command is given, so `ctm add milk` just adds.
    #[arg(long, value_name = "PAGE", value_parser = config::View::parse)]
    pub view: Option<config::View>,
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Cmd {
    /// Add a task or event: ct add dentist fri 2-3pm @health
    #[command(visible_alias = "a")]
    Add {
        /// Quick-add text. Start with `task` or `event` to force one.
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        text: Vec<String>,
    },
    /// List tasks: today, week, overdue, all, done, a list, or words to match
    #[command(visible_alias = "list")]
    Ls {
        filter: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Today: overdue, today and the next days, events and tasks together
    #[command(visible_aliases = ["ag", "agenda"])]
    Today {
        days: Option<u32>,
        #[arg(long)]
        json: bool,
    },
    /// Search every task and event: any date, any list, undated and done too
    #[command(visible_alias = "find")]
    Search {
        #[arg(required = true)]
        words: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// A month at a glance: ct cal, ct cal nov, ct cal 2027-01
    Cal {
        month: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Tick off tasks (by number from the last list, or by words)
    #[command(visible_alias = "x")]
    Done {
        #[arg(required = true)]
        refs: Vec<String>,
    },
    /// Un-tick tasks
    Undone {
        #[arg(required = true)]
        refs: Vec<String>,
    },
    /// Re-type an item with the grammar, or open it in $EDITOR with no text
    #[command(visible_alias = "e")]
    Edit {
        r#ref: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        text: Vec<String>,
        /// Only this occurrence of a repeating event.
        #[arg(long)]
        this: bool,
    },
    /// Reschedule or move: ct mv 3 fri, ct mv 3 +1d, ct mv 3 @work
    Mv {
        r#ref: String,
        #[arg(required = true, allow_hyphen_values = true)]
        to: Vec<String>,
        #[arg(long)]
        this: bool,
    },
    /// Delete items (and their subtasks)
    Rm {
        #[arg(required = true)]
        refs: Vec<String>,
        #[arg(long)]
        this: bool,
    },
    /// Undo the last change (from the CLI or the TUI)
    Undo,
    /// Redo what undo undid
    Redo,
    /// Show lists, or add/rename/remove/colour one
    Lists {
        #[command(subcommand)]
        action: Option<ListsCmd>,
        #[arg(long)]
        json: bool,
    },
    /// Import .ics files, folders, Google Takeout zips or Tasks.json (one way)
    Import {
        path: PathBuf,
        /// Put everything in this list instead of one list per calendar.
        list: Option<String>,
        /// Show what would happen without changing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Write everything (or one list) as a single .ics to stdout
    Export { list: Option<String> },
    /// Show reminders due since the last run (for a timer; no daemon)
    Remind {
        /// Print only; don't send desktop notifications.
        #[arg(long)]
        no_notify: bool,
    },
    /// List themes, or switch: ct theme nord
    Theme { name: Option<String> },
    /// Show settings, or change one: ct set week_start sunday
    Set {
        key: Option<String>,
        value: Vec<String>,
    },
    /// For agents: run the MCP server (stdio), or `install claude|codex|json`, or print the `skill`
    Mcp {
        #[command(subcommand)]
        action: Option<McpCmd>,
    },
    /// Print shell completions: ct completions zsh
    Completions { shell: clap_complete::Shell },
    /// Print shortcuts (ct, ctt, ctm, cttasks) for your rc file: eval "$(ctui init zsh)"
    Init { shell: crate::shortcuts::Shell },
}

#[derive(Subcommand, Debug, Clone)]
pub enum McpCmd {
    /// Register ctui with an agent: claude (runs `claude mcp add` and adds the skill), codex, json
    Install {
        target: String,
        /// Don't write the skill file (claude only).
        #[arg(long)]
        no_skill: bool,
        /// Replace a skill file you've edited.
        #[arg(long)]
        force: bool,
    },
    /// Print the skill file that teaches an agent to use ctui well
    Skill,
}

#[derive(Subcommand, Debug, Clone)]
pub enum ListsCmd {
    /// ct lists add work "Work stuff"
    Add { id: String, name: Option<String> },
    /// Change the name shown for a list
    Rename { id: String, name: String },
    /// Remove an empty list
    Rm { id: String },
    /// #rrggbb, a palette slot number, or `theme` to clear
    Color { id: String, color: String },
}

/// A failure the exit code distinguishes: the reference matched several items.
#[derive(Debug)]
pub struct Ambiguous(pub String);

impl std::fmt::Display for Ambiguous {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Ambiguous {}

/// Output with optional colour.
pub struct Out<'w> {
    pub w: &'w mut dyn Write,
    pub color: bool,
}

impl Out<'_> {
    pub fn paint(&self, text: &str, c: Color, bold: bool) -> String {
        if !self.color {
            return text.to_string();
        }
        let code = match c {
            Color::Reset => String::new(),
            Color::Ansi(n) if n < 8 => format!("3{n}"),
            Color::Ansi(n) => format!("9{}", n - 8),
            Color::Rgb(r, g, b) => format!("38;2;{r};{g};{b}"),
        };
        let b = if bold { "1;" } else { "" };
        if code.is_empty() && !bold {
            return text.to_string();
        }
        format!("\x1b[{b}{code}m{text}\x1b[0m")
    }

    fn line(&mut self, s: &str) -> Result<()> {
        writeln!(self.w, "{s}")?;
        Ok(())
    }
}

/// Should stdout be coloured?
pub fn want_color() -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
}

// ---------- refs: "3" (from the last listing) or words ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RefEntry {
    uid: String,
    rid: Option<String>,
}

/// An occurrence as a string that round-trips (`Z<zone>|<local time>`, `D<date>`…).
pub fn moment_key(m: &Moment) -> String {
    match m {
        Moment::Date(d) => format!("D{}", d.format("%Y%m%d")),
        Moment::Floating(n) => format!("F{}", n.format("%Y%m%dT%H%M%S")),
        Moment::Zoned(z) => format!(
            "Z{}|{}",
            z.timezone().name(),
            z.naive_local().format("%Y%m%dT%H%M%S")
        ),
    }
}

pub fn moment_from_key(s: &str) -> Option<Moment> {
    // `get` rather than `split_at`: this comes from agents and state files.
    let kind = s.get(..1)?;
    let rest = &s[1..];
    match kind {
        "D" => NaiveDate::parse_from_str(rest, "%Y%m%d")
            .ok()
            .map(Moment::Date),
        "F" => chrono::NaiveDateTime::parse_from_str(rest, "%Y%m%dT%H%M%S")
            .ok()
            .map(Moment::Floating),
        "Z" => {
            let (tz, t) = rest.split_once('|')?;
            let tz: chrono_tz::Tz = tz.parse().ok()?;
            let n = chrono::NaiveDateTime::parse_from_str(t, "%Y%m%dT%H%M%S").ok()?;
            Some(Moment::Zoned(crate::core::time::resolve(tz, n)))
        }
        _ => None,
    }
}

fn save_listing(s: &Session, entries: &[(String, Rid)]) {
    let v: Vec<RefEntry> = entries
        .iter()
        .map(|(u, r)| RefEntry {
            uid: u.clone(),
            rid: r.as_ref().map(moment_key),
        })
        .collect();
    let _ = std::fs::create_dir_all(&s.paths.state_dir);
    let _ = std::fs::write(
        s.paths.state_dir.join("last-listing.json"),
        serde_json::to_string(&v).unwrap_or_default(),
    );
}

fn load_listing(s: &Session) -> Vec<RefEntry> {
    std::fs::read_to_string(s.paths.state_dir.join("last-listing.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Finds the item a ref means. `tasks_only` narrows word matches to open tasks.
fn resolve(s: &Session, r: &str, tasks_only: bool) -> Result<(String, Rid)> {
    if let Ok(n) = r.parse::<usize>() {
        let listing = load_listing(s);
        let e = listing
            .get(n.wrapping_sub(1))
            .with_context(|| format!("no item {n} in the last list (run ct ls or ct ag)"))?;
        if s.store.get(&e.uid).is_none() {
            bail!("item {n} is gone (run ct ls again)");
        }
        return Ok((e.uid.clone(), e.rid.as_deref().and_then(moment_from_key)));
    }
    let candidates: Vec<&crate::core::model::Item> = s
        .store
        .items
        .iter()
        .filter(|i| {
            if tasks_only {
                i.kind == Kind::Task
            } else {
                true
            }
        })
        .filter(|i| query::matches(r, &i.f.title, &i.list))
        .collect();
    let open: Vec<_> = candidates.iter().filter(|i| !i.f.done).copied().collect();
    let pool = if open.is_empty() { candidates } else { open };
    let exact: Vec<_> = pool
        .iter()
        .filter(|i| i.f.title.eq_ignore_ascii_case(r))
        .copied()
        .collect();
    let pick = if exact.len() == 1 { exact } else { pool };
    match pick.as_slice() {
        [one] => Ok((one.uid.clone(), None)),
        [] => bail!("nothing matches {r:?}"),
        many => {
            let names: Vec<String> = many
                .iter()
                .take(8)
                .map(|i| format!("  {} #{}", i.f.title, i.list))
                .collect();
            Err(Ambiguous(format!(
                "{r:?} matches {} items:\n{}\nuse a number from ct ls, or more words",
                many.len(),
                names.join("\n")
            ))
            .into())
        }
    }
}

// ---------- listing output ----------

fn opts(s: &Session) -> Opts {
    Opts {
        now: s.now(),
        tz: s.tz,
        hide_done_after: s.cfg.hide_done_after(),
    }
}

fn check(e: &Entry) -> &'static str {
    match (e.kind, e.done) {
        (Kind::Task, true) => "☑",
        (Kind::Task, false) => "☐",
        (Kind::Event, _) => " ",
    }
}

fn when_text(s: &Session, e: &Entry, show_day: bool) -> String {
    let fmt = s.cfg.time_format;
    let mut parts = Vec::new();
    if show_day && let Some(d) = e.due.or((e.kind == Kind::Event).then_some(e.day)) {
        parts.push(short_day(d, s.now().date_naive()));
    }
    match (e.start, e.end, e.all_day) {
        (_, _, true) => parts.push("all day".into()),
        (Some(a), Some(b), _) => parts.push(format!("{}–{}", fmt_time(a, fmt), fmt_time(b, fmt))),
        (Some(a), None, _) => parts.push(fmt_time(a, fmt)),
        (None, Some(b), _) => parts.push(format!("…{}", fmt_time(b, fmt))),
        _ => {}
    }
    parts.join(" ")
}

/// When a search result is: `tomorrow 15:00`, `Sat 17 Oct 14:00–15:00`,
/// `1 Sep 2025 all day`, `undated`. Shared with the TUI's search panel.
pub fn search_when(e: &Entry, today: NaiveDate, fmt: config::TimeFormat) -> String {
    let day = match (e.kind, e.due) {
        (Kind::Task, None) => return "undated".into(),
        (Kind::Task, Some(d)) => d,
        (Kind::Event, _) => e.day,
    };
    let mut s = short_day(day, today);
    match (e.start, e.end, e.all_day) {
        (_, _, true) => s.push_str(" all day"),
        (Some(a), Some(b), _) => s.push_str(&format!(" {}–{}", fmt_time(a, fmt), fmt_time(b, fmt))),
        (Some(a), None, _) => s.push_str(&format!(" {}", fmt_time(a, fmt))),
        _ => {}
    }
    s
}

fn cmd_search(s: &mut Session, out: &mut Out, words: &[String], json: bool) -> Result<()> {
    let o = opts(s);
    let q = words.join(" ");
    let mut hits = query::search(&s.store, &q, &o);
    let more = hits.len().saturating_sub(query::SEARCH_LIMIT);
    hits.truncate(query::SEARCH_LIMIT);
    save_listing(
        s,
        &hits
            .iter()
            .map(|h| (h.entry.uid.clone(), h.entry.rid))
            .collect::<Vec<_>>(),
    );
    if json {
        let v: Vec<serde_json::Value> = hits
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let mut v = serde_json::to_value(json_entry(i + 1, &h.entry))?;
                if h.undated() {
                    v["day"] = serde_json::Value::Null;
                }
                Ok(v)
            })
            .collect::<Result<_>>()?;
        return out.line(&serde_json::to_string_pretty(&v)?);
    }
    if hits.is_empty() {
        bail!("nothing matches {q:?}");
    }
    let today = o.today();
    let w = hits
        .iter()
        .map(|h| h.entry.title.chars().count())
        .max()
        .unwrap_or(0)
        .min(48);
    for (i, h) in hits.iter().enumerate() {
        let when = search_when(&h.entry, today, s.cfg.time_format);
        out.line(&entry_line(s, out, i + 1, &h.entry, &when, w))?;
    }
    if more > 0 {
        let msg = format!("  +{more} more · add words to narrow it");
        out.line(&out.paint(&msg, s.theme.sub, false))?;
    }
    Ok(())
}

pub fn short_day(d: NaiveDate, today: NaiveDate) -> String {
    match (d - today).num_days() {
        0 => "today".into(),
        1 => "tomorrow".into(),
        -1 => "yesterday".into(),
        n if (2..7).contains(&n) => d.format("%a").to_string(),
        _ if d.year() == today.year() => d.format("%a %-d %b").to_string(),
        _ => d.format("%-d %b %Y").to_string(),
    }
}

fn entry_line(s: &Session, out: &Out, n: usize, e: &Entry, when: &str, title_w: usize) -> String {
    let t = &s.theme;
    let num = out.paint(&format!("{n:>3}"), t.sub, false);
    let title = if let Some((d, total)) = e.progress {
        format!("{} {d}/{total}", e.title)
    } else {
        e.title.clone()
    };
    let title_c = if e.done {
        t.done
    } else if e.overdue {
        t.overdue
    } else {
        Color::Reset
    };
    let pad = title_w.saturating_sub(title.chars().count());
    let mut line = format!(
        "{num}  {} {}{}  {:<14}  {}",
        check(e),
        out.paint(&title, title_c, false),
        " ".repeat(pad),
        when,
        out.paint(&format!("#{}", e.list), s.list_color(&e.list), false)
    );
    if e.priority > 0 {
        line.push(' ');
        line.push_str(&out.paint(&"!".repeat(e.priority as usize), t.error, true));
    }
    if e.recurring {
        line.push_str(&out.paint(" ↻", t.sub, false));
    }
    line.trim_end().to_string()
}

#[derive(Serialize)]
struct JsonEntry<'a> {
    n: usize,
    uid: &'a str,
    kind: &'static str,
    title: &'a str,
    list: &'a str,
    day: String,
    start: Option<String>,
    end: Option<String>,
    all_day: bool,
    done: bool,
    overdue: bool,
    priority: u8,
    recurring: bool,
    occurrence: Option<String>,
}

fn json_entry(n: usize, e: &Entry) -> JsonEntry<'_> {
    JsonEntry {
        n,
        uid: &e.uid,
        kind: e.kind.name(),
        title: &e.title,
        list: &e.list,
        day: e.due.unwrap_or(e.day).to_string(),
        start: e.start.map(|t| t.format("%H:%M").to_string()),
        end: e.end.map(|t| t.format("%H:%M").to_string()),
        all_day: e.all_day,
        done: e.done,
        overdue: e.overdue,
        priority: e.priority,
        recurring: e.recurring,
        occurrence: e
            .rid
            .as_ref()
            .map(|m| m.instant(chrono_tz::Tz::UTC).to_rfc3339()),
    }
}

fn cmd_ls(s: &mut Session, out: &mut Out, filter: &[String], json: bool) -> Result<()> {
    let o = opts(s);
    let today = o.today();
    let words = filter.join(" ");
    let lower = words.to_lowercase();
    let list_filter = {
        let w = lower.trim().trim_start_matches(['#', '@']);
        s.store
            .lists
            .iter()
            .find(|l| l.id.to_lowercase() == w || l.name.to_lowercase() == w)
            .map(|l| l.id.clone())
    };
    let rows = query::tasks(&s.store, list_filter.as_deref(), Sort::Due, &o);
    let week_end = week_start_of(today, s.cfg.week_start) + Duration::days(6);
    let keep = |e: &Entry| -> bool {
        match lower.trim() {
            "" => !e.done && e.due.is_none_or(|d| d <= today),
            "today" => !e.done && e.due.is_some_and(|d| d <= today),
            "week" => !e.done && e.due.is_some_and(|d| d <= week_end),
            "overdue" => e.overdue,
            "all" => !e.done,
            "done" => e.done,
            _ if list_filter.is_some() => !e.done,
            q => !e.done && query::matches(q, &e.title, &e.list),
        }
    };
    let shown: Vec<&Entry> = rows
        .iter()
        .filter(|r| r.depth == 0 || lower == "all")
        .map(|r| &r.entry)
        .filter(|e| keep(e))
        .collect();
    save_listing(
        s,
        &shown
            .iter()
            .map(|e| (e.uid.clone(), None))
            .collect::<Vec<_>>(),
    );
    if json {
        let v: Vec<JsonEntry> = shown
            .iter()
            .enumerate()
            .map(|(i, e)| json_entry(i + 1, e))
            .collect();
        return out.line(&serde_json::to_string_pretty(&v)?);
    }
    if shown.is_empty() {
        return out.line(&out.paint("nothing to do", s.theme.sub, false));
    }
    let w = shown
        .iter()
        .map(|e| e.title.chars().count() + e.progress.map_or(0, |(a, b)| format!(" {a}/{b}").len()))
        .max()
        .unwrap_or(0)
        .min(48);
    for (i, e) in shown.iter().enumerate() {
        let l = entry_line(s, out, i + 1, e, &when_text(s, e, true), w);
        out.line(&l)?;
    }
    Ok(())
}

fn cmd_today(s: &mut Session, out: &mut Out, days: Option<u32>, json: bool) -> Result<()> {
    let o = opts(s);
    let sections = query::agenda(
        &s.store,
        o.today(),
        days.unwrap_or(s.cfg.days_ahead),
        &o,
        0,
        true,
    );
    let mut listing = Vec::new();
    let mut lines = Vec::new();
    let mut jsons = Vec::new();
    let w = sections
        .iter()
        .flat_map(|x| &x.entries)
        .map(|e| e.title.chars().count())
        .max()
        .unwrap_or(0)
        .min(48);
    for sec in &sections {
        lines.push(out.paint(
            &sec.title,
            if sec.day.is_none() {
                s.theme.overdue
            } else {
                s.theme.main
            },
            true,
        ));
        if sec.entries.is_empty() {
            lines.push(out.paint("       nothing", s.theme.sub, false));
        }
        for e in &sec.entries {
            listing.push((e.uid.clone(), e.rid));
            let n = listing.len();
            jsons.push(json_entry(n, e));
            let when = when_text(s, e, sec.day.is_none());
            lines.push(entry_line(s, out, n, e, &when, w));
        }
    }
    save_listing(s, &listing);
    if json {
        return out.line(&serde_json::to_string_pretty(&jsons)?);
    }
    for l in lines {
        out.line(&l)?;
    }
    Ok(())
}

/// `nov`, `2027-01`, `next`, `prev`, `+2` or nothing (this month).
fn parse_month(words: &[String], today: NaiveDate) -> Result<NaiveDate> {
    let first = today.with_day(1).expect("day 1");
    let w = words.join(" ").to_lowercase();
    let w = w.trim();
    if w.is_empty() || w == "this" {
        return Ok(first);
    }
    match w {
        "next" => return Ok(add_months(first, 1)),
        "prev" | "last" => return Ok(add_months(first, -1)),
        _ => {}
    }
    if let Some(n) = w.strip_prefix('+').and_then(|n| n.parse::<i32>().ok()) {
        return Ok(add_months(first, n));
    }
    if let Some(n) = w.strip_prefix('-').and_then(|n| n.parse::<i32>().ok()) {
        return Ok(add_months(first, -n));
    }
    if let Ok(d) = NaiveDate::parse_from_str(&format!("{w}-01"), "%Y-%m-%d") {
        return Ok(d);
    }
    let mut parts = w.split_whitespace();
    let m = parts.next().unwrap_or("");
    let names = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    if let Some(i) = names.iter().position(|n| m.len() >= 3 && m.starts_with(n)) {
        let year = match parts.next() {
            Some(y) => y.parse().context("year")?,
            None if (i as u32) + 1 < today.month() => today.year() + 1,
            None => today.year(),
        };
        return NaiveDate::from_ymd_opt(year, i as u32 + 1, 1).context("month");
    }
    bail!("not a month: {w:?} (try nov, 2027-01, next)")
}

pub fn weekday_order(start: Weekday) -> Vec<Weekday> {
    let mut d = start;
    (0..7)
        .map(|_| {
            let x = d;
            d = d.succ();
            x
        })
        .collect()
}

fn cmd_cal(s: &mut Session, out: &mut Out, words: &[String], json: bool) -> Result<()> {
    let o = opts(s);
    let today = o.today();
    let first = parse_month(words, today)?;
    let last = last_of_month(first);
    let busy = query::busy_days(&s.store, first, last + Duration::days(1), &o);
    if json {
        let mut days: Vec<(String, &Vec<String>)> =
            busy.iter().map(|(d, l)| (d.to_string(), l)).collect();
        days.sort();
        let v: Vec<serde_json::Value> = days
            .into_iter()
            .map(|(d, l)| serde_json::json!({"day": d, "count": l.len(), "lists": l}))
            .collect();
        return out.line(&serde_json::to_string_pretty(&v)?);
    }
    let t = s.theme.clone();
    let title = first.format("%B %Y").to_string();
    out.line(&out.paint(&format!("{title:^28}"), t.main, true))?;
    let heads: Vec<String> = weekday_order(s.cfg.week_start)
        .iter()
        .map(|d| format!("{:>3} ", &d.to_string()[..2]))
        .collect();
    out.line(&out.paint(&heads.concat(), t.sub, false))?;
    let start = week_start_of(first, s.cfg.week_start);
    let mut d = start;
    while d <= last {
        let mut row = String::new();
        for _ in 0..7 {
            if d.month() != first.month() {
                row.push_str("    ");
            } else {
                let num = format!("{:>3}", d.day());
                let lists = busy.get(&d);
                let num = if d == today {
                    if out.color {
                        format!("\x1b[7m{num}\x1b[0m")
                    } else {
                        format!("[{:>2}", d.day())
                    }
                } else {
                    num
                };
                let dot = match lists {
                    Some(l) => out.paint(
                        if l.len() > 3 { "•" } else { "·" },
                        s.list_color(&l[0]),
                        true,
                    ),
                    None => " ".into(),
                };
                row.push_str(&num);
                row.push_str(&if d == today && !out.color {
                    "]".to_string()
                } else {
                    dot
                });
            }
            d += Duration::days(1);
        }
        out.line(row.trim_end())?;
    }
    Ok(())
}

fn split_force(text: &[String]) -> (Option<Kind>, String) {
    match text.first().map(|s| s.as_str()) {
        Some("task") if text.len() > 1 => (Some(Kind::Task), text[1..].join(" ")),
        Some("event") if text.len() > 1 => (Some(Kind::Event), text[1..].join(" ")),
        _ => (None, text.join(" ")),
    }
}

fn notify(title: &str, body: &str) {
    #[cfg(target_os = "macos")]
    // Title and body go in as arguments, never spliced into the script.
    let _ = std::process::Command::new("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 2 of argv) with title (item 1 of argv)",
            "-e",
            "end run",
            title,
            body,
        ])
        .status();
    #[cfg(not(target_os = "macos"))]
    let _ = std::process::Command::new("notify-send")
        .args(["-a", "ctui", title, body])
        .status();
}

/// Runs one command. `interactive` is false for the TUI's `:` line (no
/// $EDITOR, no stdout-only commands).
pub fn run(s: &mut Session, cmd: Cmd, out: &mut Out, interactive: bool) -> Result<()> {
    match cmd {
        Cmd::Add { text } => {
            let (force, text) = split_force(&text);
            let (store, journal, env) = s.parts();
            let r = ops::add(store, journal, &env, &text, force, &Defaults::default())?;
            s.refresh_slots();
            out.line(&r.message)?;
        }
        Cmd::Ls { filter, json } => cmd_ls(s, out, &filter, json)?,
        Cmd::Today { days, json } => cmd_today(s, out, days, json)?,
        Cmd::Search { words, json } => cmd_search(s, out, &words, json)?,
        Cmd::Cal { month, json } => cmd_cal(s, out, &month, json)?,
        Cmd::Done { refs } => {
            for r in refs {
                let (uid, _) = resolve(s, &r, true)?;
                let (store, journal, env) = s.parts();
                out.line(&ops::set_done(store, journal, &env, &uid, true)?.message)?;
            }
        }
        Cmd::Undone { refs } => {
            for r in refs {
                let (uid, _) = resolve(s, &r, true)?;
                let (store, journal, env) = s.parts();
                out.line(&ops::set_done(store, journal, &env, &uid, false)?.message)?;
            }
        }
        Cmd::Edit { r#ref, text, this } => {
            let (uid, rid) = resolve(s, &r#ref, false)?;
            let rid = if this { rid } else { None };
            if text.is_empty() {
                if !interactive {
                    bail!("give the new text: :edit {} <text>", r#ref);
                }
                return edit_uid(s, out, &uid);
            }
            let (store, journal, env) = s.parts();
            let r = ops::edit(store, journal, &env, &uid, rid, &text.join(" "))?;
            s.refresh_slots();
            out.line(&r.message)?;
        }
        Cmd::Mv { r#ref, to, this } => {
            let (uid, rid) = resolve(s, &r#ref, false)?;
            let rid = if this { rid } else { None };
            let to = to.join(" ");
            let days = to
                .trim()
                .strip_prefix(['+', '-'])
                .and_then(|n| n.trim_end_matches('d').parse::<i64>().ok())
                .filter(|_| {
                    let t = to.trim();
                    t.ends_with('d') || t[1..].chars().all(|c| c.is_ascii_digit())
                });
            let (store, journal, env) = s.parts();
            let r = match days {
                Some(n) => ops::shift_days(
                    store,
                    journal,
                    &env,
                    &uid,
                    rid,
                    if to.trim().starts_with('-') { -n } else { n },
                )?,
                None => ops::move_to(store, journal, &env, &uid, rid, &to)?,
            };
            s.refresh_slots();
            out.line(&r.message)?;
        }
        Cmd::Rm { refs, this } => {
            let targets: Vec<(String, Rid)> = refs
                .iter()
                .map(|r| resolve(s, r, false))
                .collect::<Result<_>>()?;
            for (uid, rid) in targets {
                let rid = if this { rid } else { None };
                let (store, journal, env) = s.parts();
                out.line(&ops::remove(store, journal, &env, &uid, rid)?.message)?;
            }
        }
        Cmd::Undo => out.line(&ops::undo(&mut s.store, &mut s.journal)?.message)?,
        Cmd::Redo => out.line(&ops::redo(&mut s.store, &mut s.journal)?.message)?,
        Cmd::Lists { action, json } => cmd_lists(s, out, action, json)?,
        Cmd::Import {
            path,
            list,
            dry_run,
        } => {
            let path = config::expand_tilde(&path.to_string_lossy());
            let sources = import::read_path(&path)?;
            let into = list.map(|l| l.trim_start_matches(['#', '@']).to_string());
            let mut index = import::Index::load(&s.paths.state_dir);
            let plan = import::plan(&s.store, &index, &sources, into.as_deref())?;
            let summary = format!(
                "{} {} new, {} updated, {} unchanged, {} skipped{}",
                if dry_run {
                    "would import:"
                } else {
                    "imported:"
                },
                plan.added.len(),
                plan.updated.len(),
                plan.unchanged,
                plan.skipped.len(),
                if plan.new_lists.is_empty() {
                    String::new()
                } else {
                    format!(" · new lists: {}", plan.new_lists.join(", "))
                }
            );
            let skipped = plan.skipped.clone();
            if !dry_run {
                import::apply(
                    &mut s.store,
                    &mut s.journal,
                    &mut index,
                    plan,
                    &format!("import {}", path.display()),
                )?;
                index.save(&s.paths.state_dir)?;
                s.refresh_slots();
            }
            out.line(&summary)?;
            for k in skipped.iter().take(20) {
                out.line(&out.paint(&format!("  skipped {k}"), s.theme.sub, false))?;
            }
        }
        Cmd::Export { list } => {
            let list = list.map(|l| l.trim_start_matches(['#', '@']).to_string());
            if let Some(l) = &list
                && s.store.list(l).is_none()
            {
                bail!("no list #{l}");
            }
            write!(out.w, "{}", import::export(&s.store, list.as_deref()))?;
        }
        Cmd::Remind { no_notify } => {
            let now = s.now();
            let since = remind::last_run(&s.paths.state_dir, now);
            let alerts = remind::between(&s.store, since, now, s.tz);
            for a in &alerts {
                let when = fmt_time(a.at.time(), s.cfg.time_format);
                let body = format!(
                    "{} · #{}",
                    if a.at.date_naive() == now.date_naive() {
                        when
                    } else {
                        format!("{} {when}", a.at.format("%a %-d %b"))
                    },
                    a.list
                );
                out.line(&format!("{} · {body}", a.title))?;
                if !no_notify && interactive {
                    notify(&a.title, &body);
                }
            }
            remind::save_run(&s.paths.state_dir, now)?;
        }
        Cmd::Theme { name } => match name.as_deref() {
            None | Some("ls") | Some("list") => {
                for (n, origin) in theme::available(&s.paths.themes_dir()) {
                    let mark = if n == s.cfg.theme { "›" } else { " " };
                    let src = match origin {
                        theme::Origin::Builtin => String::new(),
                        theme::Origin::File(p) => {
                            out.paint(&format!("  {}", p.display()), s.theme.sub, false)
                        }
                    };
                    let swatch = match theme::load(&n, &s.paths.themes_dir()) {
                        Ok(t) => t
                            .lists
                            .iter()
                            .take(6)
                            .map(|c| out.paint("■", *c, false))
                            .collect::<String>(),
                        Err(_) => out.paint("(broken)", s.theme.error, false),
                    };
                    out.line(&format!("{mark} {n:<18} {swatch}{src}"))?;
                }
            }
            Some(n) => {
                let all = theme::available(&s.paths.themes_dir());
                let pick = fuzzy_pick(n, all.keys().map(String::as_str))
                    .with_context(|| format!("no theme {n:?} (ct theme lists them)"))?;
                theme::load(&pick, &s.paths.themes_dir())?;
                let path = s.paths.config_file();
                s.cfg.set(&path, "theme", &pick)?;
                if let Some(w) = s.reload_theme() {
                    out.line(&w)?;
                }
                out.line(&format!("theme · {pick}"))?;
            }
        },
        Cmd::Set { key, value } => match key {
            None => {
                let table = toml::Table::try_from(&s.cfg)?;
                for (k, about) in SETTINGS {
                    let v = k
                        .split('.')
                        .try_fold(&toml::Value::Table(table.clone()), |v, part| v.get(part))
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "(unset)".into());
                    out.line(&format!(
                        "{k:<17} {v:<24} {}",
                        out.paint(about, s.theme.sub, false)
                    ))?;
                }
            }
            Some(k) => {
                if value.is_empty() {
                    bail!("ct set {k} <value>");
                }
                let path = s.paths.config_file();
                s.cfg.set(&path, &k, &value.join(" "))?;
                if k == "theme"
                    && let Some(w) = s.reload_theme()
                {
                    out.line(&w)?;
                }
                out.line(&format!("{k} · {}", value.join(" ")))?;
            }
        },
        Cmd::Mcp { action } => {
            if !s.cfg.ai.enabled {
                bail!(
                    "agents are turned off (ai.enabled = false in config.toml; ct set ai.enabled true turns them on)"
                );
            }
            match action {
                None if interactive => {
                    let stdin = std::io::stdin();
                    crate::mcp::serve(s, stdin.lock(), out.w)?;
                }
                None => bail!("the MCP server runs as `ctui mcp`, started by your agent"),
                Some(McpCmd::Install {
                    target,
                    no_skill,
                    force,
                }) => crate::mcp::install(&target, !no_skill, force, out.w)?,
                Some(McpCmd::Skill) => write!(out.w, "{}", crate::mcp::SKILL)?,
            }
        }
        Cmd::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "ctui", out.w);
        }
        Cmd::Init { shell } => {
            write!(out.w, "{}", crate::shortcuts::init(shell))?;
        }
    }
    Ok(())
}

/// The best match for `q` among `names`: exact, then prefix, then subsequence.
pub fn fuzzy_pick<'a>(q: &str, names: impl Iterator<Item = &'a str>) -> Option<String> {
    let q = q.to_lowercase();
    let names: Vec<&str> = names.collect();
    if let Some(n) = names.iter().find(|n| n.to_lowercase() == q) {
        return Some(n.to_string());
    }
    let mut prefix: Vec<&&str> = names
        .iter()
        .filter(|n| n.to_lowercase().starts_with(&q))
        .collect();
    prefix.sort_by_key(|n| n.len());
    if let Some(n) = prefix.first() {
        return Some(n.to_string());
    }
    let mut sub: Vec<&&str> = names
        .iter()
        .filter(|n| parse::subsequence(&q, &n.to_lowercase()))
        .collect();
    sub.sort_by_key(|n| n.len());
    sub.first().map(|n| n.to_string())
}

fn cmd_lists(s: &mut Session, out: &mut Out, action: Option<ListsCmd>, json: bool) -> Result<()> {
    let strip = |x: &str| x.trim_start_matches(['#', '@']).to_string();
    let msg = match action {
        None => {
            if json {
                let v: Vec<serde_json::Value> = s
                    .store
                    .lists
                    .iter()
                    .map(|l| {
                        let n = s.store.items.iter().filter(|i| i.list == l.id).count();
                        serde_json::json!({"id": l.id, "name": l.name, "color": l.color, "items": n})
                    })
                    .collect();
                return out.line(&serde_json::to_string_pretty(&v)?);
            }
            if s.store.lists.is_empty() {
                return out.line("no lists yet: ct add creates one");
            }
            for l in &s.store.lists {
                let open = s
                    .store
                    .items
                    .iter()
                    .filter(|i| i.list == l.id && i.kind == Kind::Task && !i.f.done)
                    .count();
                let events = s
                    .store
                    .items
                    .iter()
                    .filter(|i| i.list == l.id && i.kind == Kind::Event)
                    .count();
                let dot = out.paint("●", s.list_color(&l.id), false);
                let name = if l.name != l.id {
                    format!("  ({})", l.name)
                } else {
                    String::new()
                };
                out.line(&format!(
                    "{dot} #{:<16} {open:>3} tasks  {events:>4} events{name}",
                    l.id
                ))?;
            }
            return Ok(());
        }
        Some(ListsCmd::Add { id, name }) => {
            ops::create_list(&mut s.store, &mut s.journal, &strip(&id), name.as_deref())?.message
        }
        Some(ListsCmd::Rename { id, name }) => {
            ops::rename_list(&mut s.store, &mut s.journal, &strip(&id), &name)?.message
        }
        Some(ListsCmd::Rm { id }) => {
            let id = strip(&id);
            s.store.remove_list(&id)?;
            format!("removed #{id}")
        }
        Some(ListsCmd::Color { id, color }) => {
            let id = strip(&id);
            if let Ok(slot) = color.parse::<usize>() {
                let path = s.paths.config_file();
                s.cfg.set_in_table(
                    &path,
                    "list_slots",
                    &id,
                    Some(toml::Value::Integer(slot as i64)),
                )?;
                ops::set_list_color(&mut s.store, &mut s.journal, &id, None)?;
                format!("#{id} uses palette colour {slot}")
            } else if color == "theme" || color == "none" {
                let path = s.paths.config_file();
                s.cfg.set_in_table(&path, "list_slots", &id, None)?;
                ops::set_list_color(&mut s.store, &mut s.journal, &id, None)?.message
            } else {
                let c = crate::core::store::normalize_color(&color)
                    .context("colours are #rrggbb, a palette number, or theme")?;
                ops::set_list_color(&mut s.store, &mut s.journal, &id, Some(c))?.message
            }
        }
    };
    s.refresh_slots();
    out.line(&msg)
}

/// Opens an item's .ics in $VISUAL/$EDITOR and saves it if it still parses.
pub fn edit_uid(s: &mut Session, out: &mut Out, uid: &str) -> Result<()> {
    let item = s.store.get(uid).context("gone")?.clone();
    let path = s.store.root.join(&item.list).join(&item.file);
    let before = std::fs::read_to_string(&path)?;
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".into());
    let tmp = s.paths.state_dir.join(format!("edit-{}", item.file));
    std::fs::create_dir_all(&s.paths.state_dir)?;
    std::fs::write(&tmp, &before)?;
    let mut parts = editor.split_whitespace();
    let prog = parts.next().unwrap_or("vi");
    let status = std::process::Command::new(prog)
        .args(parts)
        .arg(&tmp)
        .status()
        .with_context(|| format!("running {editor}"))?;
    let after = std::fs::read_to_string(&tmp)?;
    let _ = std::fs::remove_file(&tmp);
    if !status.success() || after == before {
        return out.line("no change");
    }
    let mut cals = crate::core::ical::parse(&after)
        .context("that isn't valid iCalendar any more; nothing saved")?;
    if cals.is_empty() {
        bail!("the file is empty; nothing saved");
    }
    let edited = crate::core::model::Item::from_calendar(cals.remove(0), &item.list, &item.file)
        .context("no task or event left in the file; nothing saved")?;
    if edited.uid != item.uid {
        bail!("the UID changed; nothing saved");
    }
    let title = edited.f.title.clone();
    s.store.commit(
        &mut s.journal,
        &format!("edit {title}"),
        vec![crate::core::store::Change::Put(edited)],
    )?;
    out.line(&format!("edited {title}"))
}
