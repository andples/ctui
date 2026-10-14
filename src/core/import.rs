//! One-way import (`.ics`, folders of them, Google Takeout `.zip`s and
//! Takeout's `Tasks.json`) and export to a single `.ics`.
//!
//! Imports are idempotent by UID. The state dir remembers what each imported
//! item looked like, so re-importing a newer export updates items that are
//! unchanged here and skips ones that were edited or deleted here.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, NaiveDate};
use serde::{Deserialize, Serialize};

use super::ical::{self, Component, Prop};
use super::model::{Item, file_stem};
use super::store::{Change, Store, normalize_color, slug};
use super::undo::Journal;

/// One calendar read from somewhere.
#[derive(Debug, Clone)]
pub struct Source {
    pub name: String,
    pub color: Option<String>,
    pub cal: Component,
}

/// FNV-1a: stable across builds, unlike `DefaultHasher`.
fn hash(text: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn stem_name(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "calendar".into())
}

fn sources_from_ics(text: &str, fallback_name: &str) -> Result<Vec<Source>> {
    let cals = ical::parse(text)?;
    Ok(cals
        .into_iter()
        .filter(|c| c.name == "VCALENDAR")
        .map(|cal| Source {
            name: cal
                .text("X-WR-CALNAME")
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| fallback_name.to_string()),
            color: cal
                .get("X-APPLE-CALENDAR-COLOR")
                .or_else(|| cal.get("COLOR"))
                .and_then(|p| normalize_color(&p.value)),
            cal,
        })
        .collect())
}

/// Google Takeout's `Tasks.json`: task lists of tasks.
fn sources_from_tasks_json(text: &str) -> Result<Vec<Source>> {
    let v: serde_json::Value = serde_json::from_str(text).context("reading Tasks.json")?;
    let lists = v
        .get("items")
        .and_then(|x| x.as_array())
        .context("Tasks.json has no task lists")?;
    let mut out = Vec::new();
    for list in lists {
        let name = list
            .get("title")
            .and_then(|x| x.as_str())
            .unwrap_or("Tasks")
            .to_string();
        let mut cal = Component::new("VCALENDAR");
        cal.props.push(Prop::new("VERSION", "2.0"));
        cal.props
            .push(Prop::new("PRODID", "-//ctui//Google Tasks import//EN"));
        for t in list
            .get("items")
            .and_then(|x| x.as_array())
            .into_iter()
            .flatten()
        {
            let s = |k: &str| t.get(k).and_then(|x| x.as_str());
            let Some(id) = s("id") else { continue };
            let mut c = Component::new("VTODO");
            c.props.push(Prop::new(
                "UID",
                ical::escape(&format!("{id}@google-tasks")),
            ));
            c.props
                .push(Prop::new("SUMMARY", ical::escape(s("title").unwrap_or(""))));
            if let Some(n) = s("notes").filter(|n| !n.is_empty()) {
                c.props.push(Prop::new("DESCRIPTION", ical::escape(n)));
            }
            if let Some(due) = s("due")
                .and_then(|d| d.get(..10))
                .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
            {
                c.props.push(super::time::Moment::Date(due).to_prop("DUE"));
            }
            let stamp = |k: &str| {
                s(k).and_then(|x| DateTime::parse_from_rfc3339(x).ok())
                    .map(|d| d.naive_utc().format("%Y%m%dT%H%M%SZ").to_string())
            };
            if s("status") == Some("completed") {
                c.props.push(Prop::new("STATUS", "COMPLETED"));
                if let Some(at) = stamp("completed") {
                    c.props.push(Prop::new("COMPLETED", at));
                }
            } else {
                c.props.push(Prop::new("STATUS", "NEEDS-ACTION"));
            }
            if let Some(at) = stamp("updated") {
                c.props.push(Prop::new("LAST-MODIFIED", at.clone()));
                c.props.push(Prop::new("DTSTAMP", at));
            }
            if let Some(p) = s("parent") {
                c.props.push(Prop::new(
                    "RELATED-TO",
                    ical::escape(&format!("{p}@google-tasks")),
                ));
            }
            cal.children.push(c);
        }
        out.push(Source {
            name,
            color: None,
            cal,
        });
    }
    Ok(out)
}

fn sources_from_file(name: &str, bytes: Vec<u8>) -> Result<Vec<Source>> {
    let lower = name.to_ascii_lowercase();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let base = Path::new(name);
    if lower.ends_with(".ics") || lower.ends_with(".ical") || lower.ends_with(".ifb") {
        return sources_from_ics(&text, &stem_name(base));
    }
    if lower.ends_with(".json") {
        return sources_from_tasks_json(&text);
    }
    bail!("{name}: not an .ics, .zip or Tasks.json file")
}

/// Reads every calendar at `path`: a file, a folder (recursively) or a zip.
pub fn read_path(path: &Path) -> Result<Vec<Source>> {
    if path.is_dir() {
        let mut out = Vec::new();
        let mut stack = vec![path.to_path_buf()];
        let mut files: Vec<PathBuf> = Vec::new();
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d)?.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    files.push(p);
                }
            }
        }
        files.sort();
        for p in files {
            let n = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let l = n.to_ascii_lowercase();
            if l.ends_with(".ics") || l == "tasks.json" {
                out.extend(
                    sources_from_file(&n, fs::read(&p)?)
                        .with_context(|| p.display().to_string())?,
                );
            }
        }
        return Ok(out);
    }
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if name.to_ascii_lowercase().ends_with(".zip") {
        let mut zip =
            zip::ZipArchive::new(std::io::Cursor::new(bytes)).context("reading the zip")?;
        let mut out = Vec::new();
        let mut names: Vec<String> = zip
            .file_names()
            .filter_map(|n| n.ok().map(|n| n.to_string()))
            .collect();
        names.sort();
        for n in names {
            let l = n.to_ascii_lowercase();
            let file = Path::new(&n)
                .file_name()
                .map(|f| f.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            if !(l.ends_with(".ics") || file == "tasks.json") {
                continue;
            }
            let mut f = zip.by_name(&n)?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            let short = Path::new(&n)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or(n.clone());
            out.extend(sources_from_file(&short, buf).with_context(|| n.clone())?);
        }
        return Ok(out);
    }
    sources_from_file(&name, bytes)
}

/// The import state: per UID, the source and the file as last imported.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Index {
    #[serde(default)]
    items: BTreeMap<String, Seen>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Seen {
    source: u64,
    written: u64,
}

impl Index {
    pub fn load(state_dir: &Path) -> Self {
        fs::read_to_string(state_dir.join("imports.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, state_dir: &Path) -> Result<()> {
        fs::create_dir_all(state_dir)?;
        fs::write(state_dir.join("imports.json"), serde_json::to_string(self)?)?;
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct Plan {
    pub changes: Vec<Change>,
    pub added: Vec<String>,
    pub updated: Vec<String>,
    /// Edited or deleted here since the last import: left alone.
    pub skipped: Vec<String>,
    pub unchanged: usize,
    pub new_lists: Vec<String>,
    seen: Vec<(String, Seen)>,
}

/// Splits a calendar into one VCALENDAR per UID, with the VTIMEZONEs it uses.
fn split(cal: &Component) -> Vec<(String, Component)> {
    let zones: HashMap<String, &Component> = cal
        .children_named("VTIMEZONE")
        .filter_map(|z| Some((z.text("TZID")?, z)))
        .collect();
    let mut groups: Vec<(String, Vec<&Component>)> = Vec::new();
    for c in cal
        .children
        .iter()
        .filter(|c| c.name == "VEVENT" || c.name == "VTODO")
    {
        let uid = c
            .text("UID")
            .unwrap_or_else(|| format!("{:016x}@import", hash(&ical::write(c))));
        match groups.iter_mut().find(|(u, _)| *u == uid) {
            Some((_, v)) => v.push(c),
            None => groups.push((uid, vec![c])),
        }
    }
    groups
        .into_iter()
        .map(|(uid, comps)| {
            let mut out = Component::new("VCALENDAR");
            for p in ["VERSION", "PRODID", "CALSCALE"] {
                if let Some(x) = cal.get(p) {
                    out.props.push(x.clone());
                }
            }
            if out.get("VERSION").is_none() {
                out.props.insert(0, Prop::new("VERSION", "2.0"));
            }
            let mut used: Vec<String> = Vec::new();
            for c in &comps {
                for p in &c.props {
                    if let Some(t) = p.param("TZID")
                        && !used.iter().any(|u| u == t)
                    {
                        used.push(t.to_string());
                    }
                }
            }
            for t in used {
                if let Some(z) = zones.get(&t) {
                    out.children.push((*z).clone());
                }
            }
            for c in comps {
                let mut c = c.clone();
                if c.get("UID").is_none() {
                    c.props.insert(0, Prop::new("UID", ical::escape(&uid)));
                }
                out.children.push(c);
            }
            (uid, out)
        })
        .collect()
}

/// Works out what an import would do, without changing anything.
pub fn plan(store: &Store, index: &Index, sources: &[Source], into: Option<&str>) -> Result<Plan> {
    let mut p = Plan::default();
    let mut planned_lists: HashSet<String> = HashSet::new();
    let mut seen_uids: HashSet<String> = HashSet::new();
    for src in sources {
        let list = match into {
            Some(l) => l.to_string(),
            None => slug(&src.name),
        };
        let groups = split(&src.cal);
        if groups.is_empty() {
            continue;
        }
        if store.list(&list).is_none() && planned_lists.insert(list.clone()) {
            super::store::validate_list_id(&list)?;
            let name = (into.is_none() && src.name != list).then(|| src.name.clone());
            p.changes.push(Change::CreateList {
                id: list.clone(),
                name,
            });
            if let Some(c) = &src.color {
                p.changes.push(Change::SetListColor {
                    id: list.clone(),
                    color: Some(c.clone()),
                });
            }
            p.new_lists.push(list.clone());
        }
        for (uid, cal) in groups {
            if !seen_uids.insert(uid.clone()) {
                continue;
            }
            let text = ical::write(&cal);
            let source = hash(&text);
            let Some(mut item) =
                Item::from_calendar(cal, &list, &format!("{}.ics", file_stem(&uid)))
            else {
                continue;
            };
            let title = item.f.title.clone();
            let existing = store.get(&uid);
            match (existing, index.items.get(&uid)) {
                (None, None) => {
                    p.seen.push((
                        uid.clone(),
                        Seen {
                            source,
                            written: hash(&item.to_ics()),
                        },
                    ));
                    p.added.push(title);
                    p.changes.push(Change::Put(item));
                }
                (None, Some(_)) => p.skipped.push(format!("{title} (deleted here)")),
                (Some(_), Some(seen)) if seen.source == source => p.unchanged += 1,
                (Some(cur), Some(seen)) if hash(&cur.to_ics()) == seen.written => {
                    item.list = cur.list.clone();
                    item.file = cur.file.clone();
                    p.seen.push((
                        uid.clone(),
                        Seen {
                            source,
                            written: hash(&item.to_ics()),
                        },
                    ));
                    p.updated.push(title);
                    p.changes.push(Change::Put(item));
                }
                (Some(cur), _) => {
                    if hash(&cur.to_ics()) == hash(&item.to_ics()) {
                        p.unchanged += 1;
                    } else {
                        p.skipped.push(format!("{title} (edited here)"));
                    }
                }
            }
        }
    }
    Ok(p)
}

pub fn apply(
    store: &mut Store,
    journal: &mut Journal,
    index: &mut Index,
    plan: Plan,
    label: &str,
) -> Result<()> {
    if !plan.changes.is_empty() {
        store.commit(journal, label, plan.changes)?;
    }
    for (uid, seen) in plan.seen {
        index.items.insert(uid, seen);
    }
    Ok(())
}

/// Everything (or one list) as one standard `.ics`.
pub fn export(store: &Store, list: Option<&str>) -> String {
    let mut cal = Component::new("VCALENDAR");
    cal.props.push(Prop::new("VERSION", "2.0"));
    cal.props.push(Prop::new("PRODID", "-//ctui//ctui//EN"));
    if let Some(l) = list.and_then(|l| store.list(l)) {
        cal.props
            .push(Prop::new("X-WR-CALNAME", ical::escape(&l.name)));
        if let Some(c) = &l.color {
            cal.props
                .push(Prop::new("X-APPLE-CALENDAR-COLOR", c.clone()));
        }
    }
    let mut zones: HashSet<String> = HashSet::new();
    let mut items: Vec<&Item> = store
        .items
        .iter()
        .filter(|i| list.is_none_or(|l| i.list == l))
        .collect();
    items.sort_by(|a, b| (&a.list, &a.file).cmp(&(&b.list, &b.file)));
    for item in items {
        for c in &item.cal.children {
            if c.name == "VTIMEZONE" {
                let id = c.text("TZID").unwrap_or_default();
                if !zones.insert(id) {
                    continue;
                }
                cal.children.insert(0, c.clone());
            } else {
                cal.children.push(c.clone());
            }
        }
    }
    ical::write(&cal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::Kind;
    use chrono_tz::Tz;

    const TAKEOUT: &str = "BEGIN:VCALENDAR\r\nPRODID:-//Google Inc//Google Calendar 70.9054//EN\r\nVERSION:2.0\r\nX-WR-CALNAME:Work Stuff\r\nBEGIN:VTIMEZONE\r\nTZID:Europe/London\r\nBEGIN:STANDARD\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0000\r\nDTSTART:19701025T020000\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nDTSTART;TZID=Europe/London:20261015T093000\r\nDTEND;TZID=Europe/London:20261015T100000\r\nUID:one@google.com\r\nSUMMARY:Standup\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nDTSTART;VALUE=DATE:20261017\r\nUID:two@google.com\r\nSUMMARY:Birthday\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ctui-imp-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn import_is_idempotent_and_respects_local_edits() {
        let dir = temp("idem");
        let mut store = Store::open(&dir.join("data"), None).unwrap();
        let mut j = Journal::load(&dir.join("state"));
        let mut idx = Index::default();
        let src = sources_from_ics(TAKEOUT, "x").unwrap();
        let p = plan(&store, &idx, &src, None).unwrap();
        assert_eq!(p.added.len(), 2);
        assert_eq!(p.new_lists, vec!["work-stuff"]);
        apply(&mut store, &mut j, &mut idx, p, "import").unwrap();
        assert_eq!(store.list("work-stuff").unwrap().name, "Work Stuff");
        let standup = store.get("one@google.com").unwrap();
        assert!(
            standup.to_ics().contains("BEGIN:VTIMEZONE"),
            "the zone travels with the item"
        );
        assert!(
            !store
                .get("two@google.com")
                .unwrap()
                .to_ics()
                .contains("VTIMEZONE")
        );

        let again = plan(&store, &idx, &src, None).unwrap();
        assert_eq!(
            (again.added.len(), again.updated.len(), again.unchanged),
            (0, 0, 2)
        );

        // A newer export changes both; one was edited here.
        let newer = TAKEOUT
            .replace("SUMMARY:Standup", "SUMMARY:Daily standup")
            .replace("SUMMARY:Birthday", "SUMMARY:Bday");
        let mut b = store.get("two@google.com").unwrap().clone();
        b.f.title = "Sam's birthday".into();
        b.sync(chrono::Utc::now().with_timezone(&Tz::UTC));
        store.commit(&mut j, "edit", vec![Change::Put(b)]).unwrap();
        let src2 = sources_from_ics(&newer, "x").unwrap();
        let p2 = plan(&store, &idx, &src2, None).unwrap();
        assert_eq!(p2.updated, vec!["Daily standup"]);
        assert_eq!(p2.skipped, vec!["Bday (edited here)"]);
        apply(&mut store, &mut j, &mut idx, p2, "import").unwrap();
        assert_eq!(
            store.get("one@google.com").unwrap().f.title,
            "Daily standup"
        );
        assert_eq!(
            store.get("two@google.com").unwrap().f.title,
            "Sam's birthday"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn google_tasks_json() {
        let json = r#"{"kind":"tasks#taskLists","items":[{"kind":"tasks#taskList","id":"L1","title":"My Tasks","items":[
            {"kind":"tasks#task","id":"t1","title":"buy milk","status":"needsAction","due":"2026-10-20T00:00:00.000Z","updated":"2026-10-01T10:00:00.000Z"},
            {"kind":"tasks#task","id":"t2","title":"done thing","status":"completed","completed":"2026-10-02T10:00:00.000Z","notes":"keep me"},
            {"kind":"tasks#task","id":"t3","title":"sub","status":"needsAction","parent":"t1"}]}]}"#;
        let src = sources_from_tasks_json(json).unwrap();
        assert_eq!(src[0].name, "My Tasks");
        let dir = temp("tasks");
        let store = Store::open(&dir, None).unwrap();
        let p = plan(&store, &Index::default(), &src, None).unwrap();
        assert_eq!(p.added, vec!["buy milk", "done thing", "sub"]);
        let items: Vec<&Item> = p
            .changes
            .iter()
            .filter_map(|c| {
                if let Change::Put(i) = c {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();
        assert!(
            items
                .iter()
                .all(|i| i.kind == Kind::Task && i.list == "my-tasks")
        );
        assert_eq!(
            items[0].f.due.unwrap().date(Tz::UTC).to_string(),
            "2026-10-20"
        );
        assert!(items[1].f.done);
        assert!(items[1].to_ics().contains("DESCRIPTION:keep me"));
        assert_eq!(items[2].f.parent.as_deref(), Some("t1@google-tasks"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reads_a_takeout_zip() {
        let dir = temp("zip");
        let zpath = dir.join("takeout.zip");
        {
            let f = fs::File::create(&zpath).unwrap();
            let mut z = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            z.start_file("Takeout/Calendar/Work.ics", opts).unwrap();
            std::io::Write::write_all(&mut z, TAKEOUT.as_bytes()).unwrap();
            z.start_file("Takeout/Calendar/notes.txt", opts).unwrap();
            std::io::Write::write_all(&mut z, b"ignored").unwrap();
            z.finish().unwrap();
        }
        let src = read_path(&zpath).unwrap();
        assert_eq!(src.len(), 1);
        assert_eq!(src[0].name, "Work Stuff");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn export_round_trips() {
        let dir = temp("export");
        let mut store = Store::open(&dir.join("data"), None).unwrap();
        let mut j = Journal::load(&dir.join("state"));
        let mut idx = Index::default();
        let src = sources_from_ics(TAKEOUT, "x").unwrap();
        let p = plan(&store, &idx, &src, Some("work")).unwrap();
        apply(&mut store, &mut j, &mut idx, p, "import").unwrap();
        let out = export(&store, Some("work"));
        let back = sources_from_ics(&out, "x").unwrap();
        assert_eq!(back[0].cal.children_named("VEVENT").count(), 2);
        assert_eq!(back[0].cal.children_named("VTIMEZONE").count(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }
}
