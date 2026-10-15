//! The MCP server (`ctui mcp`): agents read and change the calendar over
//! stdio, through exactly the same ops as the CLI and the TUI. Agents are
//! treated like a person at the keyboard: same undo history, no labels.
//!
//! Agents send structured fields, not grammar text. Those fields become a
//! [`Draft`], the same type the quick-add grammar produces, and go through
//! `ops::add_draft` / `ops::edit_draft`. [`apply_fields`] destructures
//! `Draft` exhaustively, so a field added to the grammar fails to compile
//! until MCP can set it too; the tests check that fields and the equivalent
//! grammar text give identical items.
//!
//! `[ai] enabled = false` turns all of this off; `[ai] write = false` leaves
//! only the read tools.

use std::io::{BufRead, Write};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{Datelike, Duration, NaiveDate, NaiveTime};
use serde_json::{Map, Value, json};

use crate::cli::{moment_from_key, moment_key};
use crate::config::Config;
use crate::core::model::{Item, Kind};
use crate::core::ops::{self, Defaults};
use crate::core::parse::{self, Draft, ListPick, Repeat};
use crate::core::query::{self, Entry, Opts, Sort};
use crate::core::recur;
use crate::core::time::Moment;
use crate::session::Session;

const PROTOCOLS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// The fields `add` and `edit` take, in schema order: (name, JSON type, description).
pub const ITEM_FIELDS: &[(&str, &str, &str)] = &[
    (
        "title",
        "string",
        "The title, used exactly as given (no date words are read from it).",
    ),
    (
        "kind",
        "string",
        "\"task\" or \"event\". Without it: an event if end, duration_minutes, all_day or end_date is set, else a task.",
    ),
    (
        "date",
        "string",
        "YYYY-MM-DD. A task's due date, an event's (first) day. null clears a task's date.",
    ),
    (
        "end_date",
        "string",
        "YYYY-MM-DD, last day of an all-day event spanning several days (inclusive).",
    ),
    (
        "start",
        "string",
        "HH:MM (24h). A task's due time, an event's start time. null makes it untimed.",
    ),
    (
        "end",
        "string",
        "HH:MM (24h). An event's end time; earlier than start means it ends the next day.",
    ),
    (
        "duration_minutes",
        "integer",
        "An event's length, instead of end.",
    ),
    ("all_day", "boolean", "An all-day event."),
    (
        "list",
        "string",
        "The list's id or name. Must exist unless create_list is true.",
    ),
    (
        "create_list",
        "boolean",
        "Create the list if it doesn't exist.",
    ),
    ("priority", "integer", "0 none, 1 low, 2 medium, 3 high."),
    (
        "repeat",
        "string",
        "How it repeats: \"day\", \"2 weeks\", \"weekday\", \"mon,wed\", \"month on 1\", \"year\", or an iCalendar RRULE like \"FREQ=WEEKLY;BYDAY=MO\". null stops it.",
    ),
    (
        "remind_minutes",
        "integer",
        "A reminder this many minutes before the start or due time. null removes it.",
    ),
];

/// Reads a JSON-RPC message per line and answers on `out` until `input` ends.
pub fn serve(s: &mut Session, input: impl BufRead, out: &mut dyn Write) -> Result<()> {
    if !s.cfg.ai.enabled {
        bail!("agents are turned off (ai.enabled = false in config.toml)");
    }
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) => {
                let replies: Vec<Value> = batch.into_iter().filter_map(|m| handle(s, m)).collect();
                (!replies.is_empty()).then_some(Value::Array(replies))
            }
            Ok(msg) => handle(s, msg),
            Err(e) => Some(error(Value::Null, -32700, &format!("parse error: {e}"))),
        };
        if let Some(r) = reply {
            writeln!(out, "{r}")?;
            out.flush()?;
        }
    }
    Ok(())
}

fn error(id: Value, code: i64, msg: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": msg}})
}

fn ok(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// Picks up config and data changed by the TUI, the CLI or other programs.
fn refresh(s: &mut Session) {
    if let Ok(cfg) = Config::load(&s.paths.config_file()) {
        s.cfg = cfg;
    }
    if s.store.changed_on_disk() {
        let _ = s.store.reload();
        s.refresh_slots();
    }
}

/// One message; `None` for notifications.
pub fn handle(s: &mut Session, msg: Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let id = id?; // notifications (initialized, cancelled…) get no answer
    refresh(s);
    if !s.cfg.ai.enabled {
        return Some(error(
            id,
            -32000,
            "agents are turned off (ai.enabled = false in config.toml)",
        ));
    }
    Some(match method {
        "initialize" => {
            let asked = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("");
            let version = PROTOCOLS
                .iter()
                .find(|p| **p == asked)
                .copied()
                .unwrap_or(PROTOCOLS[0]);
            ok(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "ctui", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": instructions(s),
                }),
            )
        }
        "ping" => ok(id, json!({})),
        "tools/list" => ok(id, json!({"tools": tools(&s.cfg)})),
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            ok(id, tool_result(call(s, &name, &args)))
        }
        _ => error(id, -32601, &format!("unknown method {method}")),
    })
}

fn tool_result(r: Result<Value>) -> Value {
    match r {
        Ok(v) => {
            let text = match v.get("message").and_then(Value::as_str) {
                Some(m) => format!(
                    "{m}\n{}",
                    serde_json::to_string_pretty(&v).unwrap_or_default()
                ),
                None => serde_json::to_string_pretty(&v).unwrap_or_default(),
            };
            json!({"content": [{"type": "text", "text": text}], "structuredContent": v, "isError": false})
        }
        Err(e) => json!({"content": [{"type": "text", "text": format!("{e:#}")}], "isError": true}),
    }
}

fn instructions(s: &Session) -> String {
    let now = s.now();
    format!(
        "ctui is the user's calendar and task list. Today is {} ({}), time zone {}. \
         Items are tasks (things to tick off, optional due date) and events (things at a time). \
         Search before adding to avoid duplicates. Use ids from tool results, never guess them. \
         Dates are YYYY-MM-DD, times HH:MM. Every write returns what was saved: check it matches what you meant. \
         Every change can be undone with the undo tool.{}",
        now.format("%Y-%m-%d"),
        now.format("%A"),
        s.tz.name(),
        if s.cfg.ai.write {
            ""
        } else {
            " Writing is turned off: you can only read."
        }
    )
}

fn schema(props: Vec<(&str, Value)>, required: &[&str]) -> Value {
    let mut p = Map::new();
    for (k, v) in props {
        p.insert(k.to_string(), v);
    }
    json!({"type": "object", "properties": p, "required": required, "additionalProperties": false})
}

fn prop(ty: &str, desc: &str) -> Value {
    json!({"type": ty, "description": desc})
}

fn item_props(with_title: bool) -> Vec<(&'static str, Value)> {
    ITEM_FIELDS
        .iter()
        .filter(|(n, ..)| with_title || *n != "title")
        .map(|(n, ty, d)| {
            let nullable = matches!(
                *n,
                "date"
                    | "start"
                    | "repeat"
                    | "remind_minutes"
                    | "end"
                    | "duration_minutes"
                    | "end_date"
            );
            let t = if nullable {
                json!([ty, "null"])
            } else {
                json!(ty)
            };
            (*n, json!({"type": t, "description": d}))
        })
        .collect()
}

fn id_props() -> Vec<(&'static str, Value)> {
    vec![
        (
            "id",
            prop("string", "The item's id, from a previous result."),
        ),
        (
            "occurrence",
            prop(
                "string",
                "For one occurrence of a repeating event: the occurrence value from a result. Leave out to change the whole series.",
            ),
        ),
    ]
}

/// The tool list; write tools only when `ai.write` is on.
pub fn tools(cfg: &Config) -> Vec<Value> {
    let read = |name: &str, desc: &str, schema: Value| json!({"name": name, "description": desc, "inputSchema": schema, "annotations": {"readOnlyHint": true}});
    let mut v = vec![
        read(
            "today",
            "Overdue tasks, then each of the next days with its events and tasks (the Today page).",
            schema(
                vec![(
                    "days",
                    prop(
                        "integer",
                        "How many days ahead (default from settings, usually 7).",
                    ),
                )],
                &[],
            ),
        ),
        read(
            "search",
            "Find tasks and events by words in the title or list, across every date and list (done items too).",
            schema(
                vec![
                    ("query", prop("string", "Words that must all appear.")),
                    ("limit", prop("integer", "Max results (default 50).")),
                ],
                &["query"],
            ),
        ),
        read(
            "tasks",
            "Tasks, with subtasks after their parent.",
            schema(
                vec![
                    ("list", prop("string", "Only this list (id or name).")),
                    (
                        "include_done",
                        prop("boolean", "Include done tasks (default false)."),
                    ),
                ],
                &[],
            ),
        ),
        read(
            "month",
            "Which days of a month have something on, and how much.",
            schema(
                vec![("month", prop("string", "YYYY-MM (default this month)."))],
                &[],
            ),
        ),
        read(
            "get",
            "Everything about one item: dates, repeat, reminders, subtasks, description.",
            schema(vec![("id", prop("string", "The item's id."))], &["id"]),
        ),
        read(
            "lists",
            "The lists, with how many open tasks and events each has.",
            schema(vec![], &[]),
        ),
    ];
    if cfg.ai.write {
        let write = |name: &str, desc: &str, schema: Value, destructive: bool| {
            json!({"name": name, "description": desc, "inputSchema": schema,
                   "annotations": {"readOnlyHint": false, "destructiveHint": destructive}})
        };
        v.push(write(
            "add",
            "Add a task or an event.",
            schema(item_props(true), &["title"]),
            false,
        ));
        let mut edit_props = id_props();
        edit_props.extend(item_props(true));
        v.push(write(
            "edit",
            "Change an item: only the fields you give change (null clears one). Moves dates, times and lists too.",
            schema(edit_props, &["id"]),
            false,
        ));
        v.push(write(
            "done",
            "Tick a task off (or back on with done=false). A repeating task moves to its next date instead.",
            schema(vec![("id", prop("string", "The task's id.")), ("done", prop("boolean", "Default true."))], &["id"]),
            false,
        ));
        let mut mv = id_props();
        mv.push((
            "days",
            prop("integer", "Days to move by, negative for earlier."),
        ));
        v.push(write(
            "move",
            "Move an item by a number of days, keeping its time.",
            schema(mv, &["id", "days"]),
            false,
        ));
        v.push(write("delete", "Delete an item and its subtasks (or one occurrence of a repeating event). undo brings it back.", schema(id_props(), &["id"]), true));
        v.push(write(
            "undo",
            "Undo the last change (anyone's: the user's or an agent's).",
            schema(vec![], &[]),
            false,
        ));
        v.push(write(
            "redo",
            "Redo what undo undid.",
            schema(vec![], &[]),
            false,
        ));
    }
    v
}

fn opts(s: &Session) -> Opts {
    Opts {
        now: s.now(),
        tz: s.tz,
        hide_done_after: s.cfg.hide_done_after(),
    }
}

fn entry_json(e: &Entry) -> Value {
    json!({
        "id": e.uid,
        "occurrence": e.rid.as_ref().map(moment_key),
        "kind": e.kind.name(),
        "title": e.title,
        "list": e.list,
        "date": e.due.unwrap_or(e.day).to_string(),
        "due": e.due.map(|d| d.to_string()),
        "start": e.start.map(|t| t.format("%H:%M").to_string()),
        "end": e.end.map(|t| t.format("%H:%M").to_string()),
        "all_day": e.all_day,
        "done": e.done,
        "overdue": e.overdue,
        "priority": e.priority,
        "repeating": e.recurring,
        "subtasks": e.progress.map(|(d, t)| json!({"done": d, "total": t})),
    })
}

fn item_json(item: &Item, s: &Session) -> Value {
    let tz = s.tz;
    let m = |x: Option<Moment>| {
        x.map(|m| match m.time(tz) {
            Some(t) => format!("{} {}", m.date(tz), t.format("%H:%M")),
            None => m.date(tz).to_string(),
        })
    };
    let master = item.master();
    let kids: Vec<&str> = s
        .store
        .items
        .iter()
        .filter(|c| c.f.parent.as_deref() == Some(&item.uid))
        .map(|c| c.uid.as_str())
        .collect();
    json!({
        "id": item.uid,
        "kind": item.kind.name(),
        "title": item.f.title,
        "list": item.list,
        "start": m(item.f.start),
        "end": m(item.f.end),
        "due": m(item.f.due),
        "all_day": item.f.all_day(),
        "done": item.f.done,
        "completed": item.f.completed.map(|c| c.to_rfc3339()),
        "priority": item.f.priority,
        "repeat": item.f.rrule.as_ref().map(|r| json!({"rule": r, "described": recur::describe(r)})),
        "remind_minutes": item.f.alarms.iter().map(|a| a.num_minutes()).collect::<Vec<_>>(),
        "parent": item.f.parent,
        "subtasks": kids,
        "description": master.text("DESCRIPTION"),
        "location": master.text("LOCATION"),
        "summary": ops::draft_summary(item, &s.env()),
    })
}

fn str_arg<'a>(args: &'a Value, k: &str) -> Option<&'a str> {
    args.get(k).and_then(Value::as_str)
}

fn need_str<'a>(args: &'a Value, k: &str) -> Result<&'a str> {
    str_arg(args, k).with_context(|| format!("{k} is required"))
}

fn rid_arg(args: &Value) -> Result<Option<Moment>> {
    match args.get("occurrence") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(k)) => moment_from_key(k)
            .map(Some)
            .context("occurrence isn't one ctui gave out"),
        Some(_) => bail!("occurrence is a string"),
    }
}

fn check_keys(args: &Value, allowed: &[&str]) -> Result<()> {
    if let Some(obj) = args.as_object() {
        for k in obj.keys() {
            if !allowed.contains(&k.as_str()) {
                bail!("unknown field {k:?} (allowed: {})", allowed.join(", "));
            }
        }
    } else if !args.is_null() {
        bail!("arguments must be an object");
    }
    Ok(())
}

fn require_write(s: &Session) -> Result<()> {
    if !s.cfg.ai.write {
        bail!("writing is turned off (ai.write = false in config.toml)");
    }
    Ok(())
}

pub fn call(s: &mut Session, name: &str, args: &Value) -> Result<Value> {
    let o = opts(s);
    match name {
        "today" => {
            check_keys(args, &["days"])?;
            let days = args
                .get("days")
                .and_then(Value::as_u64)
                .map(|d| d.clamp(1, 366) as u32)
                .unwrap_or(s.cfg.days_ahead);
            let secs = query::agenda(&s.store, o.today(), days, &o, 0, true);
            let sections: Vec<Value> = secs
                .iter()
                .map(|x| json!({"title": x.title, "date": x.day.map(|d| d.to_string()), "items": x.entries.iter().map(entry_json).collect::<Vec<_>>()}))
                .collect();
            Ok(json!({"today": o.today().to_string(), "sections": sections}))
        }
        "search" => {
            check_keys(args, &["query", "limit"])?;
            let q = need_str(args, "query")?;
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
            let hits = query::search(&s.store, q, &o);
            let total = hits.len();
            let items: Vec<Value> = hits
                .iter()
                .take(limit)
                .map(|h| entry_json(&h.entry))
                .collect();
            Ok(json!({"today": o.today().to_string(), "total": total, "items": items}))
        }
        "tasks" => {
            check_keys(args, &["list", "include_done"])?;
            let list = match str_arg(args, "list") {
                Some(l) => Some(existing_list(s, l)?),
                None => None,
            };
            let done = args
                .get("include_done")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let o2 = Opts {
                hide_done_after: if done { None } else { o.hide_done_after },
                ..o
            };
            let rows = query::tasks(&s.store, list.as_deref(), Sort::Due, &o2);
            let items: Vec<Value> = rows
                .iter()
                .filter(|r| done || !r.entry.done)
                .map(|r| {
                    let mut e = entry_json(&r.entry);
                    e["subtask"] = json!(r.depth > 0);
                    e
                })
                .collect();
            Ok(json!({"today": o.today().to_string(), "items": items}))
        }
        "month" => {
            check_keys(args, &["month"])?;
            let first = match str_arg(args, "month") {
                Some(m) => NaiveDate::parse_from_str(&format!("{m}-01"), "%Y-%m-%d")
                    .context("month is YYYY-MM")?,
                None => o.today().with_day0(0).expect("day 1"),
            };
            let next = crate::core::time::add_months(first, 1);
            let busy = query::busy_days(&s.store, first, next, &o);
            let mut days: Vec<(NaiveDate, usize)> =
                busy.iter().map(|(d, l)| (*d, l.len())).collect();
            days.sort();
            Ok(
                json!({"month": first.format("%Y-%m").to_string(), "days": days.iter().map(|(d, n)| json!({"date": d.to_string(), "count": n})).collect::<Vec<_>>()}),
            )
        }
        "get" => {
            check_keys(args, &["id"])?;
            let id = need_str(args, "id")?;
            let item = s.store.get(id).with_context(|| format!("no item {id}"))?;
            Ok(item_json(item, s))
        }
        "lists" => {
            check_keys(args, &[])?;
            let lists: Vec<Value> = s
                .store
                .lists
                .iter()
                .map(|l| {
                    let open = s.store.items.iter().filter(|i| i.list == l.id && i.kind == Kind::Task && !i.f.done).count();
                    let events = s.store.items.iter().filter(|i| i.list == l.id && i.kind == Kind::Event).count();
                    json!({"id": l.id, "name": l.name, "color": l.color, "open_tasks": open, "events": events})
                })
                .collect();
            Ok(json!({"default_list": s.cfg.default_list, "lists": lists}))
        }
        "add" => {
            require_write(s)?;
            let names: Vec<&str> = ITEM_FIELDS.iter().map(|(n, ..)| *n).collect();
            check_keys(args, &names)?;
            let lists = s.store.lists.clone();
            let (draft, force) = apply_fields(Draft::default(), args, &lists, false)?;
            let (store, journal, env) = s.parts();
            let r = ops::add_draft(store, journal, &env, &draft, force, &Defaults::default())?;
            written(s, r)
        }
        "edit" => {
            require_write(s)?;
            let mut names: Vec<&str> = ITEM_FIELDS.iter().map(|(n, ..)| *n).collect();
            names.extend(["id", "occurrence"]);
            check_keys(args, &names)?;
            let id = need_str(args, "id")?.to_string();
            let rid = rid_arg(args)?;
            let item = s
                .store
                .get(&id)
                .cloned()
                .with_context(|| format!("no item {id}"))?;
            let base = ops::draft_from_item(&item, rid, &s.env());
            let lists = s.store.lists.clone();
            let (draft, force) = apply_fields(base, args, &lists, true)?;
            let (store, journal, env) = s.parts();
            let r = ops::edit_draft(store, journal, &env, &id, rid, &draft, force)?;
            written(s, r)
        }
        "done" => {
            require_write(s)?;
            check_keys(args, &["id", "done"])?;
            let id = need_str(args, "id")?.to_string();
            let on = args.get("done").and_then(Value::as_bool).unwrap_or(true);
            let (store, journal, env) = s.parts();
            let r = ops::set_done(store, journal, &env, &id, on)?;
            written(s, r)
        }
        "move" => {
            require_write(s)?;
            check_keys(args, &["id", "occurrence", "days"])?;
            let id = need_str(args, "id")?.to_string();
            let rid = rid_arg(args)?;
            let days = args
                .get("days")
                .and_then(Value::as_i64)
                .context("days is required")?;
            let (store, journal, env) = s.parts();
            let r = ops::shift_days(store, journal, &env, &id, rid, days)?;
            written(s, r)
        }
        "delete" => {
            require_write(s)?;
            check_keys(args, &["id", "occurrence"])?;
            let id = need_str(args, "id")?.to_string();
            let rid = rid_arg(args)?;
            let (store, journal, env) = s.parts();
            let r = ops::remove(store, journal, &env, &id, rid)?;
            Ok(json!({"message": r.message}))
        }
        "undo" | "redo" => {
            require_write(s)?;
            check_keys(args, &[])?;
            let r = if name == "undo" {
                ops::undo(&mut s.store, &mut s.journal)?
            } else {
                ops::redo(&mut s.store, &mut s.journal)?
            };
            Ok(json!({"message": r.message}))
        }
        "" => bail!("no tool name"),
        other => Err(anyhow!("unknown tool {other:?}")),
    }
}

/// A write's reply: the one-line message and the item as saved.
fn written(s: &mut Session, r: ops::Done) -> Result<Value> {
    s.refresh_slots();
    let item = r
        .uid
        .as_deref()
        .and_then(|u| s.store.get(u))
        .map(|i| item_json(i, s));
    Ok(json!({"message": r.message, "item": item}))
}

fn existing_list(s: &Session, name: &str) -> Result<String> {
    match parse::pick_list(name, &s.store.lists) {
        ListPick::Existing(id) => Ok(id),
        _ => bail!(
            "no list {name:?} (lists: {})",
            s.store
                .lists
                .iter()
                .map(|l| l.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn date_arg(v: &Value, k: &str) -> Result<Option<NaiveDate>> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) => NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map(Some)
            .with_context(|| format!("{k} is YYYY-MM-DD, not {s:?}")),
        _ => bail!("{k} is a YYYY-MM-DD string"),
    }
}

fn time_arg(v: &Value, k: &str) -> Result<Option<NaiveTime>> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) => NaiveTime::parse_from_str(s, "%H:%M")
            .map(Some)
            .with_context(|| format!("{k} is HH:MM (24h), not {s:?}")),
        _ => bail!("{k} is an HH:MM string"),
    }
}

fn minutes_arg(v: &Value, k: &str) -> Result<Option<Duration>> {
    match v {
        Value::Null => Ok(None),
        Value::Number(n) => {
            let m = n
                .as_i64()
                .filter(|m| *m >= 0)
                .with_context(|| format!("{k} is a whole number of minutes"))?;
            Ok(Some(Duration::minutes(m)))
        }
        _ => bail!("{k} is a number"),
    }
}

/// `"2 weeks"`, `"mon,wed"` (read by the grammar's own repeat parser) or an RRULE.
fn repeat_arg(v: &Value, lists: &[crate::core::model::List]) -> Result<Option<Repeat>> {
    let s = match v {
        Value::Null => return Ok(None),
        Value::String(s) => s.trim(),
        _ => bail!("repeat is a string"),
    };
    if s.to_ascii_uppercase().starts_with("FREQ=") {
        let rule = s.to_ascii_uppercase();
        rule.parse::<rrule::RRule<rrule::Unvalidated>>()
            .map_err(|e| anyhow!("repeat: {e}"))?;
        return Ok(Some(Repeat::Rule(rule)));
    }
    let phrase = s.strip_prefix("every ").unwrap_or(s);
    let ctx = parse::Ctx {
        now: chrono::Utc::now().with_timezone(&chrono_tz::Tz::UTC),
        date_order: crate::config::DateOrder::Ymd,
        week_start: chrono::Weekday::Mon,
        lists,
    };
    let d = parse::parse(&format!("x every {phrase}"), &ctx);
    match (d.repeat, d.title.as_str()) {
        (Some(Repeat::Rule(r)), "x") => Ok(Some(Repeat::Rule(r))),
        _ => bail!(
            "repeat: can't read {s:?} (try \"week\", \"2 days\", \"weekday\", \"mon,fri\", \"month on 1\" or an RRULE)"
        ),
    }
}

/// Sets a draft's fields from MCP arguments. `edit` keeps what isn't given.
///
/// The destructuring below lists every `Draft` field on purpose: adding a
/// field to the grammar's `Draft` stops this compiling until MCP handles it,
/// which keeps the two ways in (grammar text and fields) in sync.
pub fn apply_fields(
    mut d: Draft,
    args: &Value,
    lists: &[crate::core::model::List],
    editing: bool,
) -> Result<(Draft, Option<Kind>)> {
    let Draft {
        title,
        date,
        end_date,
        time,
        end_time,
        length,
        allday,
        list,
        priority,
        repeat,
        remind,
        implied_date,
        spans,
    } = &mut d;
    // Grammar-only bookkeeping: fields never imply or highlight anything.
    *implied_date = false;
    spans.clear();
    let get = |k: &str| args.get(k);
    let mut force = None;
    if let Some(v) = get("title") {
        *title = v.as_str().context("title is a string")?.trim().to_string();
    }
    if let Some(v) = get("kind") {
        force = match v.as_str() {
            Some("task") => Some(Kind::Task),
            Some("event") => Some(Kind::Event),
            _ => bail!("kind is \"task\" or \"event\""),
        };
    }
    if let Some(v) = get("date") {
        *date = date_arg(v, "date")?;
    }
    if let Some(v) = get("end_date") {
        *end_date = date_arg(v, "end_date")?;
    }
    let old_len = match (*time, *end_time, *length) {
        (Some(t), Some(e), _) if e > t => Some(e - t),
        (_, _, l) => l,
    };
    if let Some(v) = get("start") {
        *time = time_arg(v, "start")?;
        if time.is_some() {
            *allday = false;
            if editing && get("end").is_none() && get("duration_minutes").is_none() {
                // Moving the start keeps the length.
                *end_time = None;
                *length = old_len;
            }
        }
    }
    if let Some(v) = get("end") {
        *end_time = time_arg(v, "end")?;
        if end_time.is_some() {
            *length = None;
        }
    }
    if let Some(v) = get("duration_minutes") {
        *length = minutes_arg(v, "duration_minutes")?;
        if length.is_some() {
            *end_time = None;
        }
    }
    if let Some(v) = get("all_day") {
        *allday = v.as_bool().context("all_day is true or false")?;
        if *allday {
            *time = None;
            *end_time = None;
            *length = None;
        }
    }
    let create = match get("create_list") {
        Some(v) => v.as_bool().context("create_list is true or false")?,
        None => false,
    };
    if let Some(v) = get("list") {
        let name = v
            .as_str()
            .context("list is a string")?
            .trim_start_matches(['#', '@']);
        *list = Some(match parse::pick_list(name, lists) {
            ListPick::Existing(id) => ListPick::Existing(id),
            _ if create => {
                crate::core::store::validate_list_id(name)?;
                ListPick::Create(name.to_string())
            }
            ListPick::Ambiguous(n, ids) => bail!("list {n:?} could be {}", ids.join(" or ")),
            _ => bail!(
                "no list {name:?} (lists: {}; set create_list to make it)",
                lists
                    .iter()
                    .map(|l| l.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        });
    }
    if let Some(v) = get("priority") {
        let p = v
            .as_u64()
            .filter(|p| *p <= 3)
            .context("priority is 0, 1, 2 or 3")? as u8;
        *priority = (p > 0).then_some(p);
    }
    if let Some(v) = get("repeat") {
        *repeat = repeat_arg(v, lists)?;
    }
    if let Some(v) = get("remind_minutes") {
        *remind = minutes_arg(v, "remind_minutes")?;
    }
    if title.is_empty() {
        bail!("title is required");
    }
    Ok((d, force))
}

/// `ctui mcp install`: where agents are told about ctui.
pub fn install(target: &str, skill: bool, out: &mut dyn Write) -> Result<()> {
    let exe = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .context("finding the ctui binary")?;
    let exe = exe.to_string_lossy().to_string();
    match target {
        "claude" => {
            let args = ["mcp", "add", "--scope", "user", "ctui", "--", &exe, "mcp"];
            match std::process::Command::new("claude").args(args).status() {
                Ok(st) if st.success() => writeln!(out, "added ctui to Claude Code (user scope)")?,
                Ok(_) => writeln!(
                    out,
                    "claude mcp add didn't succeed (already added? `claude mcp list` shows it)"
                )?,
                Err(_) => writeln!(
                    out,
                    "claude isn't on PATH. Run this where it is:\n  claude {}",
                    args.join(" ")
                )?,
            }
            if skill {
                let dir = directories::BaseDirs::new()
                    .context("no home directory")?
                    .home_dir()
                    .join(".claude/skills/ctui");
                std::fs::create_dir_all(&dir)?;
                std::fs::write(dir.join("SKILL.md"), SKILL)?;
                writeln!(
                    out,
                    "wrote the ctui skill to {}",
                    dir.join("SKILL.md").display()
                )?;
            }
        }
        "codex" => writeln!(
            out,
            "# add to ~/.codex/config.toml\n[mcp_servers.ctui]\ncommand = {exe:?}\nargs = [\"mcp\"]"
        )?,
        "json" | "cursor" | "other" => writeln!(
            out,
            "{}",
            serde_json::to_string_pretty(
                &json!({"mcpServers": {"ctui": {"command": exe, "args": ["mcp"]}}})
            )?
        )?,
        t => bail!("unknown target {t:?} (claude, codex, json)"),
    }
    Ok(())
}

/// The skill file that teaches an agent to use ctui well (`ctui mcp skill`).
pub const SKILL: &str = include_str!("../agents/SKILL.md");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Paths;
    use crate::core::model::Fields;
    use chrono::TimeZone;
    use chrono_tz::Tz;

    struct T {
        s: Session,
        dir: std::path::PathBuf,
    }

    impl Drop for T {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn setup(name: &str) -> T {
        let dir = std::env::temp_dir().join(format!("ctui-mcp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let paths = Paths {
            config_dir: dir.join("c"),
            data_dir: dir.join("d"),
            state_dir: dir.join("s"),
        };
        let mut s = Session::open(paths, Config::default()).unwrap();
        s.tz = Tz::Europe__London;
        s.fixed_now = Some(
            Tz::Europe__London
                .with_ymd_and_hms(2026, 10, 15, 12, 0, 0)
                .unwrap(),
        );
        for l in ["work", "health"] {
            ops::create_list(&mut s.store, &mut s.journal, l, None).unwrap();
        }
        T { s, dir }
    }

    fn rpc(s: &mut Session, method: &str, params: Value) -> Value {
        handle(
            s,
            json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}),
        )
        .unwrap()
    }

    fn tool(s: &mut Session, name: &str, args: Value) -> Value {
        rpc(s, "tools/call", json!({"name": name, "arguments": args}))["result"].clone()
    }

    /// The created item's fields when added through the grammar vs through MCP.
    fn fields_both_ways(name: &str, text: &str, args: Value) -> (Fields, Fields) {
        let mut a = setup(&format!("{name}-g"));
        let (store, journal, env) = a.s.parts();
        let uid = ops::add(store, journal, &env, text, None, &Defaults::default())
            .unwrap()
            .uid
            .unwrap();
        let g = a.s.store.get(&uid).unwrap().f.clone();
        let mut b = setup(&format!("{name}-m"));
        let res = tool(&mut b.s, "add", args);
        assert_eq!(res["isError"], false, "{res}");
        let id = res["structuredContent"]["item"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let m = b.s.store.get(&id).unwrap().f.clone();
        (g, m)
    }

    #[test]
    fn fields_and_grammar_make_the_same_items() {
        let cases = [
            ("buy milk", json!({"title": "buy milk"})),
            (
                "pay rent 2026-10-16 !!!",
                json!({"title": "pay rent", "date": "2026-10-16", "priority": 3}),
            ),
            (
                "call 2026-10-16 15:00 remind 10m",
                json!({"title": "call", "date": "2026-10-16", "start": "15:00", "remind_minutes": 10}),
            ),
            (
                "dentist 2026-10-17 14:00-15:30 #health",
                json!({"title": "dentist", "date": "2026-10-17", "start": "14:00", "end": "15:30", "list": "health"}),
            ),
            (
                "standup 2026-10-19 9:30 15m every weekday #work",
                json!({"title": "standup", "date": "2026-10-19", "start": "09:30", "duration_minutes": 15, "repeat": "weekday", "list": "work"}),
            ),
            (
                "trip 2026-10-20 to 2026-10-22",
                json!({"title": "trip", "date": "2026-10-20", "end_date": "2026-10-22", "all_day": true}),
            ),
            (
                "birthday allday 2026-12-03 every year",
                json!({"title": "birthday", "date": "2026-12-03", "all_day": true, "repeat": "FREQ=YEARLY"}),
            ),
            (
                "gym 2026-10-19 every mon,wed",
                json!({"title": "gym", "date": "2026-10-19", "repeat": "mon,wed"}),
            ),
        ];
        for (i, (text, args)) in cases.into_iter().enumerate() {
            let (g, m) = fields_both_ways(&format!("p{i}"), text, args.clone());
            assert_eq!(g, m, "{text} vs {args}");
        }
    }

    #[test]
    fn schema_covers_exactly_the_fields_apply_reads() {
        // Every advertised field is accepted, and an unknown one is refused.
        let mut t = setup("schema");
        let samples = json!({"title": "x", "kind": "event", "date": "2026-10-20", "end_date": "2026-10-20", "start": "09:00",
            "end": "10:00", "duration_minutes": 30, "all_day": false, "list": "work", "create_list": false, "priority": 1,
            "repeat": "week", "remind_minutes": 5});
        for (name, ..) in ITEM_FIELDS {
            assert!(samples.get(name).is_some(), "sample missing for {name}");
        }
        assert_eq!(samples.as_object().unwrap().len(), ITEM_FIELDS.len());
        let r = tool(&mut t.s, "add", samples);
        assert_eq!(r["isError"], false, "{r}");
        let bad = tool(&mut t.s, "add", json!({"title": "x", "when": "fri"}));
        assert_eq!(bad["isError"], true);
        let listed = tools(&t.s.cfg);
        let add = listed.iter().find(|x| x["name"] == "add").unwrap();
        let props: Vec<&String> = add["inputSchema"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect();
        assert_eq!(props.len(), ITEM_FIELDS.len());
    }

    #[test]
    fn draft_from_item_round_trips() {
        let mut t = setup("round");
        for args in [
            json!({"title": "the 3pm problem fri", "date": "2026-10-16"}),
            json!({"title": "late meeting", "date": "2026-10-16", "start": "23:00", "end": "01:00"}),
            json!({"title": "trip", "date": "2026-10-20", "end_date": "2026-10-23", "all_day": true, "list": "work"}),
            json!({"title": "standup", "date": "2026-10-19", "start": "09:30", "end": "09:45", "repeat": "weekday", "priority": 2}),
        ] {
            let r = tool(&mut t.s, "add", args.clone());
            let id = r["structuredContent"]["item"]["id"]
                .as_str()
                .unwrap()
                .to_string();
            let before = t.s.store.get(&id).unwrap().f.clone();
            let e = tool(&mut t.s, "edit", json!({"id": id}));
            assert_eq!(e["isError"], false, "{e}");
            assert_eq!(t.s.store.get(&id).unwrap().f, before, "{args}");
        }
    }

    #[test]
    fn titles_are_taken_literally() {
        let mut t = setup("literal");
        let r = tool(
            &mut t.s,
            "add",
            json!({"title": "meeting about the 3pm problem #work tomorrow !"}),
        );
        let item = &r["structuredContent"]["item"];
        assert_eq!(
            item["title"],
            "meeting about the 3pm problem #work tomorrow !"
        );
        assert_eq!(item["kind"], "task");
        assert_eq!(item["due"], Value::Null);
    }

    #[test]
    fn edit_moves_and_clears() {
        let mut t = setup("edit");
        let r = tool(
            &mut t.s,
            "add",
            json!({"title": "dentist", "date": "2026-10-17", "start": "14:00", "end": "15:00"}),
        );
        let id = r["structuredContent"]["item"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let e = tool(&mut t.s, "edit", json!({"id": id, "start": "16:00"}));
        assert_eq!(
            e["structuredContent"]["item"]["end"], "2026-10-17 17:00",
            "moving the start keeps the length"
        );
        let e = tool(&mut t.s, "edit", json!({"id": id, "list": "nope"}));
        assert_eq!(e["isError"], true);
        let e = tool(
            &mut t.s,
            "edit",
            json!({"id": id, "list": "gym", "create_list": true}),
        );
        assert_eq!(e["structuredContent"]["item"]["list"], "gym");
        let task = tool(
            &mut t.s,
            "add",
            json!({"title": "rent", "date": "2026-10-20", "repeat": "month"}),
        );
        let tid = task["structuredContent"]["item"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let e = tool(
            &mut t.s,
            "edit",
            json!({"id": tid, "date": null, "repeat": null}),
        );
        assert_eq!(e["structuredContent"]["item"]["due"], Value::Null);
        assert_eq!(e["structuredContent"]["item"]["repeat"], Value::Null);
        let u = tool(&mut t.s, "undo", json!({}));
        assert!(
            u["structuredContent"]["message"]
                .as_str()
                .unwrap()
                .starts_with("undid")
        );
        assert_eq!(
            t.s.store.get(&tid).unwrap().f.rrule.as_deref(),
            Some("FREQ=MONTHLY")
        );
    }

    #[test]
    fn protocol_and_switches() {
        let mut t = setup("proto");
        let init = rpc(
            &mut t.s,
            "initialize",
            json!({"protocolVersion": "2025-03-26"}),
        );
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert!(
            init["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains("2026-10-15")
        );
        assert!(
            handle(
                &mut t.s,
                json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
            )
            .is_none()
        );
        assert_eq!(rpc(&mut t.s, "nope", json!({}))["error"]["code"], -32601);
        let n = |s: &mut Session| {
            rpc(s, "tools/list", json!({}))["result"]["tools"]
                .as_array()
                .unwrap()
                .len()
        };
        let all = n(&mut t.s);
        let path = t.s.paths.config_file();
        t.s.cfg.set(&path, "ai.write", "false").unwrap();
        assert_eq!(n(&mut t.s), 6, "read tools only");
        assert!(all > 6);
        let w = tool(&mut t.s, "add", json!({"title": "x"}));
        assert_eq!(w["isError"], true);
        t.s.cfg.set(&path, "ai.enabled", "false").unwrap();
        assert_eq!(
            rpc(&mut t.s, "tools/list", json!({}))["error"]["code"],
            -32000
        );
        let mut out = Vec::new();
        assert!(serve(&mut t.s, std::io::Cursor::new(""), &mut out).is_err());
    }

    #[test]
    fn reads_and_occurrences() {
        let mut t = setup("read");
        tool(
            &mut t.s,
            "add",
            json!({"title": "standup", "date": "2026-10-15", "start": "09:30", "duration_minutes": 15, "repeat": "weekday"}),
        );
        tool(&mut t.s, "add", json!({"title": "milk"}));
        let today = tool(&mut t.s, "today", json!({"days": 3}));
        let sec = &today["structuredContent"]["sections"];
        assert_eq!(sec[0]["title"], "Today · Thu 15");
        let occ = sec[1]["items"][0]["occurrence"]
            .as_str()
            .unwrap()
            .to_string();
        let id = sec[1]["items"][0]["id"].as_str().unwrap().to_string();
        let e = tool(
            &mut t.s,
            "edit",
            json!({"id": id, "occurrence": occ, "start": "11:00"}),
        );
        assert!(
            e["structuredContent"]["message"]
                .as_str()
                .unwrap()
                .contains("this standup only"),
            "{e}"
        );
        let found = tool(&mut t.s, "search", json!({"query": "milk"}));
        assert_eq!(found["structuredContent"]["total"], 1);
        assert_eq!(
            tool(&mut t.s, "lists", json!({}))["structuredContent"]["lists"]
                .as_array()
                .unwrap()
                .len(),
            3,
            "work, health and the default list"
        );
        assert!(
            tool(&mut t.s, "month", json!({"month": "2026-10"}))["structuredContent"]["days"]
                .as_array()
                .unwrap()
                .len()
                > 10
        );
    }
}
