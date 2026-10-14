//! Everything a command needs: paths, settings, the store and the journal.

use anyhow::Result;
use chrono::DateTime;
use chrono_tz::Tz;

use crate::config::{Config, Paths};
use crate::core::ops::Env;
use crate::core::store::Store;
use crate::core::time;
use crate::core::undo::Journal;
use crate::theme::{self, Slots, Theme};

pub struct Session {
    pub paths: Paths,
    pub cfg: Config,
    pub store: Store,
    pub journal: Journal,
    pub tz: Tz,
    pub theme: Theme,
    pub slots: Slots,
    /// Problems worth telling the user once (a broken theme file, unreadable items).
    pub warnings: Vec<String>,
    /// Fixed "now" for tests; the clock otherwise.
    pub fixed_now: Option<DateTime<Tz>>,
}

impl Session {
    pub fn open(paths: Paths, cfg: Config) -> Result<Self> {
        let store = Store::open(&paths.data_dir, Some(&paths.state_dir))?;
        let journal = Journal::load(&paths.state_dir);
        let tz = time::local_zone();
        let (theme, warn) = theme::load_or_default(&cfg.theme, &paths.themes_dir());
        let mut slots = Slots::load(&paths.state_dir);
        slots.assign(&store.lists);
        let mut warnings: Vec<String> = warn.into_iter().collect();
        warnings.extend(store.warnings.iter().map(|w| format!("couldn't read {w}")));
        let fixed_now = std::env::var("CTUI_NOW").ok().and_then(|s| {
            DateTime::parse_from_rfc3339(&s)
                .ok()
                .map(|d| d.with_timezone(&tz))
        });
        Ok(Self {
            paths,
            cfg,
            store,
            journal,
            tz,
            theme,
            slots,
            warnings,
            fixed_now,
        })
    }

    pub fn now(&self) -> DateTime<Tz> {
        self.fixed_now.unwrap_or_else(|| time::now_in(self.tz))
    }

    /// Splits the session so an op can borrow the store mutably next to the env.
    pub fn parts(&mut self) -> (&mut Store, &mut Journal, Env<'_>) {
        let now = self.now();
        let env = Env {
            now,
            tz: self.tz,
            cfg: &self.cfg,
        };
        (&mut self.store, &mut self.journal, env)
    }

    pub fn env(&self) -> Env<'_> {
        Env {
            now: self.now(),
            tz: self.tz,
            cfg: &self.cfg,
        }
    }

    /// Re-reads the theme after a config change.
    pub fn reload_theme(&mut self) -> Option<String> {
        let (theme, warn) = theme::load_or_default(&self.cfg.theme, &self.paths.themes_dir());
        self.theme = theme;
        warn
    }

    /// After the lists change: hand out palette slots to new ones.
    pub fn refresh_slots(&mut self) {
        self.slots.assign(&self.store.lists);
    }

    pub fn list_color(&self, list: &str) -> theme::Color {
        match self.store.list(list) {
            Some(l) => theme::list_color(l, &self.theme, &self.cfg, &self.slots),
            None => self.theme.sub,
        }
    }
}
