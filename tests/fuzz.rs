//! Throws random and mangled input at everything that reads untrusted data:
//! .ics files (imports, synced calendars), quick-add text, and MCP requests.
//! Nothing may panic. CTUI_FUZZ=<n> runs n rounds (default 3000).

use chrono::{NaiveDate, TimeZone};
use chrono_tz::Tz;
use ctui::core::{ical, model::Item, parse, recur};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const SEEDS: &[&str] = &[
    "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\nDTSTART;TZID=Europe/London:20261015T093000\r\nDTEND;TZID=Europe/London:20261015T100000\r\nRRULE:FREQ=WEEKLY;BYDAY=MO,WE;UNTIL=20270101\r\nEXDATE;TZID=Europe/London:20261019T093000\r\nSUMMARY:Stand\\, up\r\nBEGIN:VALARM\r\nTRIGGER:-PT10M\r\nEND:VALARM\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:a\r\nRECURRENCE-ID;TZID=Europe/London:20261021T093000\r\nDTSTART;TZID=Europe/London:20261021T110000\r\nSUMMARY:moved\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nUID:t\r\nDUE;VALUE=DATE:20261020\r\nSTATUS:COMPLETED\r\nCOMPLETED:20261001T100000Z\r\nPRIORITY:1\r\nRELATED-TO:p\r\nRRULE:FREQ=DAILY;COUNT=3\r\nSUMMARY:t\r\nEND:VTODO\r\nEND:VCALENDAR\r\n",
    "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:d\r\nDTSTART;VALUE=DATE:20261020\r\nDURATION:P3D\r\nRRULE:FREQ=MONTHLY;BYSETPOS=-1;BYDAY=FR\r\nSUMMARY:d\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
];

const PIECES: &[&str] = &[
    "BEGIN:VEVENT\r\n",
    "END:VEVENT\r\n",
    "BEGIN:VTODO\r\n",
    "END:VTODO\r\n",
    "DTSTART:",
    "DTSTART;VALUE=DATE:",
    "DUE:",
    "DTEND;TZID=Nowhere/Land:",
    "RRULE:FREQ=",
    "RRULE:FREQ=SECONDLY;INTERVAL=1",
    "COUNT=999999999",
    "UNTIL=1",
    "EXDATE:",
    "RECURRENCE-ID:",
    "TRIGGER:-P",
    "DURATION:P99999999W",
    "20261015T",
    "20261015",
    "é",
    "🙂",
    "\u{1b}[31m",
    ";",
    ":",
    ",",
    "\\",
    "\"",
    "\r\n",
    "\r\n ",
    "=",
    "99999999T999999",
    "BYDAY=",
    "MO,TU",
    "X-",
    "PRIORITY:999",
];

fn mutate(r: &mut Rng, base: &str) -> String {
    let mut s: Vec<char> = base.chars().collect();
    for _ in 0..1 + r.below(8) {
        match r.below(4) {
            0 if !s.is_empty() => {
                let i = r.below(s.len());
                s.remove(i);
            }
            1 => {
                let i = r.below(s.len() + 1);
                let p = PIECES[r.below(PIECES.len())];
                for (k, c) in p.chars().enumerate() {
                    s.insert(i + k, c);
                }
            }
            2 if !s.is_empty() => {
                let i = r.below(s.len());
                s[i] = char::from_u32(r.below(0x2000) as u32).unwrap_or('x');
            }
            _ if s.len() > 2 => {
                let a = r.below(s.len());
                let b = r.below(s.len());
                s.swap(a, b);
            }
            _ => {}
        }
    }
    s.into_iter().collect()
}

fn rounds() -> usize {
    std::env::var("CTUI_FUZZ")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000)
}

#[test]
fn ics_never_panics() {
    let tz = Tz::Europe__London;
    let mut r = Rng(0x9e3779b97f4a7c15);
    for _ in 0..rounds() {
        let seed = SEEDS[r.below(SEEDS.len())];
        let text = mutate(&mut r, seed);
        let Ok(cals) = ical::parse(&text) else {
            continue;
        };
        for cal in cals {
            let _ = ical::write(&cal);
            if let Some(mut item) = Item::from_calendar(cal, "l", "f.ics") {
                let from = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
                let _ = recur::occurrences(&item, from, from + chrono::Duration::days(400), tz);
                if let (Some(rule), Some(due)) = (item.f.rrule.clone(), item.f.due) {
                    let _ = recur::next_due(
                        &rule,
                        &due,
                        tz.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap(),
                        tz,
                    );
                }
                item.f.title.push('x');
                item.sync(tz.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap());
                let _ = item.to_ics();
            }
        }
    }
}

#[test]
fn deep_nesting_is_refused_not_a_crash() {
    let text = "BEGIN:VCALENDAR\r\n".repeat(200_000);
    assert!(ical::parse(&text).is_err());
    let closed = format!(
        "{}{}",
        "BEGIN:X\r\n".repeat(200_000),
        "END:X\r\n".repeat(200_000)
    );
    assert!(ical::parse(&closed).is_err());
}

#[test]
fn grammar_never_panics() {
    let words = [
        "today",
        "tom",
        "fri",
        "next",
        "oct",
        "20",
        "20/10",
        "2026-02-30",
        "+3d",
        "+99999999y",
        "3pm",
        "15:00",
        "9.30",
        "3-4pm",
        "11-1pm",
        "for",
        "45m",
        "1.5h",
        "allday",
        "#",
        "#w",
        "@",
        "!",
        "!!!!",
        "every",
        "2",
        "weeks",
        "month",
        "on",
        "the",
        "31st",
        "mon,",
        "and",
        "remind",
        "in",
        "99999",
        "days",
        "to",
        "\"",
        "é",
        "🙂",
        "-",
        "–",
        "eom",
        "12am",
        "99:99",
        "25-26pm",
        "0/0",
        "1/2/3/4",
        "+",
        "+m",
    ];
    let lists = vec![ctui::core::model::List {
        id: "work".into(),
        name: "Work".into(),
        color: None,
    }];
    let ctx = parse::Ctx {
        now: Tz::Europe__London
            .with_ymd_and_hms(2026, 10, 15, 12, 0, 0)
            .unwrap(),
        date_order: ctui::config::DateOrder::Dmy,
        week_start: chrono::Weekday::Mon,
        lists: &lists,
    };
    let mut r = Rng(42);
    for _ in 0..rounds() * 3 {
        let n = 1 + r.below(8);
        let text: Vec<&str> = (0..n).map(|_| words[r.below(words.len())]).collect();
        let joined = if r.below(3) == 0 {
            text.concat()
        } else {
            text.join(" ")
        };
        let d = parse::parse(&joined, &ctx);
        let _ = d.preview(d.kind(), ctui::config::TimeFormat::H12, None);
        let _ = parse::parse(&mutate(&mut r, &joined), &ctx);
    }
}
