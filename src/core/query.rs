//! Read-only views of the store: the agenda, the task list, month dots.

use std::collections::HashMap;

use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc};
use chrono_tz::Tz;

use super::model::{Item, Kind};
use super::recur;
use super::store::Store;
use super::time::Moment;

/// One line in the agenda: an event occurrence or a task.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub uid: String,
    /// The occurrence of a recurring event.
    pub rid: Option<Moment>,
    pub kind: Kind,
    pub title: String,
    pub list: String,
    pub day: NaiveDate,
    pub start: Option<NaiveTime>,
    pub end: Option<NaiveTime>,
    /// The event runs on from an earlier day (`… continues`).
    pub continued: bool,
    pub all_day: bool,
    pub done: bool,
    pub overdue: bool,
    pub priority: u8,
    pub recurring: bool,
    /// Subtasks done / total, for a parent task.
    pub progress: Option<(usize, usize)>,
    /// Due date shown in the Overdue section.
    pub due: Option<NaiveDate>,
    /// Happening right now (an event in progress, a task due this minute).
    pub now: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub title: String,
    pub day: Option<NaiveDate>,
    pub entries: Vec<Entry>,
    /// Entries left out (overdue past the collapse limit).
    pub hidden: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct Opts {
    pub now: DateTime<Tz>,
    pub tz: Tz,
    /// Done tasks stay visible this long after completion (`None` = always).
    pub hide_done_after: Option<Duration>,
}

impl Opts {
    pub fn today(&self) -> NaiveDate {
        self.now.date_naive()
    }

    /// A done task stays visible for the rest of the day it was ticked, and
    /// for `hide_done_after` after that.
    pub fn done_visible(&self, item: &Item) -> bool {
        if !item.f.done {
            return true;
        }
        let Some(at) = item.f.completed else { return false };
        let local = at.with_timezone(&self.tz);
        if local.date_naive() == self.today() {
            return true;
        }
        match self.hide_done_after {
            None => true,
            Some(d) => self.now.with_timezone(&Utc) - at < d,
        }
    }
}

fn progress(store: &Store, uid: &str) -> Option<(usize, usize)> {
    let kids: Vec<&Item> = store.items.iter().filter(|c| c.f.parent.as_deref() == Some(uid)).collect();
    (!kids.is_empty()).then(|| (kids.iter().filter(|c| c.f.done).count(), kids.len()))
}

fn task_entry(store: &Store, item: &Item, day: NaiveDate, o: &Opts) -> Entry {
    let tz = o.tz;
    let due = item.f.due;
    let overdue = !item.f.done
        && due.is_some_and(|d| match d.time(tz) {
            Some(_) => d.instant(tz) < o.now,
            None => d.date(tz) < o.today(),
        });
    let now = !item.f.done && due.is_some_and(|d| d.time(tz).is_some() && (d.instant(tz) - o.now).num_minutes().abs() < 15);
    Entry {
        uid: item.uid.clone(),
        rid: None,
        kind: Kind::Task,
        title: item.f.title.clone(),
        list: item.list.clone(),
        day,
        start: due.and_then(|d| d.time(tz)),
        end: None,
        continued: false,
        all_day: false,
        done: item.f.done,
        overdue,
        priority: item.f.priority,
        recurring: item.is_recurring(),
        progress: progress(store, &item.uid),
        due: due.map(|d| d.date(tz)),
        now,
    }
}

/// Event and task entries on one day (no overdue section).
pub fn day(store: &Store, d: NaiveDate, o: &Opts) -> Vec<Entry> {
    let tz = o.tz;
    let mut out = Vec::new();
    for item in &store.items {
        match item.kind {
            Kind::Event => {
                for occ in recur::occurrences(item, d, d + Duration::days(1), tz) {
                    let s_day = occ.start.date(tz);
                    let all_day = occ.start.is_date();
                    let end_local = occ.end.map(|e| e.local(tz));
                    let ends_today = end_local.is_some_and(|e| e.date() == d);
                    let in_progress = !all_day
                        && occ.start.instant(tz) <= o.now
                        && occ.end.is_some_and(|e| e.instant(tz) > o.now);
                    out.push(Entry {
                        uid: item.uid.clone(),
                        rid: occ.rid,
                        kind: Kind::Event,
                        title: occ.f.title.clone(),
                        list: item.list.clone(),
                        day: d,
                        start: if s_day == d { occ.start.time(tz) } else { None },
                        end: if ends_today && !all_day { end_local.map(|e| e.time()) } else { None },
                        continued: s_day < d,
                        all_day: all_day || (s_day < d && !ends_today),
                        done: false,
                        overdue: false,
                        priority: occ.f.priority,
                        recurring: item.is_recurring(),
                        progress: None,
                        due: None,
                        now: in_progress,
                    });
                }
            }
            Kind::Task => {
                if item.f.parent.is_some() {
                    continue;
                }
                let due_day = item.f.due.map(|m| m.date(tz));
                let done_day = item.f.completed.map(|c| c.with_timezone(&tz).date_naive());
                let show = if item.f.done {
                    o.done_visible(item) && (done_day == Some(d) || (done_day.is_none() && due_day == Some(d)))
                } else {
                    due_day == Some(d)
                };
                if show {
                    out.push(task_entry(store, item, d, o));
                }
            }
        }
    }
    sort_day(&mut out);
    out
}

/// Timed items by time, then all-day events, then tasks without a time.
fn sort_day(v: &mut [Entry]) {
    v.sort_by(|a, b| {
        let rank = |e: &Entry| match (e.start.is_some() || e.end.is_some() && !e.all_day, e.kind) {
            (true, _) => 0,
            (false, Kind::Event) => 1,
            (false, Kind::Task) => 2,
        };
        let at = |e: &Entry| e.start.or(e.end).unwrap_or(NaiveTime::MIN);
        rank(a)
            .cmp(&rank(b))
            .then(at(a).cmp(&at(b)))
            .then(a.done.cmp(&b.done))
            .then(b.priority.cmp(&a.priority))
            .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
}

/// Overdue tasks, then each of the next `days` days that has anything.
/// `overdue_collapse` limits the overdue section (0 = no limit).
pub fn agenda(store: &Store, from: NaiveDate, days: u32, o: &Opts, overdue_collapse: usize, show_all_overdue: bool) -> Vec<Section> {
    let today = o.today();
    let mut sections = Vec::new();
    if from <= today {
        let mut overdue: Vec<Entry> = store
            .items
            .iter()
            .filter(|i| i.kind == Kind::Task && !i.f.done && i.f.parent.is_none())
            .filter(|i| i.f.due.is_some_and(|d| d.date(o.tz) < today))
            .map(|i| task_entry(store, i, today, o))
            .collect();
        overdue.sort_by(|a, b| a.due.cmp(&b.due).then(b.priority.cmp(&a.priority)));
        let mut hidden = 0;
        if !show_all_overdue && overdue_collapse > 0 && overdue.len() > overdue_collapse {
            hidden = overdue.len() - overdue_collapse;
            overdue.truncate(overdue_collapse);
        }
        if !overdue.is_empty() {
            sections.push(Section { title: "Overdue".into(), day: None, entries: overdue, hidden });
        }
    }
    for k in 0..days.max(1) {
        let d = from + Duration::days(k as i64);
        let entries = day(store, d, o);
        if entries.is_empty() && d != today {
            continue;
        }
        sections.push(Section { title: day_title(d, today), day: Some(d), entries, hidden: 0 });
    }
    sections
}

/// `Today · Wed 15`, `Tomorrow · Thu 16`, `Sat 18`, `Mon 2 Nov`.
pub fn day_title(d: NaiveDate, today: NaiveDate) -> String {
    let short = d.format("%a %-d").to_string();
    if d == today {
        format!("Today · {short}")
    } else if d == today + Duration::days(1) {
        format!("Tomorrow · {short}")
    } else if d == today - Duration::days(1) {
        format!("Yesterday · {short}")
    } else if (d - today).num_days().abs() < 7 {
        short
    } else {
        d.format("%a %-d %b").to_string()
    }
}

/// Undated open tasks (the agenda's "Someday").
pub fn undated(store: &Store, o: &Opts) -> Vec<Entry> {
    let mut v: Vec<Entry> = store
        .items
        .iter()
        .filter(|i| i.kind == Kind::Task && i.f.parent.is_none() && i.f.due.is_none() && o.done_visible(i))
        .map(|i| task_entry(store, i, o.today(), o))
        .collect();
    v.sort_by(|a, b| a.done.cmp(&b.done).then(b.priority.cmp(&a.priority)).then(a.title.cmp(&b.title)));
    v
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    #[default]
    Due,
    Priority,
    Manual,
}

impl Sort {
    pub fn next(self) -> Self {
        match self {
            Sort::Due => Sort::Priority,
            Sort::Priority => Sort::Manual,
            Sort::Manual => Sort::Due,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Sort::Due => "due",
            Sort::Priority => "priority",
            Sort::Manual => "manual",
        }
    }
}

/// A row of the task list: a task and how deep it sits.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskRow {
    pub entry: Entry,
    pub depth: u8,
    pub has_children: bool,
}

fn created(i: &Item) -> String {
    i.master().get("CREATED").map(|p| p.value.clone()).unwrap_or_else(|| i.file.clone())
}

fn task_order(a: &Item, b: &Item, sort: Sort, tz: Tz) -> std::cmp::Ordering {
    let due = |i: &Item| i.f.due.map(|d| (d.date(tz), d.time(tz).unwrap_or(NaiveTime::from_hms_opt(23, 59, 59).expect("valid"))));
    let by_due = |a: &Item, b: &Item| match (due(a), due(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    };
    let done = a.f.done.cmp(&b.f.done);
    let rest = match sort {
        Sort::Due => by_due(a, b).then(b.f.priority.cmp(&a.f.priority)),
        Sort::Priority => b.f.priority.cmp(&a.f.priority).then(by_due(a, b)),
        Sort::Manual => a.f.sort.unwrap_or(i64::MAX).cmp(&b.f.sort.unwrap_or(i64::MAX)),
    };
    done.then(rest).then(created(a).cmp(&created(b)))
}

/// Tasks in `list` (all lists with `None`), parents followed by their subtasks.
pub fn tasks(store: &Store, list: Option<&str>, sort: Sort, o: &Opts) -> Vec<TaskRow> {
    let tz = o.tz;
    let mut top: Vec<&Item> = store
        .items
        .iter()
        .filter(|i| i.kind == Kind::Task && list.is_none_or(|l| i.list == l))
        .filter(|i| i.f.parent.as_ref().is_none_or(|p| store.get(p).is_none()))
        .filter(|i| o.done_visible(i))
        .collect();
    top.sort_by(|a, b| task_order(a, b, sort, tz));
    let mut out = Vec::new();
    for t in top {
        let mut kids: Vec<&Item> = store.items.iter().filter(|c| c.f.parent.as_deref() == Some(&t.uid)).collect();
        kids.sort_by(|a, b| task_order(a, b, Sort::Manual, tz));
        let day = t.f.due.map(|d| d.date(tz)).unwrap_or(o.today());
        out.push(TaskRow { entry: task_entry(store, t, day, o), depth: 0, has_children: !kids.is_empty() });
        for k in kids {
            let kd = k.f.due.map(|d| d.date(tz)).unwrap_or(day);
            out.push(TaskRow { entry: task_entry(store, k, kd, o), depth: 1, has_children: false });
        }
    }
    out
}

/// Lists that have something on each day of `[from, to)`, for month dots.
pub fn busy_days(store: &Store, from: NaiveDate, to: NaiveDate, o: &Opts) -> HashMap<NaiveDate, Vec<String>> {
    let tz = o.tz;
    let mut out: HashMap<NaiveDate, Vec<String>> = HashMap::new();
    for item in &store.items {
        match item.kind {
            Kind::Event => {
                for occ in recur::occurrences(item, from, to, tz) {
                    let first = occ.start.date(tz).max(from);
                    let last = occ
                        .end
                        .map(|e| if e.is_date() { e.date(tz) - Duration::days(1) } else { e.local(tz).date() })
                        .unwrap_or(first)
                        .min(to - Duration::days(1));
                    let mut d = first;
                    while d <= last {
                        out.entry(d).or_default().push(item.list.clone());
                        d += Duration::days(1);
                    }
                }
            }
            Kind::Task => {
                if item.f.done || item.f.parent.is_some() {
                    continue;
                }
                if let Some(d) = item.f.due.map(|m| m.date(tz)).filter(|d| *d >= from && *d < to) {
                    out.entry(d).or_default().push(item.list.clone());
                }
            }
        }
    }
    out
}

/// Case-insensitive fuzzy match: every query word is a subsequence of the
/// title or the list name.
pub fn matches(query: &str, title: &str, list: &str) -> bool {
    let hay = format!("{} #{}", title.to_lowercase(), list.to_lowercase());
    query
        .to_lowercase()
        .split_whitespace()
        .all(|w| hay.contains(w) || super::parse::subsequence(w, &hay))
}

/// How well `query` matches: contiguous beats scattered, earlier beats later.
pub fn score(query: &str, title: &str) -> Option<i64> {
    let q = query.to_lowercase();
    let t = title.to_lowercase();
    if q.is_empty() {
        return Some(0);
    }
    if t == q {
        return Some(1000);
    }
    if let Some(pos) = t.find(&q) {
        let word_start = pos == 0 || !t[..pos].ends_with(char::is_alphanumeric);
        return Some(500 - pos as i64 + if word_start { 100 } else { 0 });
    }
    super::parse::subsequence(&q, &t).then_some(10)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::core::ops::{self, Defaults, Env};
    use crate::core::undo::Journal;
    use chrono::TimeZone;

    #[test]
    fn agenda_sections_and_order() {
        let dir = std::env::temp_dir().join(format!("ctui-query-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut store = Store::open(&dir.join("data"), None).unwrap();
        let mut j = Journal::load(&dir.join("state"));
        let cfg = Config::default();
        let tz = Tz::Europe__London;
        let now = tz.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap();
        let env = Env { now, tz, cfg: &cfg };
        let d = Defaults::default();
        for t in [
            "renew passport 2026-10-13",
            "standup today 9:30-10:00",
            "pay rent today !",
            "dentist today 14:00-15:00",
            "send invoice tomorrow 17:00",
            "birthday allday sat",
            "someday thing",
            "lunch today 11:45-13:00",
        ] {
            ops::add(&mut store, &mut j, &env, t, None, &d).unwrap();
        }
        let o = Opts { now, tz, hide_done_after: Some(Duration::days(1)) };
        let secs = agenda(&store, o.today(), 7, &o, 5, false);
        let titles: Vec<&str> = secs.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, vec!["Overdue", "Today · Thu 15", "Tomorrow · Fri 16", "Sat 17"]);
        let today: Vec<&str> = secs[1].entries.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(today, vec!["standup", "lunch", "dentist", "pay rent"]);
        assert!(secs[1].entries[1].now, "lunch is happening now");
        assert_eq!(undated(&store, &o).len(), 1);
        let busy = busy_days(&store, NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(), NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(), &o);
        assert_eq!(busy[&NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()].len(), 4);
        let rows = tasks(&store, None, Sort::Due, &o);
        assert_eq!(rows.first().unwrap().entry.title, "renew passport");
        assert_eq!(rows.last().unwrap().entry.title, "someday thing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fuzzy() {
        assert!(matches("mlk", "buy milk", "home"));
        assert!(matches("milk #home", "buy milk", "home"));
        assert!(!matches("bread", "buy milk", "home"));
        assert!(score("milk", "buy milk") > score("mk", "buy milk"));
    }
}
