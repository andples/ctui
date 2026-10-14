//! Every change ctui makes, from the CLI or the TUI. Each function builds an
//! updated item and commits it through [`Store::commit`] as one undoable step,
//! returning the one-line message both front ends show.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, NaiveDate, NaiveTime};
use chrono_tz::Tz;

use super::model::{Fields, Item, Kind, List};
use super::parse::{self, Ctx, Draft, ListPick, Repeat};
use super::recur;
use super::store::{Change, Store, validate_list_id};
use super::time::Moment;
use super::undo::Journal;
use crate::config::Config;

/// The world an operation runs in.
#[derive(Clone, Copy)]
pub struct Env<'a> {
    pub now: DateTime<Tz>,
    pub tz: Tz,
    pub cfg: &'a Config,
}

impl<'a> Env<'a> {
    pub fn ctx(&self, lists: &'a [List]) -> Ctx<'a> {
        Ctx { now: self.now, date_order: self.cfg.date_order, week_start: self.cfg.week_start, lists }
    }

    pub fn today(&self) -> NaiveDate {
        self.now.date_naive()
    }
}

/// Where a new item goes when the text doesn't say: the selected day or list.
#[derive(Debug, Clone, Default)]
pub struct Defaults {
    pub date: Option<NaiveDate>,
    pub list: Option<String>,
}

/// A specific occurrence of a recurring event, for "this" edits.
pub type Rid = Option<Moment>;

pub struct Done {
    pub message: String,
    /// The item the change was about (to keep it selected).
    pub uid: Option<String>,
}

fn done(message: impl Into<String>, uid: Option<String>) -> Result<Done> {
    Ok(Done { message: message.into(), uid })
}

/// Which list a draft names, plus a change to create it if needed.
fn resolve_list(store: &Store, draft: &Draft, fallback: &str, default_list: &str) -> Result<(String, Option<Change>)> {
    let create = |id: &str| -> Result<(String, Option<Change>)> {
        validate_list_id(id)?;
        Ok((id.to_string(), Some(Change::CreateList { id: id.to_string(), name: None })))
    };
    match &draft.list {
        Some(ListPick::Existing(id)) => Ok((id.clone(), None)),
        Some(ListPick::Create(id)) => create(id),
        // Naming the default list before it exists is fine.
        Some(ListPick::Unknown(n)) if n.eq_ignore_ascii_case(default_list) => create(default_list),
        Some(ListPick::Unknown(n)) => bail!("no list #{n} (#{n}! creates it)"),
        Some(ListPick::Ambiguous(..)) => bail!("{}", draft.problem().unwrap_or_default()),
        None if store.list(fallback).is_some() => Ok((fallback.to_string(), None)),
        // The default list is created on first use, quietly.
        None => create(fallback),
    }
}

/// The kind a draft makes, given a forced kind and (for edits) the item's kind.
pub fn kind_for(draft: &Draft, force: Option<Kind>, existing: Option<Kind>) -> Kind {
    match (force, existing) {
        (Some(k), _) => k,
        // An event stays an event when edited; a task becomes one only when
        // the text says so.
        (None, Some(Kind::Event)) => Kind::Event,
        _ => draft.kind(),
    }
}

/// Fields for `kind` from a draft. `base` is the item being edited, if any.
fn fields_from(draft: &Draft, kind: Kind, base: Option<&Fields>, defaults: &Defaults, env: &Env) -> Fields {
    let tz = env.tz;
    let mut f = base.cloned().unwrap_or_default();
    f.title = draft.title.trim().to_string();
    f.priority = draft.priority.unwrap_or(0);
    f.rrule = match &draft.repeat {
        Some(Repeat::Rule(r)) => Some(r.clone()),
        Some(Repeat::Keep) => base.and_then(|b| b.rrule.clone()),
        None => None,
    };
    f.alarms = draft.remind.into_iter().collect();
    match kind {
        Kind::Task => {
            f.start = None;
            f.end = None;
            f.due = draft.date.map(|d| Moment::at(tz, d, draft.time));
        }
        Kind::Event => {
            f.due = None;
            let base_start = base.and_then(|b| b.start);
            let date = draft
                .date
                .or_else(|| base_start.map(|s| s.date(tz)))
                .or(defaults.date)
                .unwrap_or_else(|| env.today());
            let untimed = draft.time.is_none() && base_start.is_none_or(|s| s.is_date());
            if draft.allday || draft.end_date.is_some() || untimed {
                let last = draft.end_date.unwrap_or(date);
                f.start = Some(Moment::Date(date));
                f.end = Some(Moment::Date(last + Duration::days(1)));
            } else {
                let t = draft
                    .time
                    .or_else(|| base_start.and_then(|s| s.time(tz)))
                    .unwrap_or_else(|| NaiveTime::from_hms_opt(9, 0, 0).expect("valid"));
                let start = match base_start {
                    Some(s) if !s.is_date() => s.with_local(date, Some(t), tz),
                    _ => Moment::at(tz, date, Some(t)),
                };
                let old_len = base.and_then(|b| Some(b.end?.instant(tz) - b.start?.instant(tz))).filter(|l| *l > Duration::zero());
                let end_at = match (draft.end_time, draft.length) {
                    (Some(e), _) => {
                        let mut end_date = date;
                        if e <= t {
                            end_date += Duration::days(1);
                        }
                        start.with_local(end_date, Some(e), tz)
                    }
                    (None, Some(len)) => shift(start, len),
                    (None, None) => shift(start, old_len.unwrap_or(Duration::hours(1))),
                };
                f.start = Some(start);
                f.end = Some(end_at);
            }
        }
    }
    f
}

fn shift(m: Moment, d: Duration) -> Moment {
    match m {
        Moment::Date(x) => Moment::Date(x + Duration::days(d.num_days())),
        Moment::Floating(n) => Moment::Floating(n + d),
        Moment::Zoned(z) => Moment::Zoned(z + d),
    }
}

/// The default reminder for a list, if the config sets one.
fn default_alarm(env: &Env, list: &str) -> Option<Duration> {
    env.cfg.default_reminder.get(list).and_then(|s| parse::parse_length(s))
}

/// Adds an item from quick-add text.
pub fn add(store: &mut Store, journal: &mut Journal, env: &Env, text: &str, force: Option<Kind>, defaults: &Defaults) -> Result<Done> {
    let lists = store.lists.clone();
    let draft = parse::parse(text, &env.ctx(&lists));
    if draft.title.trim().is_empty() {
        bail!("nothing to add: give it a title");
    }
    let fallback = defaults.list.clone().unwrap_or_else(|| env.cfg.default_list.clone());
    let (list, create) = resolve_list(store, &draft, &fallback, &env.cfg.default_list)?;
    let kind = kind_for(&draft, force, None);
    let mut f = fields_from(&draft, kind, None, defaults, env);
    if f.alarms.is_empty() {
        let timed = f.start.is_some_and(|s| !s.is_date()) || f.due.is_some_and(|d| !d.is_date());
        if timed {
            f.alarms.extend(default_alarm(env, &list));
        }
    }
    let item = Item::new(kind, &list, f, env.now);
    let preview = draft_summary(&item, env);
    let uid = item.uid.clone();
    let mut changes: Vec<Change> = create.into_iter().collect();
    changes.push(Change::Put(item));
    store.commit(journal, &format!("add {}", draft.title), changes)?;
    done(format!("added {preview}"), Some(uid))
}

/// `task · pay rent · due Fri 17 Oct · #work`.
pub fn draft_summary(item: &Item, env: &Env) -> String {
    let mut parts = vec![format!("{} {}", item.kind.name(), item.f.title)];
    let fmt = env.cfg.time_format;
    let tz = env.tz;
    match item.kind {
        Kind::Task => {
            if let Some(d) = item.f.due {
                let mut s = format!("due {}", d.date(tz).format("%a %-d %b"));
                if let Some(t) = d.time(tz) {
                    s.push(' ');
                    s.push_str(&parse::fmt_time(t, fmt));
                }
                parts.push(s);
            }
        }
        Kind::Event => {
            if let Some(s) = item.f.start {
                let day = s.date(tz).format("%a %-d %b").to_string();
                match (s.time(tz), item.f.end.and_then(|e| e.time(tz))) {
                    (Some(a), Some(b)) => parts.push(format!("{day} {}–{}", parse::fmt_time(a, fmt), parse::fmt_time(b, fmt))),
                    (Some(a), None) => parts.push(format!("{day} {}", parse::fmt_time(a, fmt))),
                    _ => parts.push(format!("{day} all day")),
                }
            }
        }
    }
    parts.push(format!("#{}", item.list));
    if let Some(r) = &item.f.rrule {
        parts.push(recur::describe(r).unwrap_or_else(|| "repeats".into()));
    }
    parts.join(" · ")
}

fn get(store: &Store, uid: &str) -> Result<Item> {
    store.get(uid).cloned().with_context(|| format!("no item {uid}"))
}

/// The grammar text that recreates an item, for the edit bar.
pub fn to_grammar(item: &Item, rid: Rid, env: &Env) -> String {
    let tz = env.tz;
    let today = env.today();
    let fmt = env.cfg.time_format;
    let (f, start, end) = match (rid, item.kind) {
        (Some(r), Kind::Event) => {
            let occ = recur::occurrences(item, r.date(tz) - Duration::days(1), r.date(tz) + Duration::days(2), tz)
                .into_iter()
                .find(|o| o.rid == Some(r));
            match occ {
                Some(o) => (o.f.clone(), Some(o.start), o.end),
                None => (item.f.clone(), item.f.start, item.f.end),
            }
        }
        _ => (item.f.clone(), item.f.start, item.f.end),
    };
    let mut out = vec![quote_title(&f.title)];
    match item.kind {
        Kind::Task => {
            if let Some(d) = f.due {
                out.push(parse::grammar_date(d.date(tz), today));
                if let Some(t) = d.time(tz) {
                    out.push(parse::grammar_time(t, fmt));
                }
            }
        }
        Kind::Event => {
            if let Some(s) = start {
                let day = s.date(tz);
                if s.is_date() {
                    let last = end.map(|e| e.date(tz) - Duration::days(1)).filter(|l| *l > day);
                    match last {
                        Some(l) => out.push(format!("{} to {}", parse::grammar_date(day, today), parse::grammar_date(l, today))),
                        None => {
                            out.push("allday".into());
                            out.push(parse::grammar_date(day, today));
                        }
                    }
                } else {
                    out.push(parse::grammar_date(day, today));
                    let st = s.time(tz).expect("timed");
                    match end {
                        Some(e) if e.date(tz) == day && e.time(tz).is_some_and(|et| et > st) => {
                            out.push(format!("{}-{}", parse::grammar_time(st, fmt), parse::grammar_time(e.time(tz).expect("timed"), fmt)));
                        }
                        Some(e) => {
                            out.push(parse::grammar_time(st, fmt));
                            let len = e.instant(tz) - s.instant(tz);
                            if len > Duration::zero() {
                                out.push(format!("for {}", parse::fmt_length(len)));
                            }
                        }
                        None => out.push(parse::grammar_time(st, fmt)),
                    }
                }
            }
        }
    }
    out.push(format!("#{}", item.list));
    if f.priority > 0 {
        out.push("!".repeat(f.priority as usize));
    }
    if rid.is_none() {
        if let Some(r) = &item.f.rrule {
            out.push(recur::describe(r).unwrap_or_else(|| "every custom".into()));
        }
    }
    if let Some(a) = f.alarms.first() {
        out.push(format!("remind {}", parse::fmt_length(*a)));
    }
    out.join(" ")
}

/// Quotes title words the grammar would otherwise read as tokens.
fn quote_title(title: &str) -> String {
    let probe_lists: [List; 0] = [];
    let ctx = Ctx {
        now: chrono::Utc::now().with_timezone(&Tz::UTC),
        date_order: crate::config::DateOrder::Dmy,
        week_start: chrono::Weekday::Mon,
        lists: &probe_lists,
    };
    let parsed = parse::parse(title, &ctx);
    if parsed.title == title && parsed.spans.is_empty() { title.to_string() } else { format!("\"{}\"", title.replace('"', "'")) }
}

/// Re-parses text onto an item (all of it, or one occurrence with `rid`).
pub fn edit(store: &mut Store, journal: &mut Journal, env: &Env, uid: &str, rid: Rid, text: &str) -> Result<Done> {
    let lists = store.lists.clone();
    let draft = parse::parse(text, &env.ctx(&lists));
    if draft.title.trim().is_empty() {
        bail!("an item needs a title");
    }
    let mut item = get(store, uid)?;
    let (list, create) = resolve_list(store, &draft, &item.list, &env.cfg.default_list)?;
    let kind = kind_for(&draft, None, Some(item.kind));
    let defaults = Defaults::default();
    if let (Some(r), Kind::Event) = (rid, item.kind) {
        // Only this occurrence: an override at its RECURRENCE-ID.
        let mut base = item
            .overrides
            .iter()
            .find(|(x, _)| *x == r)
            .map(|(_, f)| f.clone())
            .unwrap_or_else(|| item.f.clone());
        if base.start.is_none() || item.overrides.iter().all(|(x, _)| *x != r) {
            let len = item.f.end.zip(item.f.start).map(|(e, s)| e.instant(env.tz) - s.instant(env.tz));
            base.start = Some(r);
            base.end = len.map(|l| shift(r, l));
        }
        let mut f = fields_from(&draft, Kind::Event, Some(&base), &defaults, env);
        f.rrule = None;
        item.overrides.retain(|(x, _)| *x != r);
        item.overrides.push((r, f));
        item.sync(env.now);
        let title = item.f.title.clone();
        store.commit(journal, &format!("edit {title}"), vec![Change::Put(item)])?;
        return done(format!("edited this {title} only"), Some(uid.to_string()));
    }
    if kind != item.kind {
        item = convert(item, kind);
    }
    item.f = fields_from(&draft, kind, Some(&item.f), &defaults, env);
    item.list = list;
    item.sync(env.now);
    let summary = draft_summary(&item, env);
    let mut changes: Vec<Change> = create.into_iter().collect();
    changes.extend(children_follow(store, &item));
    changes.push(Change::Put(item));
    store.commit(journal, &format!("edit {}", draft.title), changes)?;
    done(format!("edited {summary}"), Some(uid.to_string()))
}

/// Subtasks move with their parent when its list changes.
fn children_follow(store: &Store, parent: &Item) -> Vec<Change> {
    store
        .items
        .iter()
        .filter(|c| c.f.parent.as_deref() == Some(&parent.uid) && c.list != parent.list)
        .map(|c| {
            let mut c = c.clone();
            c.list = parent.list.clone();
            Change::Put(c)
        })
        .collect()
}

/// Turns a task into an event (or back), keeping everything else in the file.
fn convert(mut item: Item, kind: Kind) -> Item {
    let comp = kind.component();
    let old = item.kind.component();
    for c in item.cal.children.iter_mut().filter(|c| c.name == old) {
        c.name = comp.to_string();
        for p in ["DUE", "DTSTART", "DTEND", "DURATION", "STATUS", "COMPLETED", "PERCENT-COMPLETE"] {
            c.remove(p);
        }
    }
    item.kind = kind;
    item.f.start = None;
    item.f.end = None;
    item.f.due = None;
    item.f.done = false;
    item.f.completed = None;
    item.overrides.clear();
    item.exdates.clear();
    item
}

/// Ticks a task off (or back on). A repeating task moves to its next due.
pub fn set_done(store: &mut Store, journal: &mut Journal, env: &Env, uid: &str, on: bool) -> Result<Done> {
    let mut item = get(store, uid)?;
    if item.kind != Kind::Task {
        bail!("{} is an event; only tasks can be ticked off", item.f.title);
    }
    let title = item.f.title.clone();
    if on {
        if let (Some(rule), Some(due)) = (item.f.rrule.clone(), item.f.due) {
            let next = recur::next_due(&rule, &due, env.now, env.tz).context("the repeat has no next date")?;
            item.f.due = Some(next);
            item.sync(env.now);
            // Subtasks start over each time.
            let mut changes: Vec<Change> = store
                .items
                .iter()
                .filter(|c| c.f.parent.as_deref() == Some(uid) && c.f.done)
                .map(|c| {
                    let mut c = c.clone();
                    c.f.done = false;
                    c.f.completed = None;
                    c.sync(env.now);
                    Change::Put(c)
                })
                .collect();
            changes.push(Change::Put(item));
            store.commit(journal, &format!("done {title}"), changes)?;
            let day = next.date(env.tz).format("%a %-d %b");
            return done(format!("done · {title} (next: {day})"), Some(uid.to_string()));
        }
    }
    if item.f.done == on {
        return done(format!("{} · {title} already", if on { "done" } else { "not done" }), Some(uid.to_string()));
    }
    item.f.done = on;
    item.f.completed = None;
    item.sync(env.now);
    store.commit(journal, &format!("{} {title}", if on { "done" } else { "undone" }), vec![Change::Put(item)])?;
    done(format!("{} · {title}", if on { "done" } else { "not done" }), Some(uid.to_string()))
}

/// Deletes an item (and its subtasks), or one occurrence of a recurring event.
pub fn remove(store: &mut Store, journal: &mut Journal, env: &Env, uid: &str, rid: Rid) -> Result<Done> {
    let mut item = get(store, uid)?;
    let title = item.f.title.clone();
    if let (Some(r), true) = (rid, item.is_recurring()) {
        item.overrides.retain(|(x, _)| *x != r);
        item.exdates.push(r);
        item.sync(env.now);
        store.commit(journal, &format!("delete one {title}"), vec![Change::Put(item)])?;
        return done(format!("deleted this {title} only"), None);
    }
    let kids: Vec<String> = store.items.iter().filter(|c| c.f.parent.as_deref() == Some(uid)).map(|c| c.uid.clone()).collect();
    let mut changes = vec![Change::Remove(uid.to_string())];
    changes.extend(kids.iter().cloned().map(Change::Remove));
    store.commit(journal, &format!("delete {title}"), changes)?;
    let extra = match kids.len() {
        0 => String::new(),
        1 => " (+1 subtask)".into(),
        n => format!(" (+{n} subtasks)"),
    };
    done(format!("deleted {title}{extra}"), None)
}

/// Moves by days (the `+`/`-` keys). Undated tasks start from today.
pub fn shift_days(store: &mut Store, journal: &mut Journal, env: &Env, uid: &str, rid: Rid, days: i64) -> Result<Done> {
    let mut item = get(store, uid)?;
    let tz = env.tz;
    let title = item.f.title.clone();
    if let (Some(r), true) = (rid, item.is_recurring() && item.kind == Kind::Event) {
        let len = item.f.end.zip(item.f.start).map(|(e, s)| e.instant(tz) - s.instant(tz));
        let mut f = item.overrides.iter().find(|(x, _)| *x == r).map(|(_, f)| f.clone()).unwrap_or_else(|| {
            let mut f = item.f.clone();
            f.start = Some(r);
            f.end = len.map(|l| shift(r, l));
            f
        });
        f.rrule = None;
        f.start = f.start.map(|s| s.shift_days(days));
        f.end = f.end.map(|e| e.shift_days(days));
        item.overrides.retain(|(x, _)| *x != r);
        item.overrides.push((r, f.clone()));
        item.sync(env.now);
        store.commit(journal, &format!("move {title}"), vec![Change::Put(item)])?;
        let day = f.start.map(|s| s.date(tz).format("%a %-d %b").to_string()).unwrap_or_default();
        return done(format!("moved this {title} to {day}"), Some(uid.to_string()));
    }
    match item.kind {
        Kind::Task => {
            item.f.due = Some(match item.f.due {
                Some(d) => d.shift_days(days),
                None => Moment::Date(env.today() + Duration::days(days.max(0))),
            });
        }
        Kind::Event => {
            item.f.start = item.f.start.map(|s| s.shift_days(days));
            item.f.end = item.f.end.map(|e| e.shift_days(days));
        }
    }
    item.sync(env.now);
    let day = item.date(tz).map(|d| d.format("%a %-d %b").to_string()).unwrap_or_default();
    store.commit(journal, &format!("move {title}"), vec![Change::Put(item)])?;
    done(format!("moved {title} to {day}"), Some(uid.to_string()))
}

/// Moves to a date, a time, a list — whatever the text names (`fri`, `3pm`, `#work`).
pub fn move_to(store: &mut Store, journal: &mut Journal, env: &Env, uid: &str, rid: Rid, text: &str) -> Result<Done> {
    let lists = store.lists.clone();
    let draft = parse::parse(text, &env.ctx(&lists));
    if !draft.title.trim().is_empty() {
        bail!("move takes a date, a time or a #list, not {:?}", draft.title);
    }
    let item = get(store, uid)?;
    let tz = env.tz;
    let explicit_date = draft.date.is_some() && !draft.implied_date;
    if let (Some(r), true) = (rid, item.is_recurring() && item.kind == Kind::Event) {
        if draft.list.is_some() {
            bail!("a single occurrence can't change list; move the whole series");
        }
        let target = draft.date.filter(|_| explicit_date).unwrap_or_else(|| r.date(tz));
        let delta = (target - r.date(tz)).num_days();
        let res = shift_days(store, journal, env, uid, Some(r), delta)?;
        if let Some(t) = draft.time {
            let item = get(store, uid)?;
            let (_, f) = item.overrides.iter().find(|(x, _)| *x == r).cloned().context("override")?;
            let mut item = item;
            let mut f = f;
            let len = f.end.zip(f.start).map(|(e, s)| e.instant(tz) - s.instant(tz));
            let s = f.start.expect("start").with_local(target, Some(t), tz);
            f.start = Some(s);
            f.end = len.map(|l| shift(s, l));
            item.overrides.retain(|(x, _)| *x != r);
            item.overrides.push((r, f));
            item.sync(env.now);
            store.commit(journal, "move time", vec![Change::Put(item)])?;
        }
        return Ok(res);
    }
    let mut item = item;
    let (list, create) = match &draft.list {
        Some(_) => resolve_list(store, &draft, &item.list, &env.cfg.default_list)?,
        None => (item.list.clone(), None),
    };
    let title = item.f.title.clone();
    match item.kind {
        Kind::Task => {
            if draft.date.is_some() || draft.time.is_some() {
                let date = if explicit_date { draft.date } else { item.f.due.map(|d| d.date(tz)).or(draft.date) };
                let time = draft.time.or_else(|| item.f.due.and_then(|d| d.time(tz)));
                item.f.due = date.map(|d| Moment::at(tz, d, time));
            }
        }
        Kind::Event => {
            if let (Some(s), true) = (item.f.start, draft.date.is_some() || draft.time.is_some()) {
                let len = item.f.end.map(|e| e.instant(tz) - s.instant(tz));
                let date = if explicit_date { draft.date.expect("date") } else { s.date(tz) };
                let new_start = match (draft.time, s.is_date()) {
                    (Some(t), _) => s.with_local(date, Some(t), tz),
                    (None, true) => Moment::Date(date),
                    (None, false) => s.with_local(date, s.time(tz), tz),
                };
                let new_end = match (s.is_date(), new_start.is_date(), len) {
                    (true, false, _) => Some(shift(new_start, Duration::hours(1))),
                    (_, _, Some(l)) => Some(shift(new_start, l)),
                    _ => None,
                };
                item.f.start = Some(new_start);
                item.f.end = new_end;
            }
        }
    }
    item.list = list;
    item.sync(env.now);
    let summary = draft_summary(&item, env);
    let mut changes: Vec<Change> = create.into_iter().collect();
    changes.extend(children_follow(store, &item));
    changes.push(Change::Put(item));
    store.commit(journal, &format!("move {title}"), changes)?;
    done(format!("moved {summary}"), Some(uid.to_string()))
}

/// Makes `uid` a subtask of `parent` (or a top-level task with `None`).
pub fn set_parent(store: &mut Store, journal: &mut Journal, env: &Env, uid: &str, parent: Option<&str>) -> Result<Done> {
    let mut item = get(store, uid)?;
    if item.kind != Kind::Task {
        bail!("only tasks can be subtasks");
    }
    let title = item.f.title.clone();
    if let Some(p) = parent {
        let pi = get(store, p)?;
        if pi.kind != Kind::Task || pi.f.parent.is_some() || p == uid {
            bail!("subtasks go one level deep, under a task");
        }
        if store.items.iter().any(|c| c.f.parent.as_deref() == Some(uid)) {
            bail!("{title} has its own subtasks, so it can't be one");
        }
        item.f.parent = Some(p.to_string());
        item.list = pi.list.clone();
        item.sync(env.now);
        store.commit(journal, &format!("indent {title}"), vec![Change::Put(item)])?;
        return done(format!("{title} is now under {}", pi.f.title), Some(uid.to_string()));
    }
    if item.f.parent.is_none() {
        return done(format!("{title} is already top-level"), Some(uid.to_string()));
    }
    item.f.parent = None;
    item.sync(env.now);
    store.commit(journal, &format!("outdent {title}"), vec![Change::Put(item)])?;
    done(format!("{title} is top-level again"), Some(uid.to_string()))
}

/// Writes manual sort positions for `order` (uids in their new order).
pub fn reorder(store: &mut Store, journal: &mut Journal, env: &Env, order: &[String], moved: &str) -> Result<Done> {
    let mut changes = Vec::new();
    for (i, uid) in order.iter().enumerate() {
        let mut it = get(store, uid)?;
        let pos = Some((i as i64 + 1) * 10);
        if it.f.sort != pos {
            it.f.sort = pos;
            it.sync(env.now);
            changes.push(Change::Put(it));
        }
    }
    let title = store.get(moved).map(|i| i.f.title.clone()).unwrap_or_default();
    store.commit(journal, &format!("reorder {title}"), changes)?;
    done(format!("moved {title}"), Some(moved.to_string()))
}

pub fn create_list(store: &mut Store, journal: &mut Journal, id: &str, name: Option<&str>) -> Result<Done> {
    validate_list_id(id)?;
    if store.list(id).is_some() {
        bail!("#{id} already exists");
    }
    store.commit(journal, &format!("new list {id}"), vec![Change::CreateList { id: id.into(), name: name.map(str::to_string) }])?;
    done(format!("new list #{id}"), None)
}

pub fn rename_list(store: &mut Store, journal: &mut Journal, id: &str, name: &str) -> Result<Done> {
    if store.list(id).is_none() {
        bail!("no list #{id}");
    }
    store.commit(journal, &format!("rename {id}"), vec![Change::SetListName { id: id.into(), name: Some(name.into()) }])?;
    done(format!("#{id} is shown as {name}"), None)
}

pub fn set_list_color(store: &mut Store, journal: &mut Journal, id: &str, color: Option<String>) -> Result<Done> {
    if store.list(id).is_none() {
        bail!("no list #{id}");
    }
    let msg = match &color {
        Some(c) => format!("#{id} is {c}"),
        None => format!("#{id} uses the theme's colour"),
    };
    store.commit(journal, &format!("colour {id}"), vec![Change::SetListColor { id: id.into(), color }])?;
    done(msg, None)
}

pub fn undo(store: &mut Store, journal: &mut Journal) -> Result<Done> {
    match journal.undo(store)? {
        Some(label) => done(format!("undid {label}"), None),
        None => done("nothing to undo", None),
    }
}

pub fn redo(store: &mut Store, journal: &mut Journal) -> Result<Done> {
    match journal.redo(store)? {
        Some(label) => done(format!("redid {label}"), None),
        None => done("nothing to redo", None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::path::PathBuf;

    struct T {
        dir: PathBuf,
        store: Store,
        journal: Journal,
        cfg: Config,
    }

    fn setup(name: &str) -> T {
        let dir = std::env::temp_dir().join(format!("ctui-ops-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open(&dir.join("data"), Some(&dir.join("state"))).unwrap();
        let journal = Journal::load(&dir.join("state"));
        T { dir, store, journal, cfg: Config::default() }
    }

    impl Drop for T {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn env(cfg: &Config) -> Env<'_> {
        let tz = Tz::Europe__London;
        Env { now: tz.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap(), tz, cfg }
    }

    #[test]
    fn add_task_and_event() {
        let mut t = setup("add");
        let cfg = t.cfg.clone();
        let e = env(&cfg);
        let r = add(&mut t.store, &mut t.journal, &e, "pay rent fri !", None, &Defaults::default()).unwrap();
        assert_eq!(r.message, "added task pay rent · due Fri 16 Oct · #personal");
        let r = add(&mut t.store, &mut t.journal, &e, "dentist sat 2-3pm #health!", None, &Defaults::default()).unwrap();
        assert_eq!(r.message, "added event dentist · Sat 17 Oct 14:00–15:00 · #health");
        assert_eq!(t.store.lists.len(), 2);
        assert!(add(&mut t.store, &mut t.journal, &e, "x #nope", None, &Defaults::default()).is_err());
        let r = add(&mut t.store, &mut t.journal, &e, "meeting 3pm", Some(Kind::Event), &Defaults::default()).unwrap();
        assert_eq!(r.message, "added event meeting · Thu 15 Oct 15:00–16:00 · #personal");
    }

    #[test]
    fn edit_round_trips_through_grammar() {
        let mut t = setup("edit");
        let cfg = t.cfg.clone();
        let e = env(&cfg);
        for text in [
            "pay rent fri 15:00 #personal !! every month remind 10m",
            "trip oct 20 to oct 22 #personal",
            "birthday allday sat #personal every year",
            "standup oct 20 09:30-10:00 #personal every weekday",
            "\"fri\" night oct 21 #personal",
        ] {
            let r = add(&mut t.store, &mut t.journal, &e, text, None, &Defaults::default()).unwrap();
            let uid = r.uid.unwrap();
            let item = t.store.get(&uid).unwrap().clone();
            let g = to_grammar(&item, None, &e);
            edit(&mut t.store, &mut t.journal, &e, &uid, None, &g).unwrap();
            let after = t.store.get(&uid).unwrap();
            assert_eq!(after.f, item.f, "{text} → {g}");
        }
    }

    #[test]
    fn done_and_repeat() {
        let mut t = setup("done");
        let cfg = t.cfg.clone();
        let e = env(&cfg);
        let uid = add(&mut t.store, &mut t.journal, &e, "water plants every 3 days", None, &Defaults::default()).unwrap().uid.unwrap();
        // No date given: a plain repeat starts undated; give it one.
        move_to(&mut t.store, &mut t.journal, &e, &uid, None, "today").unwrap();
        let r = set_done(&mut t.store, &mut t.journal, &e, &uid, true).unwrap();
        assert_eq!(r.message, "done · water plants (next: Sun 18 Oct)");
        assert!(!t.store.get(&uid).unwrap().f.done);
        let uid2 = add(&mut t.store, &mut t.journal, &e, "milk", None, &Defaults::default()).unwrap().uid.unwrap();
        set_done(&mut t.store, &mut t.journal, &e, &uid2, true).unwrap();
        assert!(t.store.get(&uid2).unwrap().f.done);
        undo(&mut t.store, &mut t.journal).unwrap();
        assert!(!t.store.get(&uid2).unwrap().f.done);
    }

    #[test]
    fn shifting_and_moving() {
        let mut t = setup("move");
        let cfg = t.cfg.clone();
        let e = env(&cfg);
        let uid = add(&mut t.store, &mut t.journal, &e, "call fri 3pm", None, &Defaults::default()).unwrap().uid.unwrap();
        shift_days(&mut t.store, &mut t.journal, &e, &uid, None, 1).unwrap();
        let d = t.store.get(&uid).unwrap().f.due.unwrap();
        assert_eq!(d.date(e.tz).to_string(), "2026-10-17");
        assert_eq!(d.time(e.tz).unwrap().to_string(), "15:00:00");
        move_to(&mut t.store, &mut t.journal, &e, &uid, None, "9am").unwrap();
        let d = t.store.get(&uid).unwrap().f.due.unwrap();
        assert_eq!(d.date(e.tz).to_string(), "2026-10-17", "a time alone keeps the date");
        assert_eq!(d.time(e.tz).unwrap().to_string(), "09:00:00");
        move_to(&mut t.store, &mut t.journal, &e, &uid, None, "#work!").unwrap();
        assert_eq!(t.store.get(&uid).unwrap().list, "work");
        assert!(move_to(&mut t.store, &mut t.journal, &e, &uid, None, "lunch").is_err());
    }

    #[test]
    fn occurrence_edits() {
        let mut t = setup("occ");
        let cfg = t.cfg.clone();
        let e = env(&cfg);
        let uid = add(&mut t.store, &mut t.journal, &e, "standup oct 19 09:30-10:00 every weekday", None, &Defaults::default())
            .unwrap()
            .uid
            .unwrap();
        let item = t.store.get(&uid).unwrap().clone();
        let tz = e.tz;
        let occ = recur::occurrences(&item, NaiveDate::from_ymd_opt(2026, 10, 20).unwrap(), NaiveDate::from_ymd_opt(2026, 10, 21).unwrap(), tz);
        let rid = occ[0].rid;
        shift_days(&mut t.store, &mut t.journal, &e, &uid, rid, 1).unwrap();
        let item = t.store.get(&uid).unwrap().clone();
        assert_eq!(item.overrides.len(), 1);
        let wed = recur::occurrences(&item, NaiveDate::from_ymd_opt(2026, 10, 21).unwrap(), NaiveDate::from_ymd_opt(2026, 10, 22).unwrap(), tz);
        assert_eq!(wed.len(), 2, "the moved Tuesday and Wednesday's own");
        remove(&mut t.store, &mut t.journal, &e, &uid, rid).unwrap();
        let item = t.store.get(&uid).unwrap().clone();
        assert!(item.overrides.is_empty());
        assert_eq!(item.exdates.len(), 1);
        let tue = recur::occurrences(&item, NaiveDate::from_ymd_opt(2026, 10, 20).unwrap(), NaiveDate::from_ymd_opt(2026, 10, 22).unwrap(), tz);
        assert_eq!(tue.len(), 1);
    }

    #[test]
    fn subtasks() {
        let mut t = setup("sub");
        let cfg = t.cfg.clone();
        let e = env(&cfg);
        let p = add(&mut t.store, &mut t.journal, &e, "plan trip", None, &Defaults::default()).unwrap().uid.unwrap();
        let c = add(&mut t.store, &mut t.journal, &e, "book flights", None, &Defaults::default()).unwrap().uid.unwrap();
        set_parent(&mut t.store, &mut t.journal, &e, &c, Some(&p)).unwrap();
        assert!(set_parent(&mut t.store, &mut t.journal, &e, &p, Some(&c)).is_err());
        let r = remove(&mut t.store, &mut t.journal, &e, &p, None).unwrap();
        assert_eq!(r.message, "deleted plan trip (+1 subtask)");
        assert!(t.store.items.is_empty());
        undo(&mut t.store, &mut t.journal).unwrap();
        assert_eq!(t.store.items.len(), 2);
    }
}
