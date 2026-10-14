//! End-to-end: the real binary against temp dirs.

use std::path::PathBuf;
use std::process::{Command, Output};

struct Sandbox(PathBuf);

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let d = std::env::temp_dir().join(format!("ctui-e2e-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Sandbox(d)
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_ctui"))
            .arg("--config-dir")
            .arg(self.0.join("c"))
            .arg("--data-dir")
            .arg(self.0.join("d"))
            .arg("--state-dir")
            .arg(self.0.join("s"))
            .args(args)
            .env("CTUI_NOW", "2026-10-15T12:00:00+01:00")
            .env("TZ", "Europe/London")
            .env("NO_COLOR", "1")
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(
            o.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8(o.stdout).unwrap()
    }
}

#[test]
fn add_list_done_undo() {
    let s = Sandbox::new("basic");
    assert_eq!(
        s.ok(&["add", "pay", "rent", "today", "!"]).trim(),
        "added task pay rent · due Thu 15 Oct · #personal"
    );
    s.ok(&["add", "buy milk"]);
    s.ok(&["add", "dentist sat 2-3pm @health!"]);
    let ls = s.ok(&["ls"]);
    assert!(ls.contains("  1  ☐ pay rent"), "{ls}");
    assert!(ls.contains("  2  ☐ buy milk"), "{ls}");
    assert_eq!(s.ok(&["done", "2"]).trim(), "done · buy milk");
    assert!(!s.ok(&["ls"]).contains("buy milk"));
    assert_eq!(s.ok(&["undo"]).trim(), "undid done buy milk");
    assert!(s.ok(&["ls"]).contains("buy milk"));
    let ag = s.ok(&["ag"]);
    assert!(
        ag.contains("Today · Thu 15") && ag.contains("Sat 17") && ag.contains("dentist"),
        "{ag}"
    );
    let json: serde_json::Value = serde_json::from_str(&s.ok(&["ls", "--json"])).unwrap();
    assert_eq!(json.as_array().unwrap().len(), 2);
    assert!(s.ok(&["cal"]).contains("October 2026"));
    assert!(s.ok(&["lists"]).contains("#health"));
}

#[test]
fn exit_codes() {
    let s = Sandbox::new("codes");
    s.ok(&["add", "pay rent"]);
    s.ok(&["add", "pay bills"]);
    assert_eq!(s.run(&["done", "pay"]).status.code(), Some(2), "ambiguous");
    assert_eq!(s.run(&["done", "nothing-like-this"]).status.code(), Some(1));
    assert_eq!(s.run(&["add", "x @nope"]).status.code(), Some(1));
    assert!(s.run(&["done", "bills"]).status.success());
}

#[test]
fn import_export_round_trip() {
    let s = Sandbox::new("import");
    let ics = s.0.join("cal.ics");
    std::fs::write(
        &ics,
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nX-WR-CALNAME:Family\r\nBEGIN:VEVENT\r\nUID:a@x\r\nDTSTART;VALUE=DATE:20261017\r\nSUMMARY:Picnic\r\nX-KEEP:yes\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    )
    .unwrap();
    let dry = s.ok(&["import", ics.to_str().unwrap(), "--dry-run"]);
    assert!(dry.starts_with("would import: 1 new"), "{dry}");
    assert!(s.ok(&["lists"]).contains("no lists"));
    assert!(
        s.ok(&["import", ics.to_str().unwrap()])
            .starts_with("imported: 1 new")
    );
    assert!(
        s.ok(&["import", ics.to_str().unwrap()])
            .contains("0 new, 0 updated, 1 unchanged")
    );
    let out = s.ok(&["export"]);
    assert!(
        out.contains("SUMMARY:Picnic") && out.contains("X-KEEP:yes"),
        "{out}"
    );
    s.ok(&["edit", "picnic", "Picnic in the park sat allday"]);
    let out = s.ok(&["export", "family"]);
    assert!(
        out.contains("X-KEEP:yes"),
        "unknown properties survive an edit"
    );
    assert!(out.contains("X-WR-CALNAME:Family"));
}

#[test]
fn settings_and_themes() {
    let s = Sandbox::new("set");
    assert!(
        s.ok(&["set", "time_format", "12h"])
            .contains("time_format · 12h")
    );
    s.ok(&["add", "call 3pm"]);
    assert!(s.ok(&["ls", "today"]).contains("3pm"));
    assert!(s.run(&["set", "time_format", "13h"]).status.code() == Some(1));
    assert!(s.ok(&["theme"]).contains("nord"));
    assert_eq!(s.ok(&["theme", "nor"]).trim(), "theme · nord");
    let cfg = std::fs::read_to_string(s.0.join("c/config.toml")).unwrap();
    assert!(cfg.contains("theme = \"nord\"") && cfg.contains("time_format = \"12h\""));
}

#[test]
fn piped_without_args_prints_the_agenda() {
    let s = Sandbox::new("pipe");
    s.ok(&["add", "standup today 9:00-9:15"]);
    assert!(s.ok(&[]).contains("standup"));
}

#[test]
fn remind_finds_due_alarms() {
    let s = Sandbox::new("remind");
    s.ok(&["add", "dentist today 12:30-13:00 remind 1h"]);
    let out = s.ok(&["remind", "--no-notify"]);
    // First run looks back a minute: 11:30 is long gone.
    assert!(out.is_empty(), "{out}");
}
