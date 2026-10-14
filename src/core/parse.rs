//! The quick-add grammar (DESIGN.md §5), shared by `ct add`, the TUI add bar,
//! edit, move and go-to-date.
//!
//! Words that aren't tokens make up the title, in any order. Every token the
//! parser recognises is reported as a [`Span`] so the add bar can colour it,
//! and [`Draft::preview`] says in words what will be created.

use std::ops::Range;

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, Weekday};
use chrono_tz::Tz;

use super::model::{Kind, List};
use super::time::{add_months, last_of_month, week_start_of};
use crate::config::{DateOrder, TimeFormat};

/// What the parser needs to know about the world.
#[derive(Clone)]
pub struct Ctx<'a> {
    pub now: DateTime<Tz>,
    pub date_order: DateOrder,
    pub week_start: Weekday,
    pub lists: &'a [List],
}

impl Ctx<'_> {
    pub fn today(&self) -> NaiveDate {
        self.now.date_naive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Date,
    Time,
    List,
    Priority,
    Repeat,
    Remind,
    Flag,
    /// A token that was recognised but is wrong (an unknown list).
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub role: Role,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListPick {
    Existing(String),
    /// `#name!`: create it.
    Create(String),
    Unknown(String),
    Ambiguous(String, Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Repeat {
    Rule(String),
    /// `every custom`: keep whatever rule the item already has.
    Keep,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Draft {
    pub title: String,
    pub date: Option<NaiveDate>,
    /// Last day of an all-day span (inclusive).
    pub end_date: Option<NaiveDate>,
    pub time: Option<NaiveTime>,
    pub end_time: Option<NaiveTime>,
    pub length: Option<Duration>,
    pub allday: bool,
    pub list: Option<ListPick>,
    pub priority: Option<u8>,
    pub repeat: Option<Repeat>,
    pub remind: Option<Duration>,
    /// The date wasn't typed: it came from a time alone (today) or a repeat.
    pub implied_date: bool,
    pub spans: Vec<Span>,
}

impl Draft {
    /// Event when it has a range, a length, a span or `allday`.
    pub fn kind(&self) -> Kind {
        if self.end_time.is_some() || self.length.is_some() || self.allday || self.end_date.is_some() {
            Kind::Event
        } else {
            Kind::Task
        }
    }

    /// Why this can't be saved as it stands, if anything.
    pub fn problem(&self) -> Option<String> {
        match &self.list {
            Some(ListPick::Unknown(n)) => return Some(format!("no list #{n} (#{n}! creates it)")),
            Some(ListPick::Ambiguous(n, ids)) => {
                return Some(format!("#{n} could be {}", ids.iter().map(|i| format!("#{i}")).collect::<Vec<_>>().join(" or ")));
            }
            _ => {}
        }
        if self.title.trim().is_empty() {
            return Some("needs a title".into());
        }
        if let (Some(a), Some(b)) = (self.date, self.end_date) {
            if b < a {
                return Some("the span ends before it starts".into());
            }
        }
        None
    }

    /// One line in words: `task · due Fri 17 Oct 15:00 · #work · !`.
    pub fn preview(&self, kind: Kind, fmt: TimeFormat, list_name: Option<&str>) -> String {
        let mut parts = vec![kind.name().to_string()];
        let day = |d: NaiveDate| d.format("%a %-d %b").to_string();
        match kind {
            Kind::Task => match (self.date, self.time) {
                (Some(d), Some(t)) => parts.push(format!("due {} {}", day(d), fmt_time(t, fmt))),
                (Some(d), None) => parts.push(format!("due {}", day(d))),
                (None, _) => parts.push("no date".into()),
            },
            Kind::Event => {
                let d = self.date.map(day).unwrap_or_else(|| "today".into());
                if let Some(end) = self.end_date.filter(|e| Some(*e) != self.date) {
                    parts.push(format!("{d} – {} all day", day(end)));
                } else if self.allday || self.time.is_none() {
                    parts.push(format!("{d} all day"));
                } else {
                    let t = self.time.expect("checked");
                    let end = self.end_time.or_else(|| self.length.map(|l| t + l));
                    match end {
                        Some(e) => parts.push(format!("{d} {}–{}", fmt_time(t, fmt), fmt_time(e, fmt))),
                        None => parts.push(format!("{d} {}", fmt_time(t, fmt))),
                    }
                }
            }
        }
        match &self.list {
            Some(ListPick::Existing(id)) => parts.push(format!("#{id}")),
            Some(ListPick::Create(id)) => parts.push(format!("#{id} (new list)")),
            _ => {
                if let Some(n) = list_name {
                    parts.push(format!("#{n}"));
                }
            }
        }
        if let Some(p) = self.priority.filter(|p| *p > 0) {
            parts.push("!".repeat(p as usize));
        }
        match &self.repeat {
            Some(Repeat::Rule(r)) => {
                parts.push(super::recur::describe(r).unwrap_or_else(|| "repeats".into()));
            }
            Some(Repeat::Keep) => parts.push("repeats as before".into()),
            None => {}
        }
        if let Some(r) = self.remind {
            parts.push(format!("remind {} before", fmt_length(r)));
        }
        parts.join(" · ")
    }
}

pub fn fmt_time(t: NaiveTime, fmt: TimeFormat) -> String {
    match fmt {
        TimeFormat::H24 => t.format("%H:%M").to_string(),
        TimeFormat::H12 => {
            if t.format("%M").to_string() == "00" {
                t.format("%-I%P").to_string()
            } else {
                t.format("%-I:%M%P").to_string()
            }
        }
    }
}

pub fn fmt_length(d: Duration) -> String {
    let mins = d.num_minutes();
    if mins % 1440 == 0 && mins >= 1440 {
        format!("{}d", mins / 1440)
    } else if mins % 60 == 0 && mins >= 60 {
        format!("{}h", mins / 60)
    } else if mins > 60 {
        format!("{}h{}m", mins / 60, mins % 60)
    } else {
        format!("{mins}m")
    }
}

#[derive(Debug, Clone)]
struct Tok {
    text: String,
    lower: String,
    range: Range<usize>,
    quoted: bool,
}

fn tokenize(input: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut chars = input.char_indices().peekable();
    while let Some(&(i, c)) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        if c == '"' {
            chars.next();
            let mut text = String::new();
            let mut end = input.len();
            for (j, c) in chars.by_ref() {
                if c == '"' {
                    end = j + 1;
                    break;
                }
                text.push(c);
            }
            out.push(Tok { lower: text.to_lowercase(), text, range: i..end, quoted: true });
            continue;
        }
        let mut end = input.len();
        let mut text = String::new();
        while let Some(&(j, c)) = chars.peek() {
            if c.is_whitespace() {
                end = j;
                break;
            }
            text.push(c);
            chars.next();
        }
        out.push(Tok { lower: text.to_lowercase(), text, range: i..end, quoted: false });
    }
    out
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

fn month(s: &str) -> Option<u32> {
    let s = s.trim_end_matches(['.', ',']);
    if s.len() < 3 {
        return None;
    }
    const FULL: [&str; 12] = [
        "january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november",
        "december",
    ];
    if s == "sept" {
        return Some(9);
    }
    FULL.iter().position(|m| m.starts_with(s)).map(|i| i as u32 + 1)
}

pub fn weekday(s: &str) -> Option<Weekday> {
    Some(match s.trim_end_matches(',') {
        "mon" | "monday" => Weekday::Mon,
        "tue" | "tues" | "tuesday" => Weekday::Tue,
        "wed" | "weds" | "wednesday" => Weekday::Wed,
        "thu" | "thur" | "thurs" | "thursday" => Weekday::Thu,
        "fri" | "friday" => Weekday::Fri,
        "sat" | "saturday" => Weekday::Sat,
        "sun" | "sunday" => Weekday::Sun,
        _ => return None,
    })
}

fn day_number(s: &str) -> Option<u32> {
    let s = s.trim_end_matches(',');
    let s = ["st", "nd", "rd", "th"].iter().find_map(|x| s.strip_suffix(x)).unwrap_or(s);
    let n: u32 = s.parse().ok()?;
    (1..=31).contains(&n).then_some(n)
}

fn year(s: &str) -> Option<i32> {
    let n: i32 = s.trim_end_matches(',').parse().ok()?;
    (1900..=2200).contains(&n).then_some(n)
}

/// The next date (from today, inclusive) that is `wd`.
fn next_weekday(today: NaiveDate, wd: Weekday) -> NaiveDate {
    let ahead = (7 + wd.num_days_from_monday() - today.weekday().num_days_from_monday()) % 7;
    today + Duration::days(ahead as i64)
}

/// A month and day without a year: this year, or next if it already passed.
fn forward_md(today: NaiveDate, m: u32, d: u32) -> Option<NaiveDate> {
    let this = NaiveDate::from_ymd_opt(today.year(), m, d);
    match this {
        Some(x) if x >= today => Some(x),
        _ => NaiveDate::from_ymd_opt(today.year() + 1, m, d).or(this),
    }
}

/// `1h`, `45m`, `1h30m`, `90min`, `2hrs`, `1.5h`, `2d`.
pub fn parse_length(s: &str) -> Option<Duration> {
    let s = s.trim().to_lowercase();
    if s.is_empty() || !s.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let mut total = Duration::zero();
    let mut num = String::new();
    let mut rest = s.as_str();
    let mut any = false;
    while !rest.is_empty() {
        let n_end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(rest.len());
        num.clear();
        num.push_str(&rest[..n_end]);
        rest = &rest[n_end..];
        let u_end = rest.find(|c: char| c.is_ascii_digit()).unwrap_or(rest.len());
        let unit = &rest[..u_end];
        rest = &rest[u_end..];
        let n: f64 = num.parse().ok()?;
        let mins = match unit {
            "m" | "min" | "mins" | "minute" | "minutes" => n,
            "h" | "hr" | "hrs" | "hour" | "hours" => n * 60.0,
            "d" | "day" | "days" => n * 1440.0,
            "w" | "wk" | "week" | "weeks" => n * 10080.0,
            _ => return None,
        };
        total += Duration::minutes(mins.round() as i64);
        any = true;
    }
    (any && total > Duration::zero()).then_some(total)
}

/// One time of day: `3pm`, `3:30pm`, `15:00`, `9.30`, `noon`, `midnight`.
/// `meridiem` is what was written (to share it across a range).
fn time_of(s: &str) -> Option<(NaiveTime, Option<bool>)> {
    let s = s.trim_end_matches(',');
    match s {
        "noon" | "midday" => return Some((NaiveTime::from_hms_opt(12, 0, 0)?, None)),
        "midnight" => return Some((NaiveTime::MIN, None)),
        _ => {}
    }
    let (body, pm) = if let Some(b) = s.strip_suffix("pm").or_else(|| s.strip_suffix('p')) {
        (b, Some(true))
    } else if let Some(b) = s.strip_suffix("am").or_else(|| s.strip_suffix('a')) {
        (b, Some(false))
    } else {
        (s, None)
    };
    if body.is_empty() || !body.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let (h, m) = match body.split_once([':', '.']) {
        Some((h, m)) => {
            if m.len() != 2 {
                return None;
            }
            (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?)
        }
        None => {
            // A bare number is only a time with am/pm ("3pm"), never "3".
            pm?;
            if body.len() > 2 {
                return None;
            }
            (body.parse::<u32>().ok()?, 0)
        }
    };
    if body.contains('.') && pm.is_none() && h > 23 {
        return None;
    }
    let h = match pm {
        Some(true) if (1..=11).contains(&h) => h + 12,
        Some(true) if h == 12 => 12,
        Some(false) if h == 12 => 0,
        Some(_) if !(1..=12).contains(&h) => return None,
        _ => h,
    };
    Some((NaiveTime::from_hms_opt(h, m, 0)?, pm))
}

/// `3-4pm`, `3pm-4pm`, `14:00-15:30`, `11-1pm`.
fn time_range(s: &str) -> Option<(NaiveTime, NaiveTime)> {
    // Bare "9-17" isn't a range: it needs a ':' or am/pm somewhere.
    if !(s.contains(':') || s.ends_with('m') || s.contains("m-") || s.ends_with('p') || s.ends_with('a')) {
        return None;
    }
    let (a, b) = s.split_once(['-', '–'])?;
    let (end, end_pm) = time_of(b)?;
    let start = match time_of(a) {
        Some((t, _)) => t,
        None => {
            // "3-4pm": the start borrows the end's am/pm.
            let pm = end_pm?;
            let (t, _) = time_of(&format!("{a}{}", if pm { "pm" } else { "am" }))?;
            if t > end { time_of(&format!("{a}am"))?.0 } else { t }
        }
    };
    Some((start, end))
}

struct Parser<'a, 'c> {
    ctx: &'a Ctx<'c>,
    toks: Vec<Tok>,
    used: Vec<Option<Role>>,
    d: Draft,
    /// Length tokens, kept only if there's also a time.
    lengths: Vec<Range<usize>>,
}

impl Parser<'_, '_> {
    fn lower(&self, i: usize) -> Option<&str> {
        self.toks.get(i).filter(|t| !t.quoted).map(|t| t.lower.as_str())
    }

    /// A single date at `i`: returns (date, tokens used).
    fn date_at(&self, i: usize) -> Option<(NaiveDate, usize)> {
        let today = self.ctx.today();
        let w = self.lower(i)?;
        let single = match w {
            "today" | "tod" | "tdy" => Some(today),
            "tomorrow" | "tom" | "tmr" | "tmrw" => Some(today + Duration::days(1)),
            "yesterday" => Some(today - Duration::days(1)),
            "eom" => Some(last_of_month(today)),
            "eow" => Some(week_start_of(today, self.ctx.week_start) + Duration::days(6)),
            "eoy" => NaiveDate::from_ymd_opt(today.year(), 12, 31),
            _ => None,
        };
        if let Some(d) = single {
            return Some((d, 1));
        }
        if let Some(wd) = weekday(w) {
            return Some((next_weekday(today, wd), 1));
        }
        if w == "next" {
            let n = self.lower(i + 1)?;
            if let Some(wd) = weekday(n) {
                return Some((next_weekday(today, wd) + Duration::days(7), 2));
            }
            return match n {
                "week" => Some((week_start_of(today, self.ctx.week_start) + Duration::days(7), 2)),
                "month" => Some((add_months(today.with_day(1)?, 1), 2)),
                "year" => Some((NaiveDate::from_ymd_opt(today.year() + 1, 1, 1)?, 2)),
                _ => None,
            };
        }
        if w == "in" {
            let n: i64 = self.lower(i + 1)?.parse().ok()?;
            let unit = self.lower(i + 2)?;
            let d = match unit.trim_end_matches('s') {
                "day" => today + Duration::days(n),
                "week" => today + Duration::weeks(n),
                "month" => add_months(today, n as i32),
                "year" => add_months(today, n as i32 * 12),
                _ => return None,
            };
            return Some((d, 3));
        }
        if let Some(rest) = w.strip_prefix('+') {
            let (num, unit) = rest.split_at(rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len()));
            let n: i64 = num.parse().ok()?;
            let d = match unit {
                "" | "d" => today + Duration::days(n),
                "w" => today + Duration::weeks(n),
                "m" => add_months(today, n as i32),
                "y" => add_months(today, n as i32 * 12),
                _ => return None,
            };
            return Some((d, 1));
        }
        // 2026-10-20
        if let Ok(d) = NaiveDate::parse_from_str(w, "%Y-%m-%d") {
            return Some((d, 1));
        }
        // oct 20 [2027] / oct 20th
        if let Some(m) = month(w) {
            let day = day_number(self.lower(i + 1)?)?;
            if let Some(y) = self.lower(i + 2).and_then(year) {
                return Some((NaiveDate::from_ymd_opt(y, m, day)?, 3));
            }
            return Some((forward_md(today, m, day)?, 2));
        }
        // 20 oct [2027]
        if let Some(day) = day_number(w) {
            if let Some(m) = self.lower(i + 1).and_then(month) {
                if let Some(y) = self.lower(i + 2).and_then(year) {
                    return Some((NaiveDate::from_ymd_opt(y, m, day)?, 3));
                }
                return Some((forward_md(today, m, day)?, 2));
            }
        }
        // 20/10, 20/10/2026, 10/20 (mdy)
        if w.contains('/') {
            let parts: Vec<&str> = w.split('/').collect();
            let nums: Option<Vec<i32>> = parts.iter().map(|p| p.parse().ok()).collect();
            let nums = nums?;
            let (y, m, d) = match (nums.as_slice(), self.ctx.date_order) {
                ([a, b], DateOrder::Dmy) => (None, *b, *a),
                ([a, b], DateOrder::Mdy | DateOrder::Ymd) => (None, *a, *b),
                ([a, b, c], DateOrder::Dmy) => (Some(*c), *b, *a),
                ([a, b, c], DateOrder::Mdy) => (Some(*c), *a, *b),
                ([a, b, c], DateOrder::Ymd) => (Some(*a), *b, *c),
                _ => return None,
            };
            let (m, d) = (u32::try_from(m).ok()?, u32::try_from(d).ok()?);
            let date = match y {
                Some(y) => NaiveDate::from_ymd_opt(if y < 100 { 2000 + y } else { y }, m, d)?,
                None => forward_md(today, m, d)?,
            };
            return Some((date, 1));
        }
        None
    }

    /// `oct 20-22` / `20-22 oct` / `<date> to <date>`: (first, last, tokens).
    fn span_at(&self, i: usize) -> Option<(NaiveDate, NaiveDate, usize)> {
        let today = self.ctx.today();
        let w = self.lower(i)?;
        let range_days = |s: &str| -> Option<(u32, u32)> {
            let (a, b) = s.split_once(['-', '–'])?;
            Some((day_number(a)?, day_number(b)?))
        };
        if let Some(m) = month(w) {
            if let Some((a, b)) = self.lower(i + 1).and_then(range_days) {
                let first = forward_md(today, m, a)?;
                let last = NaiveDate::from_ymd_opt(first.year(), m, b)?;
                return Some((first, last, 2));
            }
        }
        if let Some((a, b)) = range_days(w) {
            if let Some(m) = self.lower(i + 1).and_then(month) {
                let first = forward_md(today, m, a)?;
                let last = NaiveDate::from_ymd_opt(first.year(), m, b)?;
                return Some((first, last, 2));
            }
        }
        let (first, n) = self.date_at(i)?;
        if matches!(self.lower(i + n), Some("to" | "until" | "-" | "–" | "through" | "thru")) {
            let (mut last, m) = self.date_at(i + n + 1)?;
            if last < first && self.lower(i + n + 1).and_then(month).is_some() {
                last = NaiveDate::from_ymd_opt(last.year() + 1, last.month(), last.day())?;
            }
            return Some((first, last, n + 1 + m));
        }
        None
    }

    /// `every …`: (RRULE, tokens).
    fn repeat_at(&self, i: usize) -> Option<(Repeat, usize)> {
        if self.lower(i)? != "every" {
            return None;
        }
        let mut j = i + 1;
        let mut interval = 1u32;
        if let Some(n) = self.lower(j).and_then(|w| w.parse::<u32>().ok()).filter(|n| *n >= 1) {
            interval = n;
            j += 1;
        } else if self.lower(j) == Some("other") {
            interval = 2;
            j += 1;
        }
        let word = self.lower(j)?;
        let with_interval = |freq: &str| {
            if interval == 1 { format!("FREQ={freq}") } else { format!("FREQ={freq};INTERVAL={interval}") }
        };
        let unit = word.trim_end_matches('s');
        let rule = match unit {
            "custom" if interval == 1 => return Some((Repeat::Keep, j + 1 - i)),
            "day" => with_interval("DAILY"),
            "weekday" if interval == 1 => "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR".into(),
            "week" => with_interval("WEEKLY"),
            "year" => with_interval("YEARLY"),
            "month" => {
                // every month on 1 / on the 1st
                let mut k = j + 1;
                if self.lower(k) == Some("on") {
                    k += 1;
                    if self.lower(k) == Some("the") {
                        k += 1;
                    }
                    if let Some(d) = self.lower(k).and_then(day_number) {
                        let base = with_interval("MONTHLY");
                        return Some((Repeat::Rule(format!("{base};BYMONTHDAY={d}")), k + 1 - i));
                    }
                }
                with_interval("MONTHLY")
            }
            _ => {
                // every mon,wed / every mon and thu / every tue
                if interval != 1 {
                    return None;
                }
                let mut days = Vec::new();
                let mut k = j;
                loop {
                    let Some(w) = self.lower(k) else { break };
                    let mut any = false;
                    for part in w.split(',').filter(|p| !p.is_empty()) {
                        match weekday(part) {
                            Some(d) => {
                                days.push(d);
                                any = true;
                            }
                            None => return None,
                        }
                    }
                    if !any {
                        break;
                    }
                    k += 1;
                    if self.lower(k) == Some("and") && self.lower(k + 1).and_then(weekday).is_some() {
                        k += 1;
                    } else if !w.ends_with(',') {
                        break;
                    }
                }
                if days.is_empty() {
                    return None;
                }
                let codes: Vec<&str> = days.iter().map(|d| ical_day(*d)).collect();
                return Some((Repeat::Rule(format!("FREQ=WEEKLY;BYDAY={}", codes.join(","))), k - i));
            }
        };
        Some((Repeat::Rule(rule), j + 1 - i))
    }

    fn pick_list(&self, name: &str) -> ListPick {
        if let Some(id) = name.strip_suffix('!') {
            if let Some(l) = self.ctx.lists.iter().find(|l| l.id.eq_ignore_ascii_case(id)) {
                return ListPick::Existing(l.id.clone());
            }
            return ListPick::Create(id.to_string());
        }
        let n = name.to_lowercase();
        let lists = self.ctx.lists;
        if let Some(l) = lists.iter().find(|l| l.id.to_lowercase() == n || l.name.to_lowercase() == n) {
            return ListPick::Existing(l.id.clone());
        }
        let prefix: Vec<&List> =
            lists.iter().filter(|l| l.id.to_lowercase().starts_with(&n) || l.name.to_lowercase().starts_with(&n)).collect();
        let pool: Vec<&List> = if prefix.is_empty() {
            lists.iter().filter(|l| subsequence(&n, &l.id.to_lowercase())).collect()
        } else {
            prefix
        };
        match pool.as_slice() {
            [one] => ListPick::Existing(one.id.clone()),
            [] => ListPick::Unknown(name.to_string()),
            many => ListPick::Ambiguous(name.to_string(), many.iter().map(|l| l.id.clone()).collect()),
        }
    }

    fn mark(&mut self, i: usize, n: usize, role: Role) {
        for k in i..i + n {
            self.used[k] = Some(role);
        }
    }

    /// Tries every token form at `i`; returns how many tokens it used.
    fn step(&mut self, i: usize) -> usize {
        let Some(w) = self.lower(i).map(str::to_string) else { return 0 };
        if let Some((r, n)) = self.repeat_at(i) {
            self.d.repeat = Some(r);
            self.mark(i, n, Role::Repeat);
            return n;
        }
        if w == "remind" || w == "reminder" {
            if let Some(len) = self.lower(i + 1).and_then(parse_length) {
                self.d.remind = Some(len);
                self.mark(i, 2, Role::Remind);
                return 2;
            }
        }
        if w == "for" {
            if let Some(len) = self.lower(i + 1).and_then(parse_length) {
                self.d.length = Some(len);
                self.mark(i, 2, Role::Time);
                self.lengths.push(i..i + 2);
                return 2;
            }
        }
        if matches!(w.as_str(), "on" | "at" | "by" | "due" | "from") {
            let save = (self.d.clone(), self.used.clone(), self.lengths.clone());
            let n = self.step_core(i + 1);
            if n > 0 {
                let role = self.used[i + 1].unwrap_or(Role::Date);
                self.used[i] = Some(role);
                return n + 1;
            }
            (self.d, self.used, self.lengths) = save;
        }
        self.step_core(i)
    }

    fn step_core(&mut self, i: usize) -> usize {
        let Some(w) = self.lower(i).map(str::to_string) else { return 0 };
        if let Some((a, b, n)) = self.span_at(i) {
            self.d.date = Some(a);
            self.d.end_date = Some(b);
            self.mark(i, n, Role::Date);
            return n;
        }
        if let Some((d, n)) = self.date_at(i) {
            self.d.date = Some(d);
            self.mark(i, n, Role::Date);
            return n;
        }
        if let Some((a, b)) = time_range(&w) {
            self.d.time = Some(a);
            self.d.end_time = Some(b);
            self.mark(i, 1, Role::Time);
            return 1;
        }
        if let Some((t, _)) = time_of(&w) {
            // 3pm - 4pm / 3pm to 4pm
            if matches!(self.lower(i + 1), Some("-" | "–" | "to" | "until")) {
                if let Some((e, _)) = self.lower(i + 2).and_then(time_of) {
                    self.d.time = Some(t);
                    self.d.end_time = Some(e);
                    self.mark(i, 3, Role::Time);
                    return 3;
                }
            }
            self.d.time = Some(t);
            self.mark(i, 1, Role::Time);
            return 1;
        }
        if let Some(len) = parse_length(&w) {
            self.d.length = Some(len);
            self.mark(i, 1, Role::Time);
            self.lengths.push(i..i + 1);
            return 1;
        }
        if w == "allday" || w == "all-day" {
            self.d.allday = true;
            self.mark(i, 1, Role::Flag);
            return 1;
        }
        if let Some(name) = w.strip_prefix('#').filter(|n| n.len() > 1 || (!n.is_empty() && *n != "!")) {
            let original = &self.toks[i].text[1..];
            let pick = self.pick_list(if original.to_lowercase() == name { original } else { name });
            let ok = matches!(pick, ListPick::Existing(_) | ListPick::Create(_));
            self.d.list = Some(pick);
            self.mark(i, 1, if ok { Role::List } else { Role::Error });
            return 1;
        }
        if !w.is_empty() && w.chars().all(|c| c == '!') && w.len() <= 3 {
            self.d.priority = Some(w.len() as u8);
            self.mark(i, 1, Role::Priority);
            return 1;
        }
        0
    }
}

fn ical_day(d: Weekday) -> &'static str {
    match d {
        Weekday::Mon => "MO",
        Weekday::Tue => "TU",
        Weekday::Wed => "WE",
        Weekday::Thu => "TH",
        Weekday::Fri => "FR",
        Weekday::Sat => "SA",
        Weekday::Sun => "SU",
    }
}

pub fn subsequence(needle: &str, hay: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|c| it.any(|h| h == c))
}

/// Parses quick-add text.
pub fn parse(input: &str, ctx: &Ctx) -> Draft {
    let toks = tokenize(input);
    let n = toks.len();
    let mut p = Parser { ctx, used: vec![None; n], toks, d: Draft::default(), lengths: Vec::new() };
    let mut i = 0;
    while i < n {
        if p.toks[i].quoted {
            i += 1;
            continue;
        }
        let used = p.step(i);
        i += used.max(1);
    }
    // A length without a start time is just words ("run 5k", "read 30m").
    if p.d.time.is_none() && p.d.length.is_some() {
        p.d.length = None;
        for r in std::mem::take(&mut p.lengths) {
            for k in r {
                p.used[k] = None;
            }
        }
    }
    let today = ctx.today();
    // A repeat without a date starts at its first occurrence.
    if p.d.date.is_none() {
        if let Some(Repeat::Rule(rule)) = &p.d.repeat {
            p.d.date = first_from_rule(rule, today);
            p.d.implied_date = p.d.date.is_some();
        }
    }
    if p.d.time.is_some() && p.d.date.is_none() {
        p.d.date = Some(today);
        p.d.implied_date = true;
    }
    let title: Vec<&str> = p
        .toks
        .iter()
        .zip(&p.used)
        .filter(|(_, u)| u.is_none())
        .map(|(t, _)| t.text.as_str())
        .collect();
    p.d.title = title.join(" ");
    let mut spans: Vec<Span> = Vec::new();
    for (t, u) in p.toks.iter().zip(&p.used) {
        if let Some(role) = u {
            match spans.last_mut() {
                Some(s) if s.role == *role && input[s.range.end..t.range.start].trim().is_empty() => {
                    s.range.end = t.range.end;
                }
                _ => spans.push(Span { range: t.range.clone(), role: *role }),
            }
        }
    }
    p.d.spans = spans;
    p.d
}

fn first_from_rule(rule: &str, today: NaiveDate) -> Option<NaiveDate> {
    if let Some(days) = rule.split(';').find_map(|p| p.strip_prefix("BYDAY=")) {
        return (0..7)
            .map(|k| today + Duration::days(k))
            .find(|d| days.split(',').any(|c| c == ical_day(d.weekday())));
    }
    if let Some(day) = rule.split(';').find_map(|p| p.strip_prefix("BYMONTHDAY=")).and_then(|d| d.parse::<u32>().ok()) {
        for k in 0..13 {
            let base = add_months(today.with_day(1)?, k);
            if let Some(d) = NaiveDate::from_ymd_opt(base.year(), base.month(), day) {
                if d >= today {
                    return Some(d);
                }
            }
        }
    }
    None
}

/// A date for "go to": the same grammar, just the date part.
pub fn parse_date(input: &str, ctx: &Ctx) -> Option<NaiveDate> {
    let d = parse(input, ctx);
    d.date.filter(|_| d.title.is_empty())
}

/// Writes a date the grammar reads back to the same day: `oct 20` when that
/// resolves forward to it, else `2026-10-20`.
pub fn grammar_date(d: NaiveDate, today: NaiveDate) -> String {
    if d == today {
        return "today".into();
    }
    if d == today + Duration::days(1) {
        return "tomorrow".into();
    }
    if forward_md(today, d.month(), d.day()) == Some(d) {
        format!("{} {}", MONTHS[d.month0() as usize], d.day())
    } else {
        d.format("%Y-%m-%d").to_string()
    }
}

pub fn grammar_time(t: NaiveTime, fmt: TimeFormat) -> String {
    match fmt {
        TimeFormat::H24 => t.format("%H:%M").to_string(),
        TimeFormat::H12 => fmt_time(t, fmt),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn lists() -> Vec<List> {
        ["personal", "work", "health", "home"]
            .iter()
            .map(|n| List { id: n.to_string(), name: n.to_string(), color: None })
            .collect()
    }

    /// Wednesday 15 October 2026, noon.
    fn ctx(l: &[List]) -> Ctx<'_> {
        Ctx {
            now: Tz::Europe__London.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap(),
            date_order: DateOrder::Dmy,
            week_start: Weekday::Mon,
            lists: l,
        }
    }

    fn d(m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, m, day).unwrap()
    }

    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    fn p(s: &str) -> Draft {
        let l = lists();
        parse(s, &ctx(&l))
    }

    #[test]
    fn plain_title_is_an_undated_task() {
        let x = p("buy milk");
        assert_eq!(x.title, "buy milk");
        assert_eq!(x.kind(), Kind::Task);
        assert_eq!(x.date, None);
        assert!(x.spans.is_empty());
    }

    #[test]
    fn dates() {
        let cases = [
            ("a today", d(10, 15)),
            ("a tod", d(10, 15)),
            ("a tom", d(10, 16)),
            ("a thu", d(10, 15)),
            ("a fri", d(10, 16)),
            ("a wed", d(10, 21)),
            ("a next fri", d(10, 23)),
            ("a oct 20", d(10, 20)),
            ("a 20 oct", d(10, 20)),
            ("a october 20th", d(10, 20)),
            ("a 20/10", d(10, 20)),
            ("a 2026-12-24", d(12, 24)),
            ("a +3d", d(10, 18)),
            ("a +2w", d(10, 29)),
            ("a +1m", d(11, 15)),
            ("a eom", d(10, 31)),
            ("a eow", d(10, 18)),
            ("a in 3 days", d(10, 18)),
            ("a next week", d(10, 19)),
            ("a on fri", d(10, 16)),
            ("a due fri", d(10, 16)),
        ];
        for (s, want) in cases {
            let x = p(s);
            assert_eq!(x.date, Some(want), "{s}");
            assert_eq!(x.title, "a", "{s}");
        }
    }

    #[test]
    fn thursday_is_today_on_a_thursday() {
        // 15 Oct 2026 is a Thursday.
        assert_eq!(d(10, 15).weekday(), Weekday::Thu);
        assert_eq!(p("x thu").date, Some(d(10, 15)));
    }

    #[test]
    fn past_month_day_rolls_to_next_year() {
        assert_eq!(p("x mar 3").date, NaiveDate::from_ymd_opt(2027, 3, 3));
        assert_eq!(p("x 3/3").date, NaiveDate::from_ymd_opt(2027, 3, 3));
        assert_eq!(p("x mar 3 2026").date, Some(d(3, 3)));
    }

    #[test]
    fn mdy_order() {
        let l = lists();
        let mut c = ctx(&l);
        c.date_order = DateOrder::Mdy;
        assert_eq!(parse("x 10/20", &c).date, Some(d(10, 20)));
    }

    #[test]
    fn times() {
        let cases = [
            ("a 3pm", t(15, 0)),
            ("a 3:30pm", t(15, 30)),
            ("a 15:00", t(15, 0)),
            ("a 9.30", t(9, 30)),
            ("a noon", t(12, 0)),
            ("a 12am", t(0, 0)),
            ("a 12pm", t(12, 0)),
            ("a at 9am", t(9, 0)),
        ];
        for (s, want) in cases {
            let x = p(s);
            assert_eq!(x.time, Some(want), "{s}");
            assert_eq!(x.kind(), Kind::Task, "{s}");
            assert_eq!(x.date, Some(d(10, 15)), "time alone means today: {s}");
            assert_eq!(x.title, "a", "{s}");
        }
        assert_eq!(p("call 3 people").time, None);
        assert_eq!(p("call 3 people").title, "call 3 people");
    }

    #[test]
    fn ranges_make_events() {
        let x = p("dentist fri 2-3pm #health");
        assert_eq!(x.kind(), Kind::Event);
        assert_eq!((x.time, x.end_time), (Some(t(14, 0)), Some(t(15, 0))));
        assert_eq!(x.date, Some(d(10, 16)));
        assert_eq!(x.list, Some(ListPick::Existing("health".into())));
        assert_eq!(x.title, "dentist");
        assert_eq!(p("x 11-1pm").time, Some(t(11, 0)));
        assert_eq!(p("x 14:00-15:30").end_time, Some(t(15, 30)));
        assert_eq!(p("x 3pm - 4pm").end_time, Some(t(16, 0)));
        assert_eq!(p("pages 9-17").kind(), Kind::Task);
    }

    #[test]
    fn lengths_need_a_time() {
        let x = p("standup 9:30 15m");
        assert_eq!(x.kind(), Kind::Event);
        assert_eq!(x.length, Some(Duration::minutes(15)));
        let y = p("call 3pm for 1h30m");
        assert_eq!(y.length, Some(Duration::minutes(90)));
        assert_eq!(y.title, "call");
        let z = p("read 30m");
        assert_eq!(z.kind(), Kind::Task);
        assert_eq!(z.title, "read 30m");
        assert_eq!(p("run for 5k").title, "run for 5k");
    }

    #[test]
    fn all_day_and_spans() {
        let x = p("birthday allday sat");
        assert_eq!(x.kind(), Kind::Event);
        assert!(x.allday);
        assert_eq!(x.date, Some(d(10, 17)));
        let y = p("trip oct 20-22");
        assert_eq!((y.date, y.end_date), (Some(d(10, 20)), Some(d(10, 22))));
        assert_eq!(y.kind(), Kind::Event);
        let z = p("trip oct 30 to nov 2");
        assert_eq!((z.date, z.end_date), (Some(d(10, 30)), Some(d(11, 2))));
        assert_eq!(z.title, "trip");
    }

    #[test]
    fn lists_priority_quotes() {
        assert_eq!(p("x #wo").list, Some(ListPick::Existing("work".into())));
        assert_eq!(p("x #h").list, Some(ListPick::Ambiguous("h".into(), vec!["health".into(), "home".into()])));
        assert_eq!(p("x #gym!").list, Some(ListPick::Create("gym".into())));
        assert_eq!(p("x #nope").list, Some(ListPick::Unknown("nope".into())));
        assert!(p("x #nope").problem().is_some());
        assert_eq!(p("rent !!").priority, Some(2));
        let q = p("\"fri\" night pizza");
        assert_eq!(q.date, None);
        assert_eq!(q.title, "fri night pizza");
    }

    #[test]
    fn repeats() {
        let cases = [
            ("x every day", "FREQ=DAILY"),
            ("x every 2 weeks", "FREQ=WEEKLY;INTERVAL=2"),
            ("x every other week", "FREQ=WEEKLY;INTERVAL=2"),
            ("x every weekday", "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR"),
            ("x every month on 1", "FREQ=MONTHLY;BYMONTHDAY=1"),
            ("x every month on the 15th", "FREQ=MONTHLY;BYMONTHDAY=15"),
            ("x every year", "FREQ=YEARLY"),
            ("x every mon,wed", "FREQ=WEEKLY;BYDAY=MO,WE"),
            ("x every tue and thu", "FREQ=WEEKLY;BYDAY=TU,TH"),
        ];
        for (s, want) in cases {
            let x = p(s);
            assert_eq!(x.repeat, Some(Repeat::Rule(want.into())), "{s}");
            assert_eq!(x.title, "x", "{s}");
        }
        // First occurrence when no date is given.
        assert_eq!(p("gym every mon,wed").date, Some(d(10, 19)));
        assert_eq!(p("rent every month on 1").date, Some(d(11, 1)));
        assert_eq!(p("x every custom").repeat, Some(Repeat::Keep));
        assert_eq!(p("every one of them").title, "every one of them");
    }

    #[test]
    fn remind() {
        let x = p("dentist fri 2pm remind 1h");
        assert_eq!(x.remind, Some(Duration::hours(1)));
        assert_eq!(x.title, "dentist");
    }

    #[test]
    fn spans_cover_tokens() {
        let s = "pay rent fri 3pm #work !";
        let x = p(s);
        let roles: Vec<(&str, Role)> = x.spans.iter().map(|sp| (&s[sp.range.clone()], sp.role)).collect();
        assert_eq!(roles, vec![("fri", Role::Date), ("3pm", Role::Time), ("#work", Role::List), ("!", Role::Priority)]);
    }

    #[test]
    fn preview_text() {
        let x = p("pay rent fri 3pm #work !");
        assert_eq!(x.preview(x.kind(), TimeFormat::H24, None), "task · due Fri 16 Oct 15:00 · #work · !");
        let e = p("dentist sat 2-3pm");
        assert_eq!(e.preview(e.kind(), TimeFormat::H12, Some("personal")), "event · Sat 17 Oct 2pm–3pm · #personal");
    }

    #[test]
    fn grammar_dates_read_back() {
        let l = lists();
        let c = ctx(&l);
        for day in [d(10, 15), d(10, 16), d(10, 20), d(12, 31), d(10, 1), d(3, 3)] {
            let text = grammar_date(day, c.today());
            assert_eq!(parse(&format!("x {text}"), &c).date, Some(day), "{text}");
        }
    }

    #[test]
    fn lengths() {
        assert_eq!(parse_length("1h30m"), Some(Duration::minutes(90)));
        assert_eq!(parse_length("1.5h"), Some(Duration::minutes(90)));
        assert_eq!(parse_length("2d"), Some(Duration::days(2)));
        assert_eq!(parse_length("5k"), None);
        assert_eq!(parse_length("h"), None);
    }
}
