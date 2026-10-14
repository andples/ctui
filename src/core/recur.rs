//! Recurrence: expanding RRULEs into occurrences and advancing repeating tasks.

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use chrono_tz::Tz;
use rrule::{RRule, Unvalidated};

use super::model::{Fields, Item, Kind};
use super::time::{self, Moment};

/// One occurrence of an item (a non-recurring item has exactly one).
#[derive(Debug, Clone, PartialEq)]
pub struct Occurrence {
    /// For recurring items, the original start (the RECURRENCE-ID).
    pub rid: Option<Moment>,
    pub start: Moment,
    pub end: Option<Moment>,
    /// The fields to show (an override's, if this occurrence was edited).
    pub f: Fields,
}

/// Generous cap on expansion so a bad rule can't hang the UI.
const LIMIT: u16 = 2000;

fn rrule_tz(tz: Tz) -> rrule::Tz {
    rrule::Tz::Tz(tz)
}

/// Rewrites UNTIL so the rrule crate accepts it next to this DTSTART: a
/// date-only UNTIL becomes the end of that day, a zone-less one becomes UTC.
fn normalize_until(rule: &str, start: &Moment, tz: Tz) -> String {
    rule.split(';')
        .map(|part| {
            let Some(v) = part.strip_prefix("UNTIL=").or_else(|| part.strip_prefix("until=")) else {
                return part.to_string();
            };
            let naive = if v.len() == 8 {
                NaiveDate::parse_from_str(v, "%Y%m%d").ok().map(|d| d.and_hms_opt(23, 59, 59).expect("valid"))
            } else if !v.ends_with(['Z', 'z']) {
                NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()
            } else {
                return part.to_string();
            };
            let zone = match start {
                Moment::Zoned(z) => z.timezone(),
                _ => tz,
            };
            match naive {
                Some(n) => format!("UNTIL={}", time::resolve(zone, n).naive_utc().format("%Y%m%dT%H%M%SZ")),
                None => part.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// The zone the rule is expanded in, and DTSTART in it.
fn anchor(start: &Moment, tz: Tz) -> DateTime<rrule::Tz> {
    let (zone, naive) = match start {
        Moment::Zoned(z) => (z.timezone(), z.naive_local()),
        other => (tz, other.local(tz)),
    };
    let local = time::resolve(zone, naive);
    rrule_tz(zone).from_utc_datetime(&local.naive_utc())
}

fn back_to_moment(template: &Moment, dt: DateTime<rrule::Tz>) -> Moment {
    match template {
        Moment::Date(_) => Moment::Date(dt.naive_local().date()),
        Moment::Floating(_) => Moment::Floating(dt.naive_local()),
        Moment::Zoned(z) => Moment::Zoned(dt.with_timezone(&z.timezone())),
    }
}

/// Start instants of `rule` from `start`, within `[after, before]`.
fn expand(rule: &str, start: &Moment, exdates: &[Moment], after: DateTime<Tz>, before: DateTime<Tz>, tz: Tz) -> Option<Vec<Moment>> {
    let rule = normalize_until(rule, start, tz);
    let parsed: RRule<Unvalidated> = rule.parse().ok()?;
    let dtstart = anchor(start, tz);
    let mut set = parsed.build(dtstart).ok()?;
    for ex in exdates {
        let at = match ex {
            Moment::Date(d) => anchor(&Moment::at(tz, *d, start.time(tz)), tz),
            other => anchor(other, tz),
        };
        set = set.exdate(at);
    }
    let set = set
        .after(rrule::Tz::UTC.from_utc_datetime(&after.naive_utc()))
        .before(rrule::Tz::UTC.from_utc_datetime(&before.naive_utc()));
    let res = set.all(LIMIT);
    Some(res.dates.into_iter().map(|d| back_to_moment(start, d)).collect())
}

fn same_moment(a: &Moment, b: &Moment, tz: Tz) -> bool {
    match (a, b) {
        (Moment::Date(x), m) | (m, Moment::Date(x)) => m.date(tz) == *x,
        _ => a.instant(tz) == b.instant(tz),
    }
}

fn day_start(d: NaiveDate, tz: Tz) -> DateTime<Tz> {
    time::resolve(tz, d.and_time(NaiveTime::MIN))
}

/// Event occurrences that touch the days `[from, to)`.
pub fn occurrences(item: &Item, from: NaiveDate, to: NaiveDate, tz: Tz) -> Vec<Occurrence> {
    if item.kind != Kind::Event {
        return Vec::new();
    }
    let Some(start) = item.f.start else { return Vec::new() };
    let len = item.f.end.map(|e| e.instant(tz) - start.instant(tz)).unwrap_or_else(Duration::zero);
    let range_start = day_start(from, tz);
    let range_end = day_start(to, tz);
    let touches = |s: &Moment, e: &Option<Moment>| {
        let s_at = s.instant(tz);
        let e_at = e.map(|e| e.instant(tz)).unwrap_or(s_at);
        // An all-day event's DTEND is exclusive; a zero-length one still shows.
        s_at < range_end && (e_at > range_start || (e_at == s_at && s_at >= range_start))
    };
    let Some(rule) = &item.f.rrule else {
        return if touches(&start, &item.f.end) {
            vec![Occurrence { rid: None, start, end: item.f.end, f: item.f.clone() }]
        } else {
            Vec::new()
        };
    };
    let starts = expand(rule, &start, &item.exdates, range_start - len - Duration::seconds(1), range_end, tz)
        .unwrap_or_else(|| vec![start]);
    let mut out = Vec::new();
    let mut used = vec![false; item.overrides.len()];
    for s in starts {
        let ov = item.overrides.iter().position(|(rid, _)| same_moment(rid, &s, tz));
        let occ = match ov {
            Some(i) => {
                used[i] = true;
                let of = &item.overrides[i].1;
                let os = of.start.unwrap_or(s);
                let oe = of.end.or_else(|| Some(add(&os, len)));
                Occurrence { rid: Some(s), start: os, end: oe, f: of.clone() }
            }
            None => Occurrence { rid: Some(s), start: s, end: item.f.end.map(|_| add(&s, len)), f: item.f.clone() },
        };
        if touches(&occ.start, &occ.end) {
            out.push(occ);
        }
    }
    // Overrides moved into the range from an occurrence outside it.
    for (i, (rid, of)) in item.overrides.iter().enumerate() {
        if used[i] {
            continue;
        }
        let os = of.start.unwrap_or(*rid);
        let oe = of.end.or_else(|| Some(add(&os, len)));
        if touches(&os, &oe) && !item.exdates.iter().any(|e| same_moment(e, rid, tz)) {
            out.push(Occurrence { rid: Some(*rid), start: os, end: oe, f: of.clone() });
        }
    }
    out.sort_by_key(|o| o.start.instant(tz));
    out
}

fn add(m: &Moment, d: Duration) -> Moment {
    match m {
        Moment::Date(x) => Moment::Date(*x + Duration::days(d.num_days())),
        Moment::Floating(n) => Moment::Floating(*n + d),
        Moment::Zoned(z) => Moment::Zoned(*z + d),
    }
}

/// The next due of a repeating task after it's ticked off: the first
/// occurrence after both its current due and now, so an overdue daily task
/// lands on tomorrow rather than on another past day.
pub fn next_due(rule: &str, due: &Moment, now: DateTime<Tz>, tz: Tz) -> Option<Moment> {
    let floor = match due {
        Moment::Date(d) => {
            let base = (*d).max(now.date_naive());
            day_start(base + Duration::days(1), tz) - Duration::seconds(1)
        }
        other => other.instant(tz).max(now),
    };
    let found = expand(rule, due, &[], floor + Duration::seconds(1), floor + Duration::days(3660), tz)?;
    found.into_iter().find(|m| m.instant(tz) > floor)
}

/// A short human description of a rule ctui can type (`every 2 weeks`), or
/// `None` for anything the grammar can't say.
pub fn describe(rule: &str) -> Option<String> {
    let mut freq = None;
    let mut interval = 1u32;
    let mut byday = None;
    let mut bymonthday = None;
    for part in rule.split(';') {
        let (k, v) = part.split_once('=')?;
        match k.to_ascii_uppercase().as_str() {
            "FREQ" => freq = Some(v.to_ascii_uppercase()),
            "INTERVAL" => interval = v.parse().ok()?,
            "BYDAY" => byday = Some(v.to_ascii_uppercase()),
            "BYMONTHDAY" => bymonthday = Some(v.to_string()),
            "WKST" => {}
            _ => return None,
        }
    }
    let unit = |one: &str, many: &str| {
        if interval == 1 { format!("every {one}") } else { format!("every {interval} {many}") }
    };
    Some(match (freq?.as_str(), byday.as_deref(), bymonthday.as_deref()) {
        ("DAILY", None, None) => unit("day", "days"),
        ("WEEKLY", Some("MO,TU,WE,TH,FR"), None) if interval == 1 => "every weekday".into(),
        ("WEEKLY", None, None) => unit("week", "weeks"),
        ("WEEKLY", Some(days), None) if interval == 1 => {
            let names: Option<Vec<&str>> = days.split(',').map(day_word).collect();
            format!("every {}", names?.join(","))
        }
        ("MONTHLY", None, None) => unit("month", "months"),
        ("MONTHLY", None, Some(d)) if interval == 1 => format!("every month on {d}"),
        ("YEARLY", None, None) => unit("year", "years"),
        _ => return None,
    })
}

fn day_word(code: &str) -> Option<&'static str> {
    Some(match code {
        "MO" => "mon",
        "TU" => "tue",
        "WE" => "wed",
        "TH" => "thu",
        "FR" => "fri",
        "SA" => "sat",
        "SU" => "sun",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ical;

    const TZ: Tz = Tz::Europe__London;

    const GOOGLE: &str = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nDTSTART;TZID=Europe/London:20261015T093000\r\nDURATION:PT30M\r\nRRULE:FREQ=WEEKLY;BYDAY=MO,WE\r\nEXDATE;TZID=Europe/London:20261019T093000\r\nUID:abc\r\nSUMMARY:Standup\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:abc\r\nRECURRENCE-ID;TZID=Europe/London:20261021T093000\r\nDTSTART;TZID=Europe/London:20261021T110000\r\nDTEND;TZID=Europe/London:20261021T113000\r\nSUMMARY:Standup (late)\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    fn d(m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, m, day).unwrap()
    }

    fn item(text: &str) -> Item {
        Item::from_calendar(ical::parse(text).unwrap().remove(0), "l", "f.ics").unwrap()
    }

    fn ev(body: &str) -> Item {
        item(&format!("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:x\r\nSUMMARY:s\r\n{body}END:VEVENT\r\nEND:VCALENDAR\r\n"))
    }

    #[test]
    fn single_event_in_and_out_of_range() {
        let e = ev("DTSTART;TZID=Europe/London:20261015T090000\r\nDTEND;TZID=Europe/London:20261015T100000\r\n");
        assert_eq!(occurrences(&e, d(10, 15), d(10, 16), TZ).len(), 1);
        assert!(occurrences(&e, d(10, 16), d(10, 17), TZ).is_empty());
    }

    #[test]
    fn all_day_end_is_exclusive() {
        let e = ev("DTSTART;VALUE=DATE:20261020\r\nDTEND;VALUE=DATE:20261023\r\n");
        assert_eq!(occurrences(&e, d(10, 22), d(10, 23), TZ).len(), 1);
        assert!(occurrences(&e, d(10, 23), d(10, 24), TZ).is_empty());
    }

    #[test]
    fn weekly_with_exdate_and_override() {
        let e = item(GOOGLE);
        let occ = occurrences(&e, d(10, 12), d(10, 26), TZ);
        let days: Vec<_> = occ.iter().map(|o| o.start.date(TZ)).collect();
        // Mon/Wed from Thu 15th: 19 is excluded, 21 is overridden to 11:00.
        assert_eq!(days, vec![d(10, 21)]);
        assert_eq!(occ[0].f.title, "Standup (late)");
        assert_eq!(occ[0].start.time(TZ).unwrap().to_string(), "11:00:00");
    }

    #[test]
    fn dst_keeps_wall_clock() {
        let e = ev("DTSTART;TZID=Europe/London:20261020T090000\r\nRRULE:FREQ=WEEKLY\r\n");
        let occ = occurrences(&e, d(10, 26), d(11, 4), TZ);
        assert_eq!(occ.len(), 2);
        assert!(occ.iter().all(|o| o.start.time(TZ).unwrap().to_string() == "09:00:00"));
    }

    #[test]
    fn date_until_is_accepted() {
        let e = ev("DTSTART;VALUE=DATE:20261001\r\nRRULE:FREQ=DAILY;UNTIL=20261003\r\n");
        assert_eq!(occurrences(&e, d(10, 1), d(10, 31), TZ).len(), 3);
    }

    #[test]
    fn task_advances_past_now() {
        let now = TZ.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap();
        let due = Moment::Date(d(10, 10));
        assert_eq!(next_due("FREQ=DAILY", &due, now, TZ), Some(Moment::Date(d(10, 16))));
        let rent = Moment::Date(d(10, 1));
        assert_eq!(next_due("FREQ=MONTHLY", &rent, now, TZ), Some(Moment::Date(d(11, 1))));
        let timed = Moment::at(TZ, d(10, 15), NaiveTime::from_hms_opt(17, 0, 0));
        assert_eq!(next_due("FREQ=DAILY", &timed, now, TZ).unwrap().date(TZ), d(10, 16));
    }

    #[test]
    fn descriptions() {
        assert_eq!(describe("FREQ=WEEKLY;INTERVAL=2").as_deref(), Some("every 2 weeks"));
        assert_eq!(describe("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR").as_deref(), Some("every weekday"));
        assert_eq!(describe("FREQ=WEEKLY;BYDAY=MO,WE").as_deref(), Some("every mon,wed"));
        assert_eq!(describe("FREQ=MONTHLY;BYMONTHDAY=1").as_deref(), Some("every month on 1"));
        assert_eq!(describe("FREQ=MONTHLY;BYSETPOS=-1;BYDAY=FR"), None);
    }
}
