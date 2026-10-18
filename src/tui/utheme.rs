//! Live themes from utheme, macOS only (`crate::utheme_client`). With
//! `theme = "utheme"` the TUI shows utheme's current palette and recolours
//! on every switch; any other theme leaves utheme alone. The live theme sits
//! in `Session::live_theme`, which the name `utheme` then means; without one
//! ctui falls back to the static `themes/utheme.toml`, then `default`.

use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event;

use super::app::{App, Mode};
use crate::theme::Theme;
use crate::utheme_client::{self, Client, Paths, Snapshot, Update};

/// The palette keys the role mapping reads.
const NEEDS: &[&str] = &[
    "background",
    "foreground",
    "muted",
    "accent",
    "red",
    "orange",
    "yellow",
    "green",
    "cyan",
    "blue",
    "magenta",
];

/// How often the event loop looks for a switch while following. The watcher
/// thread does the reading; this only bounds how long a recolour waits.
const WAKE: Duration = Duration::from_millis(200);

/// Following state. Dropping the client removes the heartbeat.
#[derive(Default)]
pub struct Live {
    client: Option<Client>,
    /// Where utheme lives, when not `$UTHEME_HOME`/`$HOME` (tests).
    pub(super) paths: Option<Paths>,
}

/// utheme's palette in ctui's roles (protocol section 7). It goes through
/// `Theme::parse` like the static theme utheme generates, so the derived
/// colours (selection, border, …) come out the same.
pub fn theme_from(s: &Snapshot) -> Result<Theme, String> {
    let c = |k: &str| s.color(k).map(|c| c.hex());
    let lists = [
        "red", "orange", "yellow", "green", "cyan", "blue", "magenta",
    ]
    .iter()
    .map(|k| c(k).map(|h| format!("\"{h}\"")))
    .collect::<Result<Vec<_>, _>>()?
    .join(", ");
    let text = format!(
        "name = \"utheme\"\n[colors]\nbg = \"{}\"\nfg = \"{}\"\nsub = \"{}\"\nmain = \"{}\"\nerror = \"{}\"\nlists = [{lists}]\n",
        c("background")?,
        c("foreground")?,
        c("muted")?,
        c("accent")?,
        c("red")?,
    );
    Theme::parse(&text, utheme_client::THEME).map_err(|e| format!("{e:#}"))
}

/// What woke the event loop.
pub enum Wake {
    /// Input is waiting.
    Event,
    /// The usual timeout passed: run `App::tick`.
    Tick,
    /// A new theme arrived: just redraw.
    Recolour,
}

impl App {
    /// Starts or stops following to match `cfg.theme` (the picker or a `:`
    /// command can change it), then applies whatever switch has arrived.
    /// True when the colours changed.
    pub fn utheme_tick(&mut self) -> bool {
        let wanted = self.s.cfg.theme == utheme_client::THEME;
        let mut changed = false;
        if wanted && self.utheme.client.is_none() {
            let (app, version) = ("ctui", env!("CARGO_PKG_VERSION"));
            let started = match &self.utheme.paths {
                Some(p) => Some(Client::start_in(p.clone(), app, version, NEEDS)),
                None => Client::start(app, version, NEEDS),
            };
            if let Some((client, first)) = started {
                self.utheme.client = Some(client);
                changed = self.utheme_apply(first);
            }
        } else if !wanted && self.utheme.client.is_some() {
            self.utheme_stop();
        }
        if let Some(u) = self.utheme.client.as_mut().and_then(Client::poll) {
            changed |= self.utheme_apply(u);
        }
        changed
    }

    fn utheme_apply(&mut self, update: Update) -> bool {
        // The picker shows its highlighted theme; it picks the new one up on esc/enter.
        let previewing = matches!(self.mode, Mode::Themes { .. });
        match update {
            Update::Theme(snap) => match theme_from(&snap) {
                Ok(t) => {
                    self.s.live_theme = Some(t.clone());
                    if !previewing {
                        self.s.theme = t;
                    }
                    // Without a static utheme.toml the session warned it was missing.
                    if self
                        .toast
                        .as_ref()
                        .is_some_and(|(m, _)| m.contains("theme \"utheme\""))
                    {
                        self.toast = None;
                    }
                    true
                }
                Err(e) => {
                    self.utheme_error(e);
                    false
                }
            },
            // Keep the colours already showing.
            Update::Error(e) => {
                self.utheme_error(e);
                false
            }
            Update::Gone => {
                let was = self.s.live_theme.take().is_some();
                if was && !previewing {
                    self.s.reload_theme();
                }
                was
            }
        }
    }

    fn utheme_error(&mut self, e: String) {
        self.toast = Some((format!("utheme: {e}"), true));
    }

    /// Stop following and remove the heartbeat (on quit, or another theme).
    pub fn utheme_stop(&mut self) {
        self.utheme.client = None;
        self.s.live_theme = None;
    }

    /// `event::poll(timeout)`, looking for a utheme switch every `WAKE`
    /// while following.
    pub fn utheme_wait(&mut self, timeout: Duration) -> Result<Wake> {
        if self.utheme_tick() {
            return Ok(Wake::Recolour);
        }
        if self.utheme.client.is_none() {
            return Ok(if event::poll(timeout)? {
                Wake::Event
            } else {
                Wake::Tick
            });
        }
        let until = Instant::now() + timeout;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(Wake::Tick);
            }
            if event::poll(left.min(WAKE))? {
                return Ok(Wake::Event);
            }
            if self.utheme_tick() {
                return Ok(Wake::Recolour);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Color;
    use crate::utheme_client::Rgb;

    #[test]
    fn roles_match_the_static_theme() {
        let keys = [
            ("background", "#2e3440"),
            ("foreground", "#d8dee9"),
            ("muted", "#4c566a"),
            ("accent", "#88C0D0"),
            ("red", "#bf616a"),
            ("orange", "#d08770"),
            ("yellow", "#ebcb8b"),
            ("green", "#a3be8c"),
            ("cyan", "#8fbcbb"),
            ("blue", "#81a1c1"),
            ("magenta", "#b48ead"),
        ];
        let snap = Snapshot {
            generation: "g1".into(),
            id: "nord".into(),
            name: "Nord".into(),
            mode: "dark".into(),
            palette: keys
                .iter()
                .map(|(k, v)| (k.to_string(), Rgb::parse(v).unwrap()))
                .collect(),
        };
        let live = theme_from(&snap).unwrap();
        // What utheme's ctui.toml.tpl renders for the same palette.
        let fixed = Theme::parse(
            "name = \"utheme\"\n[colors]\nbg = \"#2e3440\"\nfg = \"#d8dee9\"\nsub = \"#4c566a\"\nmain = \"#88c0d0\"\nerror = \"#bf616a\"\nlists = [\"#bf616a\", \"#d08770\", \"#ebcb8b\", \"#a3be8c\", \"#8fbcbb\", \"#81a1c1\", \"#b48ead\"]\n",
            "utheme",
        )
        .unwrap();
        assert_eq!(live, fixed);
        assert_eq!(live.main, Color::Rgb(0x88, 0xc0, 0xd0));
        assert_eq!(live.lists.len(), 7);
        assert_eq!(live.lists[6], Color::Rgb(0xb4, 0x8e, 0xad));

        let mut partial = snap.clone();
        partial.palette.remove("cyan");
        assert!(theme_from(&partial).unwrap_err().contains("cyan"));
    }
}
