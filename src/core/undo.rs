//! The undo journal: every commit's file contents before and after, shared by
//! the CLI and the TUI through a file in the state dir.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::store::Store;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileChange {
    /// Relative to the data dir.
    pub path: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub label: String,
    pub files: Vec<FileChange>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Stacks {
    /// The data dir the entries belong to.
    root: PathBuf,
    undo: Vec<Entry>,
    redo: Vec<Entry>,
}

pub struct Journal {
    path: PathBuf,
}

/// Steps kept; older ones fall off.
const KEEP: usize = 100;
/// Big imports make big entries; past this the oldest go first.
const MAX_BYTES: usize = 32 * 1024 * 1024;

impl Journal {
    pub fn load(state_dir: &Path) -> Self {
        Self { path: state_dir.join("undo.json") }
    }

    fn read(&self, root: &Path) -> Stacks {
        let s: Stacks = fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        if s.root == root { s } else { Stacks { root: root.to_path_buf(), ..Stacks::default() } }
    }

    fn write(&self, s: &Stacks) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string(s)?)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub fn record(&mut self, root: &Path, label: &str, files: Vec<FileChange>) -> Result<()> {
        let mut s = self.read(root);
        s.undo.push(Entry { label: label.to_string(), files });
        s.redo.clear();
        let size = |e: &Entry| -> usize {
            e.files.iter().map(|f| f.before.as_ref().map_or(0, String::len) + f.after.as_ref().map_or(0, String::len)).sum()
        };
        while s.undo.len() > KEEP || (s.undo.len() > 1 && s.undo.iter().map(size).sum::<usize>() > MAX_BYTES) {
            s.undo.remove(0);
        }
        self.write(&s)
    }

    /// Undoes the last step; returns its label, or `None` when there's nothing.
    pub fn undo(&mut self, store: &mut Store) -> Result<Option<String>> {
        let mut s = self.read(&store.root);
        let Some(entry) = s.undo.pop() else { return Ok(None) };
        store.replay(&entry.files, true)?;
        let label = entry.label.clone();
        s.redo.push(entry);
        self.write(&s)?;
        Ok(Some(label))
    }

    pub fn redo(&mut self, store: &mut Store) -> Result<Option<String>> {
        let mut s = self.read(&store.root);
        let Some(entry) = s.redo.pop() else { return Ok(None) };
        store.replay(&entry.files, false)?;
        let label = entry.label.clone();
        s.undo.push(entry);
        self.write(&s)?;
        Ok(Some(label))
    }
}
