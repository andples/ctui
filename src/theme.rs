//! Colour themes (DESIGN.md §10.1): five colours and a list palette, with
//! the rest derived. Shared by the TUI and the CLI's coloured output.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::config::{Config, ListColors};
use crate::core::model::List;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    /// The terminal's own foreground/background.
    Reset,
    /// One of the 16 terminal colours (0–15).
    Ansi(u8),
    Rgb(u8, u8, u8),
}

const ANSI: [&str; 8] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];

impl Color {
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim().to_ascii_lowercase();
        if s == "reset" || s == "default" || s == "none" {
            return Ok(Color::Reset);
        }
        if let Some(h) = s.strip_prefix('#') {
            let full: String = match h.len() {
                3 => h.chars().flat_map(|c| [c, c]).collect(),
                6 => h.to_string(),
                _ => bail!("{s:?} isn't #rgb or #rrggbb"),
            };
            let v = u32::from_str_radix(&full, 16).with_context(|| format!("{s:?} isn't a hex colour"))?;
            return Ok(Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8));
        }
        let (bright, name) = match s.strip_prefix("bright-").or_else(|| s.strip_prefix("bright_")) {
            Some(n) => (true, n),
            None => (false, s.as_str()),
        };
        let name = if name == "gray" || name == "grey" { if bright { "white" } else { "black" } } else { name };
        let bright = bright || s == "gray" || s == "grey";
        match ANSI.iter().position(|a| *a == name) {
            Some(i) => Ok(Color::Ansi(i as u8 + if bright { 8 } else { 0 })),
            None => bail!("unknown colour {s:?}"),
        }
    }

    /// `self` blended `t` of the way toward `other`, when both are RGB.
    pub fn mix(self, other: Color, t: f32) -> Option<Color> {
        match (self, other) {
            (Color::Rgb(a, b, c), Color::Rgb(x, y, z)) => {
                let m = |p: u8, q: u8| (p as f32 + (q as f32 - p as f32) * t).round() as u8;
                Some(Color::Rgb(m(a, x), m(b, y), m(c, z)))
            }
            _ => None,
        }
    }

    pub fn is_rgb(self) -> bool {
        matches!(self, Color::Rgb(..))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub name: String,
    pub bg: Color,
    pub fg: Color,
    pub sub: Color,
    pub main: Color,
    pub error: Color,
    /// Selected row background; `None` means bold + marker only.
    pub select: Option<Color>,
    pub done: Color,
    pub overdue: Color,
    pub today: Color,
    pub border: Color,
    pub lists: Vec<Color>,
}

#[derive(Deserialize)]
struct ThemeFile {
    name: Option<String>,
    colors: ColorsFile,
}

#[derive(Deserialize)]
struct ColorsFile {
    bg: String,
    fg: String,
    sub: String,
    main: String,
    error: String,
    select: Option<String>,
    done: Option<String>,
    overdue: Option<String>,
    today: Option<String>,
    border: Option<String>,
    #[serde(default)]
    lists: Vec<String>,
}

impl Theme {
    pub fn parse(text: &str, fallback_name: &str) -> Result<Self> {
        let f: ThemeFile = toml::from_str(text)?;
        let c = &f.colors;
        let (bg, fg, sub, main, error) =
            (Color::parse(&c.bg)?, Color::parse(&c.fg)?, Color::parse(&c.sub)?, Color::parse(&c.main)?, Color::parse(&c.error)?);
        let opt = |v: &Option<String>| v.as_deref().map(Color::parse).transpose();
        let mut lists = c.lists.iter().map(|s| Color::parse(s)).collect::<Result<Vec<_>>>()?;
        if lists.is_empty() {
            lists.push(main);
        }
        Ok(Theme {
            name: f.name.unwrap_or_else(|| fallback_name.to_string()),
            select: match opt(&c.select)? {
                Some(s) => Some(s),
                None => main.mix(bg, 0.85),
            },
            done: opt(&c.done)?.unwrap_or(sub),
            overdue: opt(&c.overdue)?.unwrap_or(error),
            today: opt(&c.today)?.unwrap_or(main),
            border: opt(&c.border)?.unwrap_or_else(|| sub.mix(bg, 0.5).unwrap_or(sub)),
            bg,
            fg,
            sub,
            main,
            error,
            lists,
        })
    }

    pub fn fallback() -> Self {
        Self::parse(BUILTIN[0].1, "default").expect("the default theme parses")
    }
}

macro_rules! themes {
    ($($name:literal),* $(,)?) => { &[$(($name, include_str!(concat!("../themes/", $name, ".toml")))),*] };
}

/// Compiled-in themes. The Monkeytype ones (GPL-3.0) only with the
/// `monkeytype-themes` feature; otherwise they load from a themes folder.
const BUILTIN: &[(&str, &str)] = themes!(
    "default", "light", "catppuccin-latte", "catppuccin-mocha", "dracula", "everforest", "gruvbox", "kanagawa", "nord",
    "one-dark", "rose-pine", "solarized-dark", "solarized-light", "tokyo-night",
);

#[cfg(feature = "monkeytype-themes")]
const MONKEYTYPE: &[(&str, &str)] = themes!(
    "8008", "80s-after-dark", "aurora", "bento", "botanical", "carbon", "chaos-theory", "cherry-blossom", "comfy",
    "dark-magic-girl", "godspeed", "horizon", "iceberg-dark", "iceberg-light", "laser", "lavender", "matrix",
    "miami-nights", "modern-dolch", "nautilus", "olivia", "paper", "serika-dark", "serika", "superuser",
);
#[cfg(not(feature = "monkeytype-themes"))]
const MONKEYTYPE: &[(&str, &str)] = &[];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Builtin,
    File(PathBuf),
}

/// Folders searched for theme files, lowest priority first: packaged
/// themes (`share/ctui/themes` beside the binary and in the system data
/// dirs), then the user's `~/.config/ctui/themes`.
pub fn theme_dirs(user_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let data_dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for d in data_dirs.split(':').filter(|d| !d.is_empty()).rev() {
        dirs.push(Path::new(d).join("ctui/themes"));
    }
    if let Ok(exe) = std::env::current_exe().and_then(fs::canonicalize) {
        if let Some(prefix) = exe.parent().and_then(Path::parent) {
            dirs.push(prefix.join("share/ctui/themes"));
            // Running from a checkout (target/release/ctui): the repo's themes/.
            if let Some(repo) = prefix.parent() {
                if repo.join("themes/MONKEYTYPE.md").exists() {
                    dirs.push(repo.join("themes"));
                }
            }
        }
    }
    dirs.push(user_dir.to_path_buf());
    dirs
}

/// Every theme by name; later sources replace earlier ones.
pub fn available(user_dir: &Path) -> BTreeMap<String, Origin> {
    let mut out = BTreeMap::new();
    for (name, _) in BUILTIN.iter().chain(MONKEYTYPE) {
        out.insert(name.to_string(), Origin::Builtin);
    }
    for dir in theme_dirs(user_dir) {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "toml") {
                if let Some(stem) = p.file_stem() {
                    out.insert(stem.to_string_lossy().to_string(), Origin::File(p));
                }
            }
        }
    }
    out
}

pub fn load(name: &str, user_dir: &Path) -> Result<Theme> {
    let all = available(user_dir);
    match all.get(name) {
        Some(Origin::File(p)) => {
            let text = fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
            Theme::parse(&text, name).with_context(|| format!("theme {}", p.display()))
        }
        Some(Origin::Builtin) => {
            let text = BUILTIN.iter().chain(MONKEYTYPE).find(|(n, _)| *n == name).map(|(_, t)| *t).expect("listed");
            Theme::parse(text, name)
        }
        None => bail!("no theme {name:?}"),
    }
}

/// Loads the configured theme, falling back to the default with a warning.
pub fn load_or_default(name: &str, user_dir: &Path) -> (Theme, Option<String>) {
    match load(name, user_dir) {
        Ok(t) => (t, None),
        Err(e) => (Theme::fallback(), Some(format!("{e:#}; using the default theme"))),
    }
}

/// Palette slots handed to lists the first time ctui sees them, so colours
/// don't shuffle when lists come and go.
#[derive(Debug, Default)]
pub struct Slots {
    path: Option<PathBuf>,
    pub map: BTreeMap<String, usize>,
}

impl Slots {
    pub fn load(state_dir: &Path) -> Self {
        let path = state_dir.join("list-colors.toml");
        let map = fs::read_to_string(&path).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default();
        Self { path: Some(path), map }
    }

    /// Gives every list without a slot the lowest free one; saves if any changed.
    pub fn assign(&mut self, lists: &[List]) {
        let mut changed = false;
        for l in lists {
            if self.map.contains_key(&l.id) {
                continue;
            }
            let used: Vec<usize> = self.map.values().copied().collect();
            let slot = (0..).find(|s| !used.contains(s)).expect("a free slot");
            self.map.insert(l.id.clone(), slot);
            changed = true;
        }
        if changed {
            if let Some(p) = &self.path {
                if let Some(d) = p.parent() {
                    let _ = fs::create_dir_all(d);
                }
                let _ = fs::write(p, toml::to_string(&self.map).unwrap_or_default());
            }
        }
    }
}

/// A list's colour: its `color` file, a pinned slot, then its remembered slot.
pub fn list_color(list: &List, theme: &Theme, cfg: &Config, slots: &Slots) -> Color {
    if cfg.list_colors == ListColors::File {
        if let Some(c) = list.color.as_deref().and_then(|c| Color::parse(c).ok()) {
            return c;
        }
    }
    let slot = cfg.list_slots.get(&list.id).or_else(|| slots.map.get(&list.id)).copied().unwrap_or(0);
    theme.lists[slot % theme.lists.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_theme_parses() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("themes");
        let mut n = 0;
        for e in fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "toml") {
                let t = Theme::parse(&fs::read_to_string(&p).unwrap(), "x").unwrap_or_else(|e| panic!("{}: {e}", p.display()));
                assert!(!t.lists.is_empty());
                n += 1;
            }
        }
        assert_eq!(n, 39);
        for (name, text) in BUILTIN {
            assert_eq!(Theme::parse(text, name).unwrap().name, *name);
        }
    }

    #[test]
    fn colours_and_derivation() {
        assert_eq!(Color::parse("#abc").unwrap(), Color::Rgb(0xaa, 0xbb, 0xcc));
        assert_eq!(Color::parse("bright-black").unwrap(), Color::Ansi(8));
        assert_eq!(Color::parse("reset").unwrap(), Color::Reset);
        assert!(Color::parse("chartreuse").is_err());
        let d = Theme::fallback();
        assert_eq!(d.select, None, "ANSI themes can't blend");
        let nord = Theme::parse(BUILTIN.iter().find(|(n, _)| *n == "nord").unwrap().1, "nord").unwrap();
        assert!(nord.select.is_some());
    }

    #[test]
    fn slots_are_stable() {
        let mk = |id: &str| List { id: id.into(), name: id.into(), color: None };
        let mut s = Slots::default();
        s.assign(&[mk("b"), mk("a")]);
        s.assign(&[mk("a"), mk("c"), mk("b")]);
        assert_eq!((s.map["b"], s.map["a"], s.map["c"]), (0, 1, 2));
        let theme = Theme::fallback();
        let mut cfg = Config::default();
        assert_eq!(list_color(&mk("a"), &theme, &cfg, &s), theme.lists[1]);
        cfg.list_slots.insert("a".into(), 3);
        assert_eq!(list_color(&mk("a"), &theme, &cfg, &s), theme.lists[3]);
        let mut colored = mk("a");
        colored.color = Some("#ff0000".into());
        assert_eq!(list_color(&colored, &theme, &cfg, &s), Color::Rgb(255, 0, 0));
        cfg.list_colors = ListColors::Theme;
        assert_eq!(list_color(&colored, &theme, &cfg, &s), theme.lists[3]);
    }
}
