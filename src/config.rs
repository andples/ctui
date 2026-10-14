//! Settings (`config.toml`) and where files live.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::Weekday;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum View {
    #[default]
    Agenda,
    Month,
    Tasks,
}

impl View {
    pub const ALL: [View; 3] = [View::Agenda, View::Month, View::Tasks];

    pub fn name(self) -> &'static str {
        match self {
            View::Agenda => "agenda",
            View::Month => "month",
            View::Tasks => "tasks",
        }
    }
}

/// How typed numeric dates like `20/10` are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DateOrder {
    #[default]
    Dmy,
    Mdy,
    Ymd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TimeFormat {
    #[default]
    #[serde(rename = "24h")]
    H24,
    #[serde(rename = "12h")]
    H12,
}

/// Where list colours come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ListColors {
    /// A list's `color` file wins, then the theme.
    #[default]
    File,
    /// Always the theme's palette.
    Theme,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: String,
    pub default_view: View,
    pub views: Vec<View>,
    #[serde(with = "weekday_serde")]
    pub week_start: Weekday,
    pub date_order: DateOrder,
    pub time_format: TimeFormat,
    pub agenda_days: u32,
    /// How long done tasks stay visible: `0`, `12h`, `1d`, `7d`, `never`.
    pub hide_done_after: String,
    /// Overdue tasks shown before "+N more".
    pub overdue_collapse: usize,
    pub default_list: String,
    pub mouse: bool,
    pub data_dir: Option<String>,
    pub list_colors: ListColors,
    /// Pinned palette slots per list (`ct lists color work 3`).
    pub list_slots: BTreeMap<String, usize>,
    /// Reminder added to new items per list, like `{ work = "10m" }`.
    pub default_reminder: BTreeMap<String, String>,
    /// Action name → keys, replacing that action's defaults.
    pub keys: BTreeMap<String, Vec<String>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: "default".into(),
            default_view: View::Agenda,
            views: View::ALL.to_vec(),
            week_start: Weekday::Mon,
            date_order: DateOrder::Dmy,
            time_format: TimeFormat::H24,
            agenda_days: 7,
            hide_done_after: "1d".into(),
            overdue_collapse: 5,
            default_list: "personal".into(),
            mouse: true,
            data_dir: None,
            list_colors: ListColors::File,
            list_slots: BTreeMap::new(),
            default_reminder: BTreeMap::new(),
            keys: BTreeMap::new(),
        }
    }
}

mod weekday_serde {
    use chrono::Weekday;
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(d: &Weekday, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(super::weekday_name(*d))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Weekday, D::Error> {
        let s = String::deserialize(d)?;
        super::parse_weekday(&s).ok_or_else(|| D::Error::custom(format!("unknown weekday {s:?}")))
    }
}

pub fn weekday_name(d: Weekday) -> &'static str {
    match d {
        Weekday::Mon => "monday",
        Weekday::Tue => "tuesday",
        Weekday::Wed => "wednesday",
        Weekday::Thu => "thursday",
        Weekday::Fri => "friday",
        Weekday::Sat => "saturday",
        Weekday::Sun => "sunday",
    }
}

pub fn parse_weekday(s: &str) -> Option<Weekday> {
    let s = s.to_ascii_lowercase();
    let all = [
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
        Weekday::Fri,
        Weekday::Sat,
        Weekday::Sun,
    ];
    all.into_iter().find(|d| s.len() >= 2 && weekday_name(*d).starts_with(&s))
}

impl Config {
    /// Reads the config, or the defaults when there's no file. A broken file
    /// is an error with the reason, so the user can fix it.
    pub fn load(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("reading {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Sets one setting from text (`:set week_start sunday`) and saves just
    /// that key, leaving the rest of the file's keys alone.
    pub fn set(&mut self, path: &Path, key: &str, value: &str) -> Result<()> {
        let mut table: toml::Table = match fs::read_to_string(path) {
            Ok(t) => toml::from_str(&t).with_context(|| format!("reading {}", path.display()))?,
            Err(_) => toml::Table::new(),
        };
        let parsed = parse_value(key, value)?;
        let mut probe = toml::Table::try_from(&*self)?;
        probe.insert(key.to_string(), parsed.clone());
        let updated: Config = toml::Value::Table(probe)
            .try_into()
            .map_err(|e: toml::de::Error| anyhow::anyhow!("{key}: {}", e.message()))?;
        table.insert(key.to_string(), parsed);
        write_table(path, &table)?;
        *self = updated;
        Ok(())
    }

    /// Writes a whole sub-table entry, like `list_slots.work = 3`.
    pub fn set_in_table(&mut self, path: &Path, table_key: &str, key: &str, value: Option<toml::Value>) -> Result<()> {
        let mut table: toml::Table = match fs::read_to_string(path) {
            Ok(t) => toml::from_str(&t)?,
            Err(_) => toml::Table::new(),
        };
        let entry = table.entry(table_key.to_string()).or_insert_with(|| toml::Value::Table(toml::Table::new()));
        let Some(inner) = entry.as_table_mut() else { bail!("{table_key} is not a table") };
        match value {
            Some(v) => {
                inner.insert(key.to_string(), v);
            }
            None => {
                inner.remove(key);
            }
        }
        let updated: Config = toml::Value::Table(table.clone()).try_into()?;
        write_table(path, &table)?;
        *self = updated;
        Ok(())
    }

    /// Done items stop showing after this long; `None` = never hidden.
    pub fn hide_done_after(&self) -> Option<chrono::Duration> {
        let s = self.hide_done_after.trim();
        if s == "never" {
            return None;
        }
        if s == "0" {
            return Some(chrono::Duration::zero());
        }
        Some(crate::core::parse::parse_length(s).unwrap_or(chrono::Duration::days(1)))
    }

    pub fn views(&self) -> Vec<View> {
        let v: Vec<View> = View::ALL.into_iter().filter(|v| self.views.contains(v)).collect();
        if v.is_empty() { vec![View::Agenda] } else { v }
    }
}

/// The settings `:set` knows, with a word on each (for completion and help).
pub const SETTINGS: &[(&str, &str)] = &[
    ("theme", "colour theme"),
    ("default_view", "agenda | month | tasks"),
    ("views", "views to show, e.g. agenda,tasks"),
    ("week_start", "monday … sunday"),
    ("date_order", "dmy | mdy | ymd"),
    ("time_format", "24h | 12h"),
    ("agenda_days", "days the agenda shows"),
    ("hide_done_after", "0 | 12h | 1d | 7d | never"),
    ("overdue_collapse", "overdue tasks before +N more"),
    ("default_list", "list for new items"),
    ("mouse", "true | false"),
    ("data_dir", "where the lists live"),
    ("list_colors", "file | theme"),
];

fn parse_value(key: &str, value: &str) -> Result<toml::Value> {
    let value = value.trim();
    Ok(match key {
        "agenda_days" | "overdue_collapse" => {
            toml::Value::Integer(value.parse().with_context(|| format!("{key} takes a number"))?)
        }
        "mouse" => toml::Value::Boolean(match value {
            "true" | "on" | "yes" => true,
            "false" | "off" | "no" => false,
            _ => bail!("mouse takes true or false"),
        }),
        "views" => toml::Value::Array(
            value.split([',', ' ']).filter(|s| !s.is_empty()).map(|s| toml::Value::String(s.into())).collect(),
        ),
        "week_start" => toml::Value::String(
            weekday_name(parse_weekday(value).with_context(|| format!("unknown weekday {value:?}"))?).into(),
        ),
        k if SETTINGS.iter().any(|(n, _)| *n == k) => toml::Value::String(value.into()),
        _ => bail!("unknown setting {key:?}"),
    })
}

fn write_table(path: &Path, table: &toml::Table) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let text = toml::to_string_pretty(table)?;
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Where ctui keeps things. Every path can be overridden from the command line.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub state_dir: PathBuf,
}

impl Paths {
    pub fn resolve(config_dir: Option<PathBuf>, data_dir: Option<PathBuf>, state_dir: Option<PathBuf>) -> Result<(Self, Config)> {
        let dirs = directories::ProjectDirs::from("", "", "ctui");
        let config_dir = match config_dir {
            Some(d) => d,
            None => dirs.as_ref().context("no home directory")?.config_dir().to_path_buf(),
        };
        let config = Config::load(&config_dir.join("config.toml"))?;
        let data_dir = match (data_dir, &config.data_dir) {
            (Some(d), _) => d,
            (None, Some(d)) => expand_tilde(d),
            (None, None) => dirs.as_ref().context("no home directory")?.data_dir().to_path_buf(),
        };
        let state_dir = match state_dir {
            Some(d) => d,
            None => {
                let d = dirs.as_ref().context("no home directory")?;
                d.state_dir().map(Path::to_path_buf).unwrap_or_else(|| d.data_local_dir().join("state"))
            }
        };
        Ok((Self { config_dir, data_dir, state_dir }, config))
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn themes_dir(&self) -> PathBuf {
        self.config_dir.join("themes")
    }
}

pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = directories::BaseDirs::new() {
            return home.home_dir().join(rest);
        }
    }
    PathBuf::from(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_partial_files() {
        let c: Config = toml::from_str("week_start = \"sunday\"\ntime_format = \"12h\"\n").unwrap();
        assert_eq!(c.week_start, Weekday::Sun);
        assert_eq!(c.time_format, TimeFormat::H12);
        assert_eq!(c.agenda_days, 7);
    }

    #[test]
    fn set_writes_one_key() {
        let dir = std::env::temp_dir().join(format!("ctui-cfg-{}", std::process::id()));
        let path = dir.join("config.toml");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(&path, "theme = \"nord\"\n[keys]\ndone = [\"x\"]\n").unwrap();
        let mut c = Config::load(&path).unwrap();
        c.set(&path, "week_start", "sun").unwrap();
        assert_eq!(c.week_start, Weekday::Sun);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("theme = \"nord\""));
        assert!(text.contains("week_start = \"sunday\""));
        assert!(text.contains("done = [\"x\"]"));
        assert!(c.set(&path, "time_format", "13h").is_err());
        assert!(c.set(&path, "nope", "1").is_err());
        assert_eq!(Config::load(&path).unwrap().time_format, TimeFormat::H24);
        fs::remove_dir_all(&dir).unwrap();
    }
}
