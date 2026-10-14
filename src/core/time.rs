//! Dates and times as iCalendar stores them, and the conversions ctui needs.

use chrono::{
    DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc,
    Weekday,
};
use chrono_tz::Tz;

use super::ical::Prop;

/// A DTSTART/DTEND/DUE/… value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moment {
    /// `VALUE=DATE`: an all-day date.
    Date(NaiveDate),
    /// No zone: means the same wall-clock time everywhere (shown as local).
    Floating(NaiveDateTime),
    Zoned(DateTime<Tz>),
}

impl Moment {
    /// Reads a date or date-time property. Unknown `TZID`s (Windows zone names
    /// from Outlook, say) are read as floating local time.
    pub fn from_prop(p: &Prop) -> Option<Self> {
        let v = p.value.trim();
        let is_date = p
            .param("VALUE")
            .is_some_and(|x| x.eq_ignore_ascii_case("DATE"))
            || (v.len() == 8 && !v.contains('T'));
        if is_date {
            return NaiveDate::parse_from_str(&v[..8.min(v.len())], "%Y%m%d")
                .ok()
                .map(Moment::Date);
        }
        let (body, utc) = match v.strip_suffix(['Z', 'z']) {
            Some(b) => (b, true),
            None => (v, false),
        };
        let naive = NaiveDateTime::parse_from_str(body, "%Y%m%dT%H%M%S")
            .or_else(|_| NaiveDateTime::parse_from_str(body, "%Y%m%dT%H%M"))
            .ok()?;
        if utc {
            return Some(Moment::Zoned(Tz::UTC.from_utc_datetime(&naive)));
        }
        match p.param("TZID").and_then(|t| {
            t.trim_matches('"')
                .trim_start_matches('/')
                .parse::<Tz>()
                .ok()
        }) {
            Some(tz) => Some(Moment::Zoned(resolve(tz, naive))),
            None => Some(Moment::Floating(naive)),
        }
    }

    pub fn to_prop(&self, name: &str) -> Prop {
        match self {
            Moment::Date(d) => {
                Prop::new(name, d.format("%Y%m%d").to_string()).with_param("VALUE", "DATE")
            }
            Moment::Floating(n) => Prop::new(name, n.format("%Y%m%dT%H%M%S").to_string()),
            Moment::Zoned(dt) if dt.timezone() == Tz::UTC => {
                Prop::new(name, dt.naive_utc().format("%Y%m%dT%H%M%SZ").to_string())
            }
            Moment::Zoned(dt) => {
                Prop::new(name, dt.naive_local().format("%Y%m%dT%H%M%S").to_string())
                    .with_param("TZID", dt.timezone().name())
            }
        }
    }

    pub fn is_date(&self) -> bool {
        matches!(self, Moment::Date(_))
    }

    /// Wall-clock time in `tz` (a date is its midnight).
    pub fn local(&self, tz: Tz) -> NaiveDateTime {
        match self {
            Moment::Date(d) => d.and_time(NaiveTime::MIN),
            Moment::Floating(n) => *n,
            Moment::Zoned(dt) => dt.with_timezone(&tz).naive_local(),
        }
    }

    pub fn date(&self, tz: Tz) -> NaiveDate {
        self.local(tz).date()
    }

    /// The time of day, or `None` for an all-day date.
    pub fn time(&self, tz: Tz) -> Option<NaiveTime> {
        (!self.is_date()).then(|| self.local(tz).time())
    }

    /// The instant, reading dates and floating times in `tz`.
    pub fn instant(&self, tz: Tz) -> DateTime<Tz> {
        match self {
            Moment::Zoned(dt) => dt.with_timezone(&tz),
            other => resolve(tz, other.local(tz)),
        }
    }

    /// A moment in `tz` from a date and an optional time.
    pub fn at(tz: Tz, date: NaiveDate, time: Option<NaiveTime>) -> Self {
        match time {
            None => Moment::Date(date),
            Some(t) => Moment::Zoned(resolve(tz, date.and_time(t))),
        }
    }

    /// The same kind of moment moved by `days`, keeping the wall-clock time.
    pub fn shift_days(&self, days: i64) -> Self {
        let d = Duration::days(days);
        match self {
            Moment::Date(x) => Moment::Date(*x + d),
            Moment::Floating(n) => Moment::Floating(*n + d),
            Moment::Zoned(dt) => {
                let zone = dt.timezone();
                Moment::Zoned(resolve(zone, dt.naive_local() + d))
            }
        }
    }

    /// Same kind and zone, but a different local date/time.
    pub fn with_local(&self, date: NaiveDate, time: Option<NaiveTime>, tz: Tz) -> Self {
        match (self, time) {
            (_, None) => Moment::Date(date),
            (Moment::Floating(_), Some(t)) => Moment::Floating(date.and_time(t)),
            (Moment::Zoned(dt), Some(t)) if dt.timezone() != Tz::UTC => {
                Moment::Zoned(resolve(dt.timezone(), date.and_time(t)))
            }
            (_, Some(t)) => Moment::Zoned(resolve(tz, date.and_time(t))),
        }
    }
}

/// A local time in `tz`, taking the earlier reading in a DST overlap and
/// moving forward out of a DST gap.
pub fn resolve(tz: Tz, naive: NaiveDateTime) -> DateTime<Tz> {
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(dt) => dt,
        LocalResult::Ambiguous(a, _) => a,
        LocalResult::None => {
            let mut probe = naive;
            for _ in 0..8 {
                probe += Duration::minutes(30);
                if let LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) =
                    tz.from_local_datetime(&probe)
                {
                    return dt;
                }
            }
            Tz::UTC.from_utc_datetime(&naive).with_timezone(&tz)
        }
    }
}

/// The machine's zone (`TZ`, then the system setting), UTC if unknown.
pub fn local_zone() -> Tz {
    if let Ok(tz) = std::env::var("TZ")
        && let Ok(z) = tz.trim_start_matches(':').parse::<Tz>()
    {
        return z;
    }
    iana_time_zone::get_timezone()
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(Tz::UTC)
}

pub fn now_in(tz: Tz) -> DateTime<Tz> {
    Utc::now().with_timezone(&tz)
}

/// Parses an iCalendar DURATION (`PT10M`, `-P1D`, `P1DT2H30M`, `P2W`).
pub fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    let (neg, s) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    let s = s.strip_prefix(['P', 'p'])?;
    let mut total = Duration::zero();
    let mut num = String::new();
    let mut in_time = false;
    for c in s.chars() {
        match c.to_ascii_uppercase() {
            'T' => in_time = true,
            d if d.is_ascii_digit() => num.push(d),
            unit => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                total += match (unit, in_time) {
                    ('W', _) => Duration::weeks(n),
                    ('D', _) => Duration::days(n),
                    ('H', true) => Duration::hours(n),
                    ('M', true) => Duration::minutes(n),
                    ('S', true) => Duration::seconds(n),
                    _ => return None,
                };
            }
        }
    }
    if !num.is_empty() {
        return None;
    }
    Some(if neg { -total } else { total })
}

pub fn format_duration(d: Duration) -> String {
    let neg = d < Duration::zero();
    let mut secs = d.num_seconds().abs();
    let mut out = String::from(if neg { "-P" } else { "P" });
    let days = secs / 86_400;
    secs %= 86_400;
    if days > 0 {
        out.push_str(&format!("{days}D"));
    }
    if secs > 0 || days == 0 {
        out.push('T');
        let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
        if h > 0 {
            out.push_str(&format!("{h}H"));
        }
        if m > 0 || (h == 0 && s == 0) {
            out.push_str(&format!("{m}M"));
        }
        if s > 0 {
            out.push_str(&format!("{s}S"));
        }
    }
    out
}

/// The first day of the week containing `d`.
pub fn week_start_of(d: NaiveDate, start: Weekday) -> NaiveDate {
    let back = (7 + d.weekday().num_days_from_monday() - start.num_days_from_monday()) % 7;
    d - Duration::days(back as i64)
}

pub fn last_of_month(d: NaiveDate) -> NaiveDate {
    let (y, m) = if d.month() == 12 {
        (d.year() + 1, 1)
    } else {
        (d.year(), d.month() + 1)
    };
    NaiveDate::from_ymd_opt(y, m, 1).expect("valid month") - Duration::days(1)
}

/// `d` plus `months`, clamped to the end of a shorter month.
pub fn add_months(d: NaiveDate, months: i32) -> NaiveDate {
    let total = d.year() * 12 + d.month0() as i32 + months;
    let (y, m) = (total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
    let first = NaiveDate::from_ymd_opt(y, m, 1).expect("valid month");
    let last = last_of_month(first).day();
    NaiveDate::from_ymd_opt(y, m, d.day().min(last)).expect("clamped day")
}

pub fn utc_stamp(now: DateTime<Tz>) -> String {
    now.naive_utc().format("%Y%m%dT%H%M%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prop(line: &str) -> Prop {
        super::super::ical::parse(&format!("BEGIN:X\r\n{line}\r\nEND:X\r\n")).unwrap()[0].props[0]
            .clone()
    }

    #[test]
    fn reads_all_forms() {
        assert_eq!(
            Moment::from_prop(&prop("DTSTART;VALUE=DATE:20261020")),
            Some(Moment::Date(NaiveDate::from_ymd_opt(2026, 10, 20).unwrap()))
        );
        let z = Moment::from_prop(&prop("DTSTART:20261020T120000Z")).unwrap();
        assert_eq!(
            z.local(Tz::Europe__London).time(),
            NaiveTime::from_hms_opt(13, 0, 0).unwrap()
        );
        let t = Moment::from_prop(&prop("DTSTART;TZID=America/New_York:20261020T090000")).unwrap();
        assert_eq!(
            t.local(Tz::UTC).time(),
            NaiveTime::from_hms_opt(13, 0, 0).unwrap()
        );
        let w =
            Moment::from_prop(&prop("DTSTART;TZID=Pacific Standard Time:20261020T090000")).unwrap();
        assert!(matches!(w, Moment::Floating(_)));
    }

    #[test]
    fn writes_back_the_same() {
        for line in [
            "DTSTART;VALUE=DATE:20261020",
            "DTSTART:20261020T120000Z",
            "DTSTART;TZID=America/New_York:20261020T090000",
            "DTSTART:20261020T090000",
        ] {
            let p = prop(line);
            let m = Moment::from_prop(&p).unwrap();
            assert_eq!(Moment::from_prop(&m.to_prop("DTSTART")), Some(m), "{line}");
        }
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("PT10M"), Some(Duration::minutes(10)));
        assert_eq!(parse_duration("-P1DT2H"), Some(-Duration::hours(26)));
        assert_eq!(parse_duration("P2W"), Some(Duration::weeks(2)));
        assert_eq!(parse_duration("P1X"), None);
        for d in [
            Duration::minutes(10),
            -Duration::minutes(90),
            Duration::days(1),
            Duration::zero(),
        ] {
            assert_eq!(parse_duration(&format_duration(d)), Some(d));
        }
    }

    #[test]
    fn dst_gap_moves_forward() {
        let tz = Tz::Europe__London;
        let gap = NaiveDate::from_ymd_opt(2026, 3, 29)
            .unwrap()
            .and_hms_opt(1, 30, 0)
            .unwrap();
        assert_eq!(
            resolve(tz, gap).naive_local().time(),
            NaiveTime::from_hms_opt(2, 0, 0).unwrap()
        );
    }

    #[test]
    fn month_math() {
        let jan31 = NaiveDate::from_ymd_opt(2026, 1, 31).unwrap();
        assert_eq!(
            add_months(jan31, 1),
            NaiveDate::from_ymd_opt(2026, 2, 28).unwrap()
        );
        assert_eq!(
            add_months(jan31, -2),
            NaiveDate::from_ymd_opt(2025, 11, 30).unwrap()
        );
        let feb = NaiveDate::from_ymd_opt(2028, 2, 10).unwrap();
        assert_eq!(last_of_month(feb).day(), 29);
        let wed = NaiveDate::from_ymd_opt(2026, 10, 14).unwrap();
        assert_eq!(
            week_start_of(wed, Weekday::Mon),
            NaiveDate::from_ymd_opt(2026, 10, 12).unwrap()
        );
        assert_eq!(
            week_start_of(wed, Weekday::Sun),
            NaiveDate::from_ymd_opt(2026, 10, 11).unwrap()
        );
    }
}
