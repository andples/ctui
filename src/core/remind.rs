//! Reminders without a daemon: `ct remind` finds alarms that fell due since
//! its last run, and remembers where it got to.

use std::fs;
use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;

use super::model::Kind;
use super::recur;
use super::store::Store;

#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub title: String,
    pub list: String,
    /// When the event starts or the task is due.
    pub at: DateTime<Tz>,
    /// When the reminder fires.
    pub fires: DateTime<Tz>,
    pub kind: Kind,
}

/// Alarms firing in `(from, to]`.
pub fn between(store: &Store, from: DateTime<Tz>, to: DateTime<Tz>, tz: Tz) -> Vec<Alert> {
    let mut out = Vec::new();
    for item in &store.items {
        match item.kind {
            Kind::Event => {
                // Alarms can be up to a week before; look that far ahead.
                let first = (from - Duration::days(1)).date_naive();
                let last = (to + Duration::days(8)).date_naive();
                for occ in recur::occurrences(item, first, last, tz) {
                    let at = occ.start.instant(tz);
                    for off in &occ.f.alarms {
                        let fires = at - *off;
                        if fires > from && fires <= to {
                            out.push(Alert {
                                title: occ.f.title.clone(),
                                list: item.list.clone(),
                                at,
                                fires,
                                kind: Kind::Event,
                            });
                        }
                    }
                }
            }
            Kind::Task => {
                if item.f.done {
                    continue;
                }
                let Some(due) = item.f.due else { continue };
                let at = if due.is_date() {
                    // An all-day due date reminds relative to 9:00 that day.
                    super::time::resolve(tz, due.date(tz).and_hms_opt(9, 0, 0).expect("valid"))
                } else {
                    due.instant(tz)
                };
                for off in &item.f.alarms {
                    let fires = at - *off;
                    if fires > from && fires <= to {
                        out.push(Alert {
                            title: item.f.title.clone(),
                            list: item.list.clone(),
                            at,
                            fires,
                            kind: Kind::Task,
                        });
                    }
                }
            }
        }
    }
    out.sort_by_key(|a| (a.fires, a.at));
    out
}

/// When `ct remind` last ran (a minute ago the first time, so a new setup
/// doesn't replay old alarms).
pub fn last_run(state_dir: &Path, now: DateTime<Tz>) -> DateTime<Tz> {
    fs::read_to_string(state_dir.join("remind-last"))
        .ok()
        .and_then(|s| DateTime::parse_from_rfc3339(s.trim()).ok())
        .map(|d| d.with_timezone(&now.timezone()))
        // Never look back more than a day (a laptop that slept for a week).
        .map(|d| d.max(now - Duration::days(1)))
        .unwrap_or(now - Duration::minutes(1))
}

pub fn save_run(state_dir: &Path, now: DateTime<Tz>) -> anyhow::Result<()> {
    fs::create_dir_all(state_dir)?;
    fs::write(
        state_dir.join("remind-last"),
        now.with_timezone(&Utc).to_rfc3339(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::core::ops::{self, Defaults, Env};
    use crate::core::undo::Journal;
    use chrono::TimeZone;

    #[test]
    fn finds_alarms_in_window() {
        let dir = std::env::temp_dir().join(format!("ctui-remind-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut store = Store::open(&dir.join("data"), None).unwrap();
        let mut j = Journal::load(&dir.join("state"));
        let cfg = Config::default();
        let tz = Tz::Europe__London;
        let now = tz.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap();
        let env = Env { now, tz, cfg: &cfg };
        ops::add(
            &mut store,
            &mut j,
            &env,
            "dentist today 14:00-15:00 remind 1h",
            None,
            &Defaults::default(),
        )
        .unwrap();
        ops::add(
            &mut store,
            &mut j,
            &env,
            "call today 13:30 remind 30m",
            None,
            &Defaults::default(),
        )
        .unwrap();
        ops::add(
            &mut store,
            &mut j,
            &env,
            "standup every day 9:00-9:15 remind 10m",
            None,
            &Defaults::default(),
        )
        .unwrap();
        let a = between(&store, now, now + Duration::hours(1), tz);
        let titles: Vec<&str> = a.iter().map(|x| x.title.as_str()).collect();
        assert_eq!(titles, vec!["call", "dentist"]);
        let next_morning = between(
            &store,
            now + Duration::hours(20),
            now + Duration::hours(21),
            tz,
        );
        assert_eq!(next_morning.len(), 1, "tomorrow's standup reminder at 8:50");
        assert!(last_run(&dir.join("state"), now) < now);
        save_run(&dir.join("state"), now).unwrap();
        assert_eq!(
            last_run(&dir.join("state"), now + Duration::minutes(5)),
            now
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
