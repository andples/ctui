//! Tasks, events and lists, read from and written to iCalendar.
//!
//! An [`Item`] is one `.ics` file. Its typed [`Fields`] are what ctui shows
//! and edits; `cal` is the whole file. On write, only fields that differ from
//! what the file already says are written, so everything else — and the exact
//! form of untouched properties — survives.

use chrono::{DateTime, Duration, NaiveDate, Utc};
use chrono_tz::Tz;

use super::ical::{self, Component, Prop};
use super::time::{self, Moment};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Task,
    Event,
}

impl Kind {
    pub fn component(self) -> &'static str {
        match self {
            Kind::Task => "VTODO",
            Kind::Event => "VEVENT",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Task => "task",
            Kind::Event => "event",
        }
    }
}

/// What ctui understands of a VTODO/VEVENT.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Fields {
    pub title: String,
    /// Event start (DTSTART). Tasks keep their DTSTART untouched.
    pub start: Option<Moment>,
    /// Event end (DTEND, or DTSTART + DURATION when that's what the file has).
    pub end: Option<Moment>,
    /// Task due (DUE).
    pub due: Option<Moment>,
    pub done: bool,
    pub completed: Option<DateTime<Utc>>,
    /// 0 none, 1 `!` low, 2 `!!` medium, 3 `!!!` high.
    pub priority: u8,
    pub rrule: Option<String>,
    pub parent: Option<String>,
    /// Manual sort position (X-CTUI-SORT, else X-APPLE-SORT-ORDER).
    pub sort: Option<i64>,
    /// Display reminders, as offsets before the start/due.
    pub alarms: Vec<Duration>,
}

impl Fields {
    pub fn read(c: &Component) -> Self {
        let kind = if c.name == "VTODO" { Kind::Task } else { Kind::Event };
        let moment = |n: &str| c.get(n).and_then(Moment::from_prop);
        let start = moment("DTSTART");
        let end = moment("DTEND").or_else(|| {
            let d = c.get("DURATION").and_then(|p| time::parse_duration(&p.value))?;
            Some(add(start?, d))
        });
        let status = c.get("STATUS").map(|p| p.value.to_ascii_uppercase());
        let completed = c.get("COMPLETED").and_then(Moment::from_prop).map(|m| m.instant(Tz::UTC).with_timezone(&Utc));
        let done = kind == Kind::Task && (status.as_deref() == Some("COMPLETED") || (status.is_none() && completed.is_some()));
        let parent = c
            .get_all("RELATED-TO")
            .find(|p| p.param("RELTYPE").is_none_or(|t| t.eq_ignore_ascii_case("PARENT")))
            .map(|p| p.value.clone());
        let sort = c
            .get("X-CTUI-SORT")
            .or_else(|| c.get("X-APPLE-SORT-ORDER"))
            .and_then(|p| p.value.trim().parse().ok());
        Self {
            title: c.text("SUMMARY").unwrap_or_default(),
            start: if kind == Kind::Event { start } else { None },
            end: if kind == Kind::Event { end } else { None },
            due: if kind == Kind::Task { moment("DUE") } else { None },
            done,
            completed,
            priority: c.get("PRIORITY").and_then(|p| p.value.trim().parse::<u8>().ok()).map_or(0, from_ical_priority),
            rrule: c.get("RRULE").map(|p| p.value.clone()),
            parent,
            sort,
            alarms: read_alarms(c),
        }
    }

    /// Writes the fields that differ from what `c` already holds.
    pub fn write(&self, c: &mut Component, now: DateTime<Tz>) {
        let base = Fields::read(c);
        if *self == base {
            return;
        }
        if self.title != base.title {
            c.set_text("SUMMARY", &self.title);
        }
        if self.start != base.start {
            match self.start {
                Some(m) => c.set(m.to_prop("DTSTART")),
                None => c.remove("DTSTART"),
            }
        }
        if self.end != base.end || (self.start != base.start && c.get("DURATION").is_some()) {
            c.remove("DURATION");
            match self.end {
                Some(m) => c.set(m.to_prop("DTEND")),
                None => c.remove("DTEND"),
            }
        }
        if self.due != base.due {
            match self.due {
                Some(m) => c.set(m.to_prop("DUE")),
                None => c.remove("DUE"),
            }
        }
        if self.done != base.done || self.completed != base.completed {
            if self.done {
                c.set(Prop::new("STATUS", "COMPLETED"));
                c.set(Prop::new("PERCENT-COMPLETE", "100"));
                let at = self.completed.unwrap_or_else(|| now.with_timezone(&Utc));
                c.set(Prop::new("COMPLETED", at.format("%Y%m%dT%H%M%SZ").to_string()));
            } else {
                c.set(Prop::new("STATUS", "NEEDS-ACTION"));
                c.remove("PERCENT-COMPLETE");
                c.remove("COMPLETED");
            }
        }
        if self.priority != base.priority {
            match self.priority {
                0 => c.remove("PRIORITY"),
                p => c.set(Prop::new("PRIORITY", to_ical_priority(p).to_string())),
            }
        }
        if self.rrule != base.rrule {
            match &self.rrule {
                Some(r) => c.set(Prop::new("RRULE", r.clone())),
                None => {
                    c.remove("RRULE");
                    c.remove("EXDATE");
                }
            }
        }
        if self.parent != base.parent {
            c.props.retain(|p| {
                !(p.name == "RELATED-TO" && p.param("RELTYPE").is_none_or(|t| t.eq_ignore_ascii_case("PARENT")))
            });
            if let Some(uid) = &self.parent {
                c.props.push(Prop::new("RELATED-TO", uid.clone()));
            }
        }
        if self.sort != base.sort {
            match self.sort {
                Some(s) => c.set(Prop::new("X-CTUI-SORT", s.to_string())),
                None => c.remove("X-CTUI-SORT"),
            }
        }
        if self.alarms != base.alarms {
            c.children.retain(|a| !(a.name == "VALARM" && simple_alarm(a).is_some()));
            for off in &self.alarms {
                let mut a = Component::new("VALARM");
                a.props.push(Prop::new("ACTION", "DISPLAY"));
                a.props.push(Prop::new("DESCRIPTION", ical::escape(&self.title)));
                a.props.push(Prop::new("TRIGGER", time::format_duration(-*off)));
                c.children.push(a);
            }
        }
        let stamp = time::utc_stamp(now);
        c.set(Prop::new("LAST-MODIFIED", stamp.clone()));
        c.set(Prop::new("DTSTAMP", stamp));
        if self.start != base.start || self.end != base.end || self.due != base.due || self.rrule != base.rrule {
            if let Some(seq) = c.get("SEQUENCE").and_then(|p| p.value.trim().parse::<u32>().ok()) {
                c.set(Prop::new("SEQUENCE", (seq + 1).to_string()));
            }
        }
    }

    pub fn all_day(&self) -> bool {
        self.start.is_some_and(|s| s.is_date())
    }
}

fn add(m: Moment, d: Duration) -> Moment {
    match m {
        Moment::Date(x) => Moment::Date(x + Duration::days(d.num_days())),
        Moment::Floating(n) => Moment::Floating(n + d),
        Moment::Zoned(z) => Moment::Zoned(z + d),
    }
}

/// iCalendar: 1 highest … 9 lowest, 0 undefined.
fn from_ical_priority(p: u8) -> u8 {
    match p {
        0 => 0,
        1..=4 => 3,
        5 => 2,
        _ => 1,
    }
}

fn to_ical_priority(p: u8) -> u8 {
    match p {
        3 => 1,
        2 => 5,
        _ => 9,
    }
}

/// A display alarm with a relative trigger before the start, as ctui writes them.
fn simple_alarm(a: &Component) -> Option<Duration> {
    let trig = a.get("TRIGGER")?;
    if trig.param("VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE-TIME"))
        || trig.param("RELATED").is_some_and(|v| v.eq_ignore_ascii_case("END"))
    {
        return None;
    }
    let d = time::parse_duration(&trig.value)?;
    (d <= Duration::zero()).then_some(-d)
}

fn read_alarms(c: &Component) -> Vec<Duration> {
    c.children_named("VALARM").filter_map(simple_alarm).collect()
}

/// One `.ics` file: a task or an event, plus any recurrence overrides.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub uid: String,
    pub kind: Kind,
    /// The list (folder) it lives in.
    pub list: String,
    /// File name inside the list folder.
    pub file: String,
    pub f: Fields,
    /// The whole VCALENDAR as last read or written.
    pub cal: Component,
    /// EXDATEs (occurrences removed from a recurring item).
    pub exdates: Vec<Moment>,
    /// Single-occurrence overrides: RECURRENCE-ID and the fields there.
    pub overrides: Vec<(Moment, Fields)>,
}

impl Item {
    /// Reads the first VTODO/VEVENT master in a calendar file.
    pub fn from_calendar(cal: Component, list: &str, file: &str) -> Option<Self> {
        let idx = master_index(&cal)?;
        let master = &cal.children[idx];
        let kind = if master.name == "VTODO" { Kind::Task } else { Kind::Event };
        let uid = master.text("UID").unwrap_or_else(|| file.trim_end_matches(".ics").to_string());
        let f = Fields::read(master);
        let exdates = master
            .get_all("EXDATE")
            .flat_map(|p| {
                p.value.split(',').filter_map(|v| {
                    let one = Prop { value: v.to_string(), ..p.clone() };
                    Moment::from_prop(&one)
                }).collect::<Vec<_>>()
            })
            .collect();
        let overrides = cal
            .children
            .iter()
            .filter(|c| c.name == master.name && c.get("RECURRENCE-ID").is_some())
            .filter_map(|c| Some((Moment::from_prop(c.get("RECURRENCE-ID")?)?, Fields::read(c))))
            .collect();
        Some(Self { uid, kind, list: list.to_string(), file: file.to_string(), f, cal, exdates, overrides })
    }

    /// A new item with a fresh UID, in a fresh VCALENDAR.
    pub fn new(kind: Kind, list: &str, f: Fields, now: DateTime<Tz>) -> Self {
        let uid = format!("{}@ctui", uuid::Uuid::new_v4());
        let mut cal = Component::new("VCALENDAR");
        cal.props.push(Prop::new("VERSION", "2.0"));
        cal.props.push(Prop::new("PRODID", "-//ctui//ctui//EN"));
        let mut c = Component::new(kind.component());
        c.props.push(Prop::new("UID", ical::escape(&uid)));
        let stamp = time::utc_stamp(now);
        c.props.push(Prop::new("DTSTAMP", stamp.clone()));
        c.props.push(Prop::new("CREATED", stamp));
        if kind == Kind::Task {
            c.props.push(Prop::new("STATUS", "NEEDS-ACTION"));
        }
        cal.children.push(c);
        let file = format!("{}.ics", file_stem(&uid));
        let mut item = Self {
            uid,
            kind,
            list: list.to_string(),
            file,
            f: Fields::default(),
            cal,
            exdates: Vec::new(),
            overrides: Vec::new(),
        };
        item.f = f;
        item.sync(now);
        item
    }

    pub fn master(&self) -> &Component {
        &self.cal.children[master_index(&self.cal).expect("item has a master")]
    }

    fn master_mut(&mut self) -> &mut Component {
        let i = master_index(&self.cal).expect("item has a master");
        &mut self.cal.children[i]
    }

    /// Writes `f`, the EXDATEs and the overrides into `cal`.
    pub fn sync(&mut self, now: DateTime<Tz>) {
        let f = self.f.clone();
        f.write(self.master_mut(), now);
        let exdates = self.exdates.clone();
        let read: Vec<Moment> = Item::from_calendar(self.cal.clone(), "", "").map(|i| i.exdates).unwrap_or_default();
        if read != exdates {
            let m = self.master_mut();
            m.remove("EXDATE");
            for e in &exdates {
                m.props.push(e.to_prop("EXDATE"));
            }
        }
        let comp = self.kind.component();
        for (rid, of) in self.overrides.clone() {
            let pos = self.cal.children.iter().position(|c| {
                c.name == comp && c.get("RECURRENCE-ID").and_then(Moment::from_prop) == Some(rid)
            });
            match pos {
                Some(i) => of.write(&mut self.cal.children[i], now),
                None => {
                    let mut c = Component::new(comp);
                    c.props.push(Prop::new("UID", ical::escape(&self.uid)));
                    c.props.push(rid.to_prop("RECURRENCE-ID"));
                    of.write(&mut c, now);
                    self.cal.children.push(c);
                }
            }
        }
        // Drop overrides that were removed.
        let keep: Vec<Moment> = self.overrides.iter().map(|(r, _)| *r).collect();
        self.cal.children.retain(|c| {
            c.name != comp
                || match c.get("RECURRENCE-ID").and_then(Moment::from_prop) {
                    Some(r) => keep.contains(&r),
                    None => true,
                }
        });
    }

    pub fn to_ics(&self) -> String {
        ical::write(&self.cal)
    }

    pub fn is_recurring(&self) -> bool {
        self.f.rrule.is_some()
    }

    /// The date this item is "on": an event's start date, a task's due date.
    pub fn date(&self, tz: Tz) -> Option<NaiveDate> {
        match self.kind {
            Kind::Event => self.f.start.map(|m| m.date(tz)),
            Kind::Task => self.f.due.map(|m| m.date(tz)),
        }
    }
}

fn master_index(cal: &Component) -> Option<usize> {
    let is_item = |c: &Component| c.name == "VTODO" || c.name == "VEVENT";
    cal.children
        .iter()
        .position(|c| is_item(c) && c.get("RECURRENCE-ID").is_none())
        .or_else(|| cal.children.iter().position(is_item))
}

/// A safe file name from a UID.
pub fn file_stem(uid: &str) -> String {
    let s: String = uid
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@') { c } else { '_' })
        .collect();
    if s.is_empty() { "item".into() } else { s }
}

/// A list: one folder of `.ics` files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct List {
    /// Folder name.
    pub id: String,
    /// `displayname` file, else the folder name.
    pub name: String,
    /// `color` file (`#rrggbb`), if any.
    pub color: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Tz> {
        Tz::Europe__London.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap()
    }

    const GOOGLE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Google Inc//Google Calendar 70.9054//EN\r\nBEGIN:VEVENT\r\nDTSTART;TZID=Europe/London:20261015T093000\r\nDURATION:PT30M\r\nRRULE:FREQ=WEEKLY;BYDAY=MO,WE\r\nEXDATE;TZID=Europe/London:20261019T093000\r\nUID:abc@google.com\r\nSUMMARY:Standup\r\nATTENDEE;CN=Jane:mailto:jane@example.com\r\nX-GOOGLE-CONFERENCE:https://meet.google.com/x\r\nSEQUENCE:3\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:abc@google.com\r\nRECURRENCE-ID;TZID=Europe/London:20261021T093000\r\nDTSTART;TZID=Europe/London:20261021T110000\r\nDTEND;TZID=Europe/London:20261021T113000\r\nSUMMARY:Standup (late)\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    fn google() -> Item {
        let cal = ical::parse(GOOGLE).unwrap().remove(0);
        Item::from_calendar(cal, "work", "abc.ics").unwrap()
    }

    #[test]
    fn reads_an_event() {
        let it = google();
        assert_eq!(it.kind, Kind::Event);
        assert_eq!(it.uid, "abc@google.com");
        assert_eq!(it.f.title, "Standup");
        let tz = Tz::Europe__London;
        assert_eq!(it.f.end.unwrap().local(tz).format("%H:%M").to_string(), "10:00");
        assert_eq!(it.exdates.len(), 1);
        assert_eq!(it.overrides.len(), 1);
        assert_eq!(it.overrides[0].1.title, "Standup (late)");
    }

    #[test]
    fn untouched_item_writes_identically() {
        let mut it = google();
        let before = it.to_ics();
        it.sync(now());
        assert_eq!(it.to_ics(), before);
    }

    #[test]
    fn editing_title_keeps_unknown_props() {
        let mut it = google();
        it.f.title = "Daily standup".into();
        it.sync(now());
        let text = it.to_ics();
        assert!(text.contains("SUMMARY:Daily standup"));
        assert!(text.contains("X-GOOGLE-CONFERENCE:https://meet.google.com/x"));
        assert!(text.contains("ATTENDEE;CN=Jane:mailto:jane@example.com"));
        assert!(text.contains("DURATION:PT30M"), "duration kept when times didn't change");
        assert!(text.contains("SEQUENCE:3"));
        assert!(text.contains("Standup (late)"));
    }

    #[test]
    fn moving_bumps_sequence_and_rewrites_end() {
        let mut it = google();
        it.f.start = Some(it.f.start.unwrap().shift_days(1));
        it.f.end = Some(it.f.end.unwrap().shift_days(1));
        it.sync(now());
        let text = it.to_ics();
        assert!(!text.contains("DURATION"));
        assert!(text.contains("DTEND;TZID=Europe/London:20261016T100000"));
        assert!(text.contains("SEQUENCE:4"));
    }

    #[test]
    fn task_round_trip() {
        let f = Fields {
            title: "pay rent, now".into(),
            due: Some(Moment::Date(NaiveDate::from_ymd_opt(2026, 10, 17).unwrap())),
            priority: 3,
            alarms: vec![Duration::minutes(10)],
            ..Fields::default()
        };
        let mut t = Item::new(Kind::Task, "personal", f.clone(), now());
        assert!(t.file.ends_with("@ctui.ics"));
        let back = Item::from_calendar(ical::parse(&t.to_ics()).unwrap().remove(0), "personal", &t.file).unwrap();
        assert_eq!(back.f, f);
        t.f.done = true;
        t.sync(now());
        let done = Item::from_calendar(ical::parse(&t.to_ics()).unwrap().remove(0), "personal", &t.file).unwrap();
        assert!(done.f.done);
        assert!(done.f.completed.is_some());
    }

    #[test]
    fn override_edit_and_removal() {
        let mut it = google();
        it.overrides[0].1.title = "moved".into();
        it.sync(now());
        assert!(it.to_ics().contains("SUMMARY:moved"));
        it.overrides.clear();
        it.sync(now());
        assert!(!it.to_ics().contains("RECURRENCE-ID"));
    }
}
