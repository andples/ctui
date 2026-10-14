//! The vdir: one folder per list, one `.ics` file per item.
//!
//! [`Store::commit`] is the only way data changes. It writes files
//! atomically, keeps deleted files in the trash and records an undo entry.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration as StdDuration, SystemTime};

use anyhow::{Context, Result, bail};

use super::ical;
use super::model::{Item, List};
use super::undo::{FileChange, Journal};

pub struct Store {
    pub root: PathBuf,
    pub lists: Vec<List>,
    pub items: Vec<Item>,
    /// Files that couldn't be read, with the reason.
    pub warnings: Vec<String>,
    state_dir: Option<PathBuf>,
    signature: u64,
}

/// One change inside a commit. (Short-lived, so the big `Put` isn't boxed.)
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Change {
    /// Create or replace an item (moving its file if its list changed).
    Put(Item),
    Remove(String),
    CreateList {
        id: String,
        name: Option<String>,
    },
    SetListName {
        id: String,
        name: Option<String>,
    },
    SetListColor {
        id: String,
        color: Option<String>,
    },
}

impl Store {
    pub fn open(root: &Path, state_dir: Option<&Path>) -> Result<Self> {
        fs::create_dir_all(root).with_context(|| format!("creating {}", root.display()))?;
        let mut s = Self {
            root: root.to_path_buf(),
            lists: Vec::new(),
            items: Vec::new(),
            warnings: Vec::new(),
            state_dir: state_dir.map(Path::to_path_buf),
            signature: 0,
        };
        s.reload()?;
        if let Some(dir) = &s.state_dir {
            purge_trash(&dir.join("trash"), StdDuration::from_secs(30 * 86_400));
        }
        Ok(s)
    }

    /// Re-reads everything from disk.
    pub fn reload(&mut self) -> Result<()> {
        let mut lists = Vec::new();
        let mut items = Vec::new();
        let mut warnings = Vec::new();
        let mut entries: Vec<_> = fs::read_dir(&self.root)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            let id = entry.file_name().to_string_lossy().to_string();
            if !path.is_dir() || id.starts_with('.') {
                continue;
            }
            let read_meta = |n: &str| {
                fs::read_to_string(path.join(n))
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            };
            lists.push(List {
                name: read_meta("displayname").unwrap_or_else(|| id.clone()),
                color: read_meta("color").and_then(|c| normalize_color(&c)),
                id: id.clone(),
            });
            let mut files: Vec<_> = fs::read_dir(&path)?.filter_map(|e| e.ok()).collect();
            files.sort_by_key(|e| e.file_name());
            for f in files {
                let name = f.file_name().to_string_lossy().to_string();
                if !name.to_ascii_lowercase().ends_with(".ics") {
                    continue;
                }
                let text = match fs::read_to_string(f.path()) {
                    Ok(t) => t,
                    Err(e) => {
                        warnings.push(format!("{id}/{name}: {e}"));
                        continue;
                    }
                };
                match ical::parse(&text) {
                    Ok(mut cals) if !cals.is_empty() => {
                        if let Some(item) = Item::from_calendar(cals.remove(0), &id, &name) {
                            items.push(item);
                        }
                    }
                    Ok(_) => warnings.push(format!("{id}/{name}: empty file")),
                    Err(e) => warnings.push(format!("{id}/{name}: {e}")),
                }
            }
        }
        self.lists = lists;
        self.items = items;
        self.warnings = warnings;
        self.signature = self.disk_signature();
        Ok(())
    }

    /// A cheap fingerprint of the folders' and files' names, sizes and times.
    fn disk_signature(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let mut stack = vec![self.root.clone()];
        let mut seen = Vec::new();
        while let Some(dir) = stack.pop() {
            let Ok(rd) = fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let Ok(meta) = e.metadata() else { continue };
                let name = e.path();
                if meta.is_dir() {
                    if dir == self.root && !e.file_name().to_string_lossy().starts_with('.') {
                        stack.push(name.clone());
                    } else {
                        continue;
                    }
                }
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok());
                seen.push((name, meta.len(), mtime));
            }
        }
        seen.sort();
        seen.hash(&mut h);
        h.finish()
    }

    /// True when something changed on disk since the last read or write.
    pub fn changed_on_disk(&self) -> bool {
        self.disk_signature() != self.signature
    }

    pub fn get(&self, uid: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.uid == uid)
    }

    pub fn list(&self, id: &str) -> Option<&List> {
        self.lists.iter().find(|l| l.id == id)
    }

    fn rel(list: &str, file: &str) -> String {
        format!("{list}/{file}")
    }

    fn abs(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn read_rel(&self, rel: &str) -> Option<String> {
        fs::read_to_string(self.abs(rel)).ok()
    }

    /// Applies `changes` as one undoable step called `label`.
    pub fn commit(
        &mut self,
        journal: &mut Journal,
        label: &str,
        changes: Vec<Change>,
    ) -> Result<()> {
        let mut planned: BTreeMap<String, Option<String>> = BTreeMap::new();
        let mut dirs = Vec::new();
        for ch in changes {
            match ch {
                Change::Put(item) => {
                    let new_rel = Self::rel(&item.list, &item.file);
                    if let Some(old) = self.items.iter().find(|i| i.uid == item.uid) {
                        let old_rel = Self::rel(&old.list, &old.file);
                        if old_rel != new_rel {
                            planned.insert(old_rel, None);
                        }
                    }
                    dirs.push(item.list.clone());
                    planned.insert(new_rel, Some(item.to_ics()));
                }
                Change::Remove(uid) => {
                    let Some(old) = self.items.iter().find(|i| i.uid == uid) else {
                        bail!("no item {uid}")
                    };
                    planned.insert(Self::rel(&old.list, &old.file), None);
                }
                Change::CreateList { id, name } => {
                    validate_list_id(&id)?;
                    dirs.push(id.clone());
                    if let Some(n) = name.filter(|n| *n != id) {
                        planned.insert(format!("{id}/displayname"), Some(format!("{n}\n")));
                    }
                }
                Change::SetListName { id, name } => {
                    planned.insert(format!("{id}/displayname"), name.map(|n| format!("{n}\n")));
                }
                Change::SetListColor { id, color } => {
                    planned.insert(format!("{id}/color"), color.map(|c| format!("{c}\n")));
                }
            }
        }
        let mut files = Vec::new();
        for (rel, after) in planned {
            let before = self.read_rel(&rel);
            if before == after {
                continue;
            }
            files.push(FileChange {
                path: rel,
                before,
                after,
            });
        }
        for d in &dirs {
            fs::create_dir_all(self.root.join(d))?;
        }
        self.write_changes(&files, true)?;
        if !files.is_empty() {
            journal.record(&self.root, label, files)?;
        }
        self.reload()
    }

    /// Writes (or removes) files; removed files go to the trash.
    pub(crate) fn write_changes(&self, files: &[FileChange], forward: bool) -> Result<()> {
        // A file that only moved (its content is written elsewhere) isn't trashed.
        let written: Vec<&String> = files
            .iter()
            .filter_map(|f| {
                if forward {
                    f.after.as_ref()
                } else {
                    f.before.as_ref()
                }
            })
            .collect();
        for fc in files {
            let path = self.abs(&fc.path);
            let content = if forward { &fc.after } else { &fc.before };
            match content {
                Some(text) => {
                    if let Some(dir) = path.parent() {
                        fs::create_dir_all(dir)?;
                    }
                    atomic_write(&path, text)?;
                }
                None => {
                    if path.exists() {
                        let old = if forward { &fc.before } else { &fc.after };
                        if !old.as_ref().is_some_and(|o| written.contains(&o)) {
                            self.to_trash(&fc.path, &path);
                        }
                        fs::remove_file(&path)
                            .with_context(|| format!("removing {}", path.display()))?;
                    }
                }
            }
        }
        Ok(())
    }

    fn to_trash(&self, rel: &str, path: &Path) {
        let Some(dir) = &self.state_dir else { return };
        let trash = dir.join("trash");
        if fs::create_dir_all(&trash).is_err() {
            return;
        }
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = fs::copy(
            path,
            trash.join(format!("{stamp}-{}", rel.replace('/', "__"))),
        );
    }

    /// Removes an empty list folder (its metadata files included).
    pub fn remove_list(&mut self, id: &str) -> Result<()> {
        if self.items.iter().any(|i| i.list == id) {
            bail!("list {id} isn't empty");
        }
        let dir = self.root.join(id);
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        self.reload()
    }

    /// Reverts or re-applies a journal step. Refuses when a file changed
    /// since, so undo never clobbers an edit made elsewhere.
    pub fn replay(&mut self, files: &[FileChange], undo: bool) -> Result<()> {
        for fc in files {
            let expect = if undo { &fc.after } else { &fc.before };
            if self.read_rel(&fc.path) != *expect {
                bail!("{} changed since, so it can't be undone", fc.path);
            }
        }
        self.write_changes(files, !undo)?;
        self.reload()
    }

    /// Lists by id, for colours and names.
    pub fn list_map(&self) -> HashMap<&str, &List> {
        self.lists.iter().map(|l| (l.id.as_str(), l)).collect()
    }
}

pub fn atomic_write(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("ics.tmp");
    {
        let mut f = fs::File::create(&tmp).with_context(|| format!("writing {}", tmp.display()))?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn purge_trash(dir: &Path, older_than: StdDuration) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let now = SystemTime::now();
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > older_than);
        if old {
            let _ = fs::remove_file(e.path());
        }
    }
}

pub fn validate_list_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.starts_with('.')
        || !id
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        bail!("list names are letters, digits, - _ and . ({id:?} isn't)");
    }
    Ok(())
}

/// A folder name from a calendar name (`Work stuff` → `work-stuff`).
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "calendar".into()
    } else {
        out
    }
}

/// `#rgb`/`#rrggbb`/`#rrggbbaa` (as CalDAV servers write them) → `#rrggbb`.
pub fn normalize_color(c: &str) -> Option<String> {
    let h = c.trim().strip_prefix('#')?;
    if !h.chars().all(|x| x.is_ascii_hexdigit()) {
        return None;
    }
    let full = match h.len() {
        3 => h.chars().flat_map(|x| [x, x]).collect(),
        6 | 8 => h[..6].to_string(),
        _ => return None,
    };
    Some(format!("#{}", full.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{Fields, Kind};
    use chrono::TimeZone;
    use chrono_tz::Tz;

    pub fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ctui-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn write_move_remove_undo() {
        let dir = temp("store");
        let now = Tz::UTC.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap();
        let mut s = Store::open(&dir.join("data"), Some(&dir.join("state"))).unwrap();
        let mut j = Journal::load(&dir.join("state"));
        let it = Item::new(
            Kind::Task,
            "personal",
            Fields {
                title: "milk".into(),
                ..Fields::default()
            },
            now,
        );
        s.commit(&mut j, "add", vec![Change::Put(it.clone())])
            .unwrap();
        assert_eq!(s.items.len(), 1);
        assert_eq!(s.lists[0].id, "personal");

        let mut moved = s.items[0].clone();
        moved.list = "work".into();
        s.commit(&mut j, "move", vec![Change::Put(moved)]).unwrap();
        assert_eq!(s.items[0].list, "work");
        assert!(!dir.join("data/personal").join(&it.file).exists());

        s.commit(&mut j, "rm", vec![Change::Remove(it.uid.clone())])
            .unwrap();
        assert!(s.items.is_empty());
        assert_eq!(fs::read_dir(dir.join("state/trash")).unwrap().count(), 1);

        assert_eq!(j.undo(&mut s).unwrap().as_deref(), Some("rm"));
        assert_eq!(s.items.len(), 1);
        assert_eq!(j.undo(&mut s).unwrap().as_deref(), Some("move"));
        assert_eq!(s.items[0].list, "personal");
        assert_eq!(j.redo(&mut s).unwrap().as_deref(), Some("move"));
        assert_eq!(s.items[0].list, "work");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn undo_refuses_after_outside_edit() {
        let dir = temp("store2");
        let now = Tz::UTC.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap();
        let mut s = Store::open(&dir.join("data"), None).unwrap();
        let mut j = Journal::load(&dir.join("state"));
        let it = Item::new(
            Kind::Task,
            "p",
            Fields {
                title: "a".into(),
                ..Fields::default()
            },
            now,
        );
        s.commit(&mut j, "add", vec![Change::Put(it.clone())])
            .unwrap();
        fs::write(dir.join("data/p").join(&it.file), "changed").unwrap();
        assert!(j.undo(&mut s).is_err());
        assert!(s.changed_on_disk());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn list_metadata_files() {
        let dir = temp("store3");
        fs::create_dir_all(dir.join("w")).unwrap();
        fs::write(dir.join("w/displayname"), "Work\n").unwrap();
        fs::write(dir.join("w/color"), "#A3BE8CFF\n").unwrap();
        let s = Store::open(&dir, None).unwrap();
        assert_eq!(s.lists[0].name, "Work");
        assert_eq!(s.lists[0].color.as_deref(), Some("#a3be8c"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Work stuff!"), "work-stuff");
        assert_eq!(slug("  "), "calendar");
        assert_eq!(normalize_color("#abc").as_deref(), Some("#aabbcc"));
        assert!(validate_list_id("../x").is_err());
    }
}
