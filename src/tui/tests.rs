//! TUI tests on ratatui's TestBackend: every view at several sizes, typing,
//! undo, the theme picker and mouse clicks.

use chrono::TimeZone;
use chrono_tz::Tz;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::app::{App, Mode};
use super::view;
use crate::config::{Config, Paths, View};
use crate::session::Session;

struct T {
    app: App,
    dir: std::path::PathBuf,
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn setup(name: &str) -> T {
    let dir = std::env::temp_dir().join(format!("ctui-tui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let paths = Paths {
        config_dir: dir.join("c"),
        data_dir: dir.join("d"),
        state_dir: dir.join("s"),
    };
    let mut s = Session::open(paths, Config::default()).unwrap();
    s.tz = Tz::Europe__London;
    s.fixed_now = Some(
        Tz::Europe__London
            .with_ymd_and_hms(2026, 10, 15, 12, 0, 0)
            .unwrap(),
    );
    for text in [
        "renew passport 2026-10-13",
        "pay rent fri !",
        "dentist sat 2-3pm #health!",
        "standup every weekday 9:30-9:45 #work!",
        "buy milk",
        "plan trip",
    ] {
        let (store, journal, env) = s.parts();
        crate::core::ops::add(store, journal, &env, text, None, &Default::default()).unwrap();
    }
    s.refresh_slots();
    T {
        app: App::new(s),
        dir,
    }
}

fn screen(app: &mut App, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| view::draw(f, app)).unwrap();
    let buf = term.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn key(app: &mut App, c: char) {
    app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
}

fn typed(app: &mut App, s: &str) {
    for c in s.chars() {
        key(app, c);
    }
}

fn enter(app: &mut App) {
    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
}

#[test]
fn every_view_draws_at_every_size() {
    let mut t = setup("sizes");
    for v in [View::Today, View::Month, View::Tasks] {
        t.app.view = v;
        t.app.rebuild();
        for (w, h) in [(40, 12), (60, 20), (80, 24), (120, 40), (200, 60), (20, 6)] {
            let s = screen(&mut t.app, w, h);
            assert!(s.contains("ctui"), "{v:?} {w}x{h}");
        }
    }
    t.app.view = View::Today;
    t.app.rebuild();
    let s = screen(&mut t.app, 100, 30);
    for want in [
        "Overdue",
        "Today · Thu 15",
        "renew passport",
        "standup",
        "pay rent",
        "#health",
    ] {
        assert!(s.contains(want), "missing {want}:\n{s}");
    }
}

#[test]
fn add_tick_undo() {
    let mut t = setup("add");
    let before = t.app.s.store.items.len();
    key(&mut t.app, 'a');
    typed(&mut t.app, "call bob tom 3pm");
    let s = screen(&mut t.app, 100, 30);
    assert!(
        s.contains("task · due Fri 16 Oct 15:00"),
        "live preview:\n{s}"
    );
    enter(&mut t.app);
    assert_eq!(t.app.s.store.items.len(), before + 1);
    assert_eq!(
        t.app.selected().unwrap().title,
        "call bob",
        "the new item is selected"
    );
    key(&mut t.app, 'x');
    let uid = t.app.selected().unwrap().uid.clone();
    assert!(t.app.s.store.get(&uid).unwrap().f.done);
    key(&mut t.app, 'u');
    assert!(!t.app.s.store.get(&uid).unwrap().f.done);
    // A bad list keeps the text open with the error.
    key(&mut t.app, 'a');
    typed(&mut t.app, "x #nope");
    enter(&mut t.app);
    assert!(matches!(t.app.mode, Mode::Input(_)));
    assert!(t.app.toast.as_ref().is_some_and(|(_, bad)| *bad));
}

#[test]
fn theme_picker_previews_and_restores() {
    let mut t = setup("theme");
    let original = t.app.s.theme.clone();
    key(&mut t.app, 'c');
    key(&mut t.app, 'j');
    assert_ne!(t.app.s.theme, original, "moving previews");
    t.app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(t.app.s.theme, original, "esc goes back");
    assert_eq!(t.app.s.cfg.theme, "default");
    key(&mut t.app, 'c');
    key(&mut t.app, 'j');
    enter(&mut t.app);
    assert_ne!(t.app.s.cfg.theme, "default", "enter keeps it");
}

#[test]
fn command_line_runs_ct_commands() {
    let mut t = setup("cmd");
    key(&mut t.app, ':');
    typed(&mut t.app, "set week_start sunday");
    enter(&mut t.app);
    assert_eq!(t.app.s.cfg.week_start, chrono::Weekday::Sun);
    key(&mut t.app, ':');
    typed(&mut t.app, "ls all");
    enter(&mut t.app);
    assert!(matches!(t.app.mode, Mode::Output { .. }));
    t.app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    key(&mut t.app, ':');
    typed(&mut t.app, "nonsense");
    enter(&mut t.app);
    assert!(matches!(t.app.mode, Mode::Output { .. } | Mode::Normal));
}

#[test]
fn opens_on_a_page_and_shortcut_names_switch() {
    let mut t = setup("start");
    t.app.start_on(View::Month);
    assert_eq!(t.app.view, View::Month);
    assert_eq!(t.app.month_day, t.app.today(), "month opens on today");
    let pane: Vec<_> = t.app.day_rows.iter().map(|e| e.day).collect();
    assert!(
        !pane.is_empty() && pane.iter().all(|d| *d == t.app.today()),
        "{pane:?}"
    );
    for (word, want) in [
        ("ctt", View::Today),
        ("cttasks", View::Tasks),
        ("ctm", View::Month),
        ("cttoday", View::Today),
        ("ctmonth", View::Month),
        ("agenda", View::Today),
    ] {
        t.app.command(word);
        assert_eq!(t.app.view, want, ":{word}");
    }
    let names = super::app::command_names();
    for n in ["ctt", "cttoday", "ctm", "ctmonth", "cttasks"] {
        assert!(names.iter().any(|c| c == n), "completes {n}");
    }
    assert!(!names.iter().any(|c| c == "init"));
    // A page turned off in the config stays off, with a toast.
    t.app.command("ctt");
    t.app.s.cfg.views = vec![View::Today, View::Tasks];
    t.app.start_on(View::Month);
    assert_eq!(t.app.view, View::Today);
    assert!(
        t.app
            .toast
            .as_ref()
            .is_some_and(|(m, _)| m.contains("turned off")),
        "{:?}",
        t.app.toast
    );
}

#[test]
fn mouse_ticks_and_switches() {
    let mut t = setup("mouse");
    t.app.view = View::Tasks;
    t.app.rebuild();
    screen(&mut t.app, 100, 30);
    let (rect, idx) = t
        .app
        .hits
        .iter()
        .find_map(|(r, h)| match h {
            super::app::Hit::Check(i) => Some((*r, *i)),
            _ => None,
        })
        .unwrap();
    let uid = t.app.rows[idx].entry().unwrap().uid.clone();
    t.app.mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x,
        row: rect.y,
        modifiers: KeyModifiers::NONE,
    });
    assert!(
        t.app.s.store.get(&uid).unwrap().f.done,
        "clicking the box ticks it"
    );
    let tab = t
        .app
        .hits
        .iter()
        .find_map(|(r, h)| matches!(h, super::app::Hit::View(View::Month)).then_some(*r))
        .unwrap();
    t.app.mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: tab.x,
        row: tab.y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(t.app.view, View::Month);
}

#[test]
fn month_navigation() {
    let mut t = setup("month");
    key(&mut t.app, '2');
    assert_eq!(t.app.view, View::Month);
    key(&mut t.app, 'l');
    key(&mut t.app, 'l');
    assert_eq!(t.app.month_day.to_string(), "2026-10-17");
    assert_eq!(t.app.selected().unwrap().title, "dentist");
    key(&mut t.app, ']');
    assert_eq!(t.app.month_day.to_string(), "2026-11-17");
    key(&mut t.app, 't');
    assert_eq!(t.app.month_day.to_string(), "2026-10-15");
    key(&mut t.app, 'g');
    typed(&mut t.app, "dec 24");
    enter(&mut t.app);
    assert_eq!(t.app.month_day.to_string(), "2026-12-24");
}

#[test]
fn subtasks_in_tasks_view() {
    let mut t = setup("sub");
    key(&mut t.app, '3');
    // Select "buy milk" and put it under the task above it.
    let i = t
        .app
        .rows
        .iter()
        .position(|r| r.entry().is_some_and(|e| e.title == "buy milk"))
        .unwrap();
    t.app.sel = i;
    key(&mut t.app, '>');
    let uid = t.app.selected().unwrap().uid.clone();
    assert!(t.app.s.store.get(&uid).unwrap().f.parent.is_some());
    key(&mut t.app, '<');
    assert!(t.app.s.store.get(&uid).unwrap().f.parent.is_none());
}

fn esc(app: &mut App) {
    app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
}

fn search(app: &mut App, q: &str) {
    key(app, '/');
    typed(app, q);
}

#[test]
fn search_shows_live_results_and_jumps_to_today() {
    let mut t = setup("search-today");
    key(&mut t.app, '3');
    key(&mut t.app, '/');
    let s = screen(&mut t.app, 100, 30);
    assert!(s.contains("type to find any task or event"), "hint:\n{s}");
    assert!(t.app.found.hits.is_empty(), "an empty query lists nothing");
    typed(&mut t.app, "dent");
    let s = screen(&mut t.app, 100, 30);
    assert!(s.contains("Search everything"), "{s}");
    assert!(
        s.contains("dentist") && s.contains("Sat 14:00–15:00"),
        "{s}"
    );
    assert!(s.contains("#health"), "{s}");
    assert!(!s.contains("buy milk"), "only matches:\n{s}");
    for (w, h) in [(40, 12), (60, 20), (20, 6)] {
        screen(&mut t.app, w, h);
    }
    enter(&mut t.app);
    assert!(matches!(t.app.mode, Mode::Normal));
    assert_eq!(t.app.view, View::Today);
    assert_eq!(
        t.app.agenda_from.map(|d| d.to_string()).as_deref(),
        Some("2026-10-17")
    );
    assert_eq!(t.app.selected().unwrap().title, "dentist");
}

#[test]
fn search_jumps_to_undated_tasks_and_subtasks() {
    let mut t = setup("search-tasks");
    search(&mut t.app, "milk");
    enter(&mut t.app);
    assert_eq!(t.app.view, View::Tasks);
    assert_eq!(t.app.tab, t.app.tabs().len() - 1, "the all tab");
    assert_eq!(t.app.selected().unwrap().title, "buy milk");
    // A subtask opens its parent.
    let milk = t.app.selected().unwrap().uid.clone();
    let trip = t
        .app
        .s
        .store
        .items
        .iter()
        .find(|i| i.f.title == "plan trip")
        .unwrap()
        .uid
        .clone();
    let (store, journal, env) = t.app.s.parts();
    crate::core::ops::set_parent(store, journal, &env, &trip, Some(&milk)).unwrap();
    t.app.expanded.clear();
    key(&mut t.app, '1');
    search(&mut t.app, "trip");
    enter(&mut t.app);
    assert_eq!(t.app.view, View::Tasks);
    assert!(t.app.expanded.contains(&milk));
    assert_eq!(t.app.selected().unwrap().title, "plan trip");
}

#[test]
fn search_picks_the_occurrence_and_moves_with_arrows() {
    let mut t = setup("search-recur");
    search(&mut t.app, "a");
    assert!(t.app.found.hits.len() > 2, "several results");
    let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
    t.app.key(ctrl('n'));
    t.app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(t.app.found.sel, 2);
    t.app.key(ctrl('p'));
    assert_eq!(t.app.found.sel, 1);
    esc(&mut t.app);
    // A repeating event jumps to its next occurrence, that exact one.
    search(&mut t.app, "standup");
    let hit = t.app.found.hits[0].clone();
    assert!(hit.entry.rid.is_some());
    enter(&mut t.app);
    assert_eq!(t.app.view, View::Today);
    let sel = t.app.selected().unwrap();
    assert_eq!((&sel.uid, sel.rid), (&hit.entry.uid, hit.entry.rid));
}

#[test]
fn search_esc_goes_back_and_hidden_pages_fall_back() {
    let mut t = setup("search-esc");
    key(&mut t.app, '2');
    key(&mut t.app, 'l');
    let day = t.app.month_day;
    search(&mut t.app, "rent");
    assert_eq!(t.app.found.hits.len(), 1);
    esc(&mut t.app);
    assert!(matches!(t.app.mode, Mode::Normal));
    assert_eq!((t.app.view, t.app.month_day), (View::Month, day));
    assert!(t.app.found.hits.is_empty());
    // With Today turned off, a dated item opens in Month on its day.
    t.app.s.cfg.views = vec![View::Month, View::Tasks];
    search(&mut t.app, "rent");
    enter(&mut t.app);
    assert_eq!(t.app.view, View::Month);
    assert_eq!(t.app.month_day.to_string(), "2026-10-16");
    assert_eq!(t.app.selected().unwrap().title, "pay rent");
}

#[test]
fn search_mouse_click_jumps() {
    let mut t = setup("search-mouse");
    search(&mut t.app, "pass");
    screen(&mut t.app, 100, 30);
    let rect = t
        .app
        .hits
        .iter()
        .find_map(|(r, h)| matches!(h, super::app::Hit::Result(0)).then_some(*r))
        .unwrap();
    t.app.mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x + 10,
        row: rect.y,
        modifiers: KeyModifiers::NONE,
    });
    assert!(matches!(t.app.mode, Mode::Normal));
    assert_eq!(t.app.view, View::Today);
    assert_eq!(t.app.selected().unwrap().title, "renew passport");
}

#[cfg(target_os = "macos")]
#[test]
fn follows_utheme_live() {
    use crate::theme::Color;
    use crate::utheme_client::testing::{generation, switch};
    use crate::utheme_client::{Paths, heartbeat};
    use std::time::{Duration, Instant};

    let mut t = setup("utheme");
    let uth = Paths::under(&t.dir.join("uth"));
    switch(&uth, &generation(&uth, "g1", "#111111"));
    t.app.utheme.paths = Some(uth.clone());
    let wait = |app: &mut App, done: &dyn Fn(&App) -> bool| {
        let start = Instant::now();
        while !done(app) {
            assert!(start.elapsed() < Duration::from_secs(3), "timed out");
            std::thread::sleep(Duration::from_millis(20));
            app.utheme_tick();
        }
    };
    let bg = |app: &App| app.s.theme.bg;

    // Opted in from the command line: the live palette at once.
    key(&mut t.app, ':');
    typed(&mut t.app, "set theme utheme");
    enter(&mut t.app);
    assert!(t.app.utheme_tick());
    assert_eq!(bg(&t.app), Color::Rgb(0x11, 0x11, 0x11));
    assert_eq!(t.app.s.theme.main, Color::Rgb(0x88, 0xc0, 0xd0));
    screen(&mut t.app, 100, 30);
    let beat = heartbeat::path(&uth, "ctui");
    assert!(std::fs::read_to_string(&beat).unwrap().contains("\"g1\""));
    // From here on only utheme changes: the config must stay byte for byte.
    let config_file = t.app.s.paths.config_file();
    let config = std::fs::read(&config_file).unwrap();

    switch(&uth, &generation(&uth, "g2", "#222222"));
    wait(&mut t.app, &|a| bg(a) == Color::Rgb(0x22, 0x22, 0x22));
    assert!(std::fs::read_to_string(&beat).unwrap().contains("\"g2\""));

    // The picker keeps its preview; esc lands on the newest utheme theme.
    t.app.mode = Mode::Normal;
    key(&mut t.app, 'c');
    key(&mut t.app, 'j');
    assert!(matches!(t.app.mode, Mode::Themes { .. }));
    let preview = t.app.s.theme.clone();
    switch(&uth, &generation(&uth, "g3", "#333333"));
    wait(&mut t.app, &|a| {
        a.s.live_theme.as_ref().map(|l| l.bg) == Some(Color::Rgb(0x33, 0x33, 0x33))
    });
    assert_eq!(t.app.s.theme, preview);
    t.app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(bg(&t.app), Color::Rgb(0x33, 0x33, 0x33));

    // Uninstalled: back to the default, heartbeat gone. Back: followed again.
    std::fs::remove_file(&uth.current).unwrap();
    wait(&mut t.app, &|a| a.s.live_theme.is_none());
    let fallback = crate::theme::Theme::fallback();
    assert_eq!(
        (t.app.s.theme.bg, t.app.s.theme.lists.clone()),
        (fallback.bg, fallback.lists)
    );
    assert!(!beat.exists());
    switch(&uth, &generation(&uth, "g1", "#111111"));
    wait(&mut t.app, &|a| bg(a) == Color::Rgb(0x11, 0x11, 0x11));
    assert_eq!(std::fs::read(&config_file).unwrap(), config);

    // Another theme: no watching, no heartbeat.
    key(&mut t.app, ':');
    typed(&mut t.app, "theme nord");
    enter(&mut t.app);
    t.app.utheme_tick();
    assert_eq!(t.app.s.cfg.theme, "nord");
    assert!(!beat.exists());
    assert_ne!(bg(&t.app), Color::Rgb(0x11, 0x11, 0x11));

    // Back by hand, with no themes/utheme.toml: accepted, followed again.
    assert!(!t.app.s.paths.themes_dir().join("utheme.toml").exists());
    t.app.mode = Mode::Normal;
    key(&mut t.app, ':');
    typed(&mut t.app, "theme utheme");
    enter(&mut t.app);
    assert_eq!(t.app.s.cfg.theme, "utheme");
    assert!(t.app.utheme_tick());
    assert_eq!(bg(&t.app), Color::Rgb(0x11, 0x11, 0x11));
    assert!(beat.exists());
    // The picker lists it too.
    t.app.mode = Mode::Normal;
    key(&mut t.app, 'c');
    assert!(
        matches!(&t.app.mode, Mode::Themes { names, .. } if names.iter().any(|n| n == "utheme"))
    );
}
