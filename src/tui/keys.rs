//! Actions, their default keys, and `[keys]` remapping from the config.
//! The help screen and the hint line are generated from this table.

use std::collections::{BTreeMap, HashMap};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    Down,
    Up,
    Left,
    Right,
    PageDown,
    PageUp,
    Top,
    Bottom,
    Agenda,
    Month,
    Tasks,
    NextView,
    PrevView,
    Add,
    AddEvent,
    Done,
    Toggle,
    Edit,
    EditRaw,
    Delete,
    Undo,
    Redo,
    Move,
    Later,
    Earlier,
    Indent,
    Outdent,
    ItemDown,
    ItemUp,
    TabNext,
    TabPrev,
    Today,
    PrevPeriod,
    NextPeriod,
    Goto,
    Filter,
    Command,
    Someday,
    Sort,
    Theme,
    Help,
    Quit,
}

/// (action, config name, default keys, what it does)
pub const TABLE: &[(Action, &str, &[&str], &str)] = &[
    (Action::Down, "down", &["j", "down"], "move down"),
    (Action::Up, "up", &["k", "up"], "move up"),
    (Action::Left, "left", &["h", "left"], "previous day / list"),
    (Action::Right, "right", &["l", "right"], "next day / list"),
    (
        Action::PageDown,
        "page_down",
        &["ctrl+d", "pagedown"],
        "half a page down",
    ),
    (
        Action::PageUp,
        "page_up",
        &["ctrl+u", "pageup"],
        "half a page up",
    ),
    (Action::Top, "top", &["home"], "first item"),
    (Action::Bottom, "bottom", &["G", "end"], "last item"),
    (Action::Agenda, "agenda", &["1"], "agenda view"),
    (Action::Month, "month", &["2"], "month view"),
    (Action::Tasks, "tasks", &["3"], "tasks view"),
    (Action::NextView, "next_view", &["tab"], "next view"),
    (Action::PrevView, "prev_view", &["backtab"], "previous view"),
    (Action::Add, "add", &["a"], "add (quick-add text)"),
    (Action::AddEvent, "add_event", &["A"], "add an event"),
    (Action::Done, "done", &["x"], "tick off / un-tick"),
    (
        Action::Toggle,
        "toggle",
        &["space"],
        "tick off, or open/close subtasks",
    ),
    (Action::Edit, "edit", &["e", "enter"], "edit as text"),
    (
        Action::EditRaw,
        "edit_raw",
        &["E"],
        "edit the .ics in $EDITOR",
    ),
    (
        Action::Delete,
        "delete",
        &["d", "delete"],
        "delete (u brings it back)",
    ),
    (Action::Undo, "undo", &["u"], "undo"),
    (Action::Redo, "redo", &["ctrl+r", "U"], "redo"),
    (Action::Move, "move", &["m"], "move to a date, time or list"),
    (Action::Later, "later", &["+", "="], "one day later"),
    (Action::Earlier, "earlier", &["-"], "one day earlier"),
    (
        Action::Indent,
        "indent",
        &[">"],
        "make a subtask of the task above",
    ),
    (Action::Outdent, "outdent", &["<"], "make top-level again"),
    (
        Action::ItemDown,
        "item_down",
        &["J"],
        "move item down (manual order)",
    ),
    (
        Action::ItemUp,
        "item_up",
        &["K"],
        "move item up (manual order)",
    ),
    (Action::TabNext, "tab_next", &["L"], "next list"),
    (Action::TabPrev, "tab_prev", &["H"], "previous list"),
    (Action::Today, "today", &["t"], "jump to today"),
    (
        Action::PrevPeriod,
        "prev_period",
        &["["],
        "previous week / month",
    ),
    (
        Action::NextPeriod,
        "next_period",
        &["]"],
        "next week / month",
    ),
    (Action::Goto, "goto", &["g"], "go to a date"),
    (Action::Filter, "filter", &["/"], "filter"),
    (
        Action::Command,
        "command",
        &[":"],
        "command line (any ct command)",
    ),
    (
        Action::Someday,
        "someday",
        &["T"],
        "show undated tasks in the agenda",
    ),
    (
        Action::Sort,
        "sort",
        &["s"],
        "sort: due / priority / manual",
    ),
    (Action::Theme, "theme", &["c"], "pick a colour theme"),
    (Action::Help, "help", &["?"], "help"),
    (Action::Quit, "quit", &["q", "ctrl+c"], "quit"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: KeyCode,
    pub ctrl: bool,
    pub alt: bool,
}

impl Key {
    pub fn from_event(e: &KeyEvent) -> Self {
        let mut code = e.code;
        // Shift is folded into the character itself ("A", "+", "?").
        if let KeyCode::Char(c) = code {
            code = KeyCode::Char(c);
        }
        Key {
            code,
            ctrl: e.modifiers.contains(KeyModifiers::CONTROL),
            alt: e.modifiers.contains(KeyModifiers::ALT),
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let mut ctrl = false;
        let mut alt = false;
        let mut rest = s;
        loop {
            let l = rest.to_ascii_lowercase();
            if l.starts_with("ctrl+") || l.starts_with("ctrl-") {
                ctrl = true;
                rest = &rest[5..];
            } else if l.starts_with("alt+") || l.starts_with("alt-") {
                alt = true;
                rest = &rest[4..];
            } else {
                break;
            }
        }
        let code = match rest.to_ascii_lowercase().as_str() {
            "space" => KeyCode::Char(' '),
            "enter" | "return" => KeyCode::Enter,
            "tab" => KeyCode::Tab,
            "backtab" | "shift+tab" => KeyCode::BackTab,
            "esc" | "escape" => KeyCode::Esc,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" => KeyCode::PageDown,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "delete" | "del" => KeyCode::Delete,
            "backspace" => KeyCode::Backspace,
            f if f.starts_with('f') && f.len() > 1 && f[1..].parse::<u8>().is_ok() => {
                KeyCode::F(f[1..].parse().ok()?)
            }
            _ => {
                let mut chars = rest.chars();
                let c = chars.next()?;
                if chars.next().is_some() {
                    return None;
                }
                KeyCode::Char(if ctrl { c.to_ascii_lowercase() } else { c })
            }
        };
        Some(Key { code, ctrl, alt })
    }

    pub fn label(&self) -> String {
        let base = match self.code {
            KeyCode::Char(' ') => "space".to_string(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Enter => "enter".into(),
            KeyCode::Tab => "tab".into(),
            KeyCode::BackTab => "shift+tab".into(),
            KeyCode::Esc => "esc".into(),
            KeyCode::Up => "↑".into(),
            KeyCode::Down => "↓".into(),
            KeyCode::Left => "←".into(),
            KeyCode::Right => "→".into(),
            KeyCode::PageUp => "pgup".into(),
            KeyCode::PageDown => "pgdn".into(),
            KeyCode::Home => "home".into(),
            KeyCode::End => "end".into(),
            KeyCode::Delete => "del".into(),
            KeyCode::Backspace => "backspace".into(),
            KeyCode::F(n) => format!("f{n}"),
            _ => "?".into(),
        };
        match (self.ctrl, self.alt) {
            (true, _) => format!("ctrl+{base}"),
            (_, true) => format!("alt+{base}"),
            _ => base,
        }
    }
}

pub struct Keymap {
    by_key: HashMap<Key, Action>,
    by_action: BTreeMap<Action, Vec<Key>>,
    /// Config entries that didn't make sense, to report once.
    pub problems: Vec<String>,
}

impl Keymap {
    pub fn new(overrides: &BTreeMap<String, Vec<String>>) -> Self {
        let mut by_action: BTreeMap<Action, Vec<Key>> = BTreeMap::new();
        let mut problems = Vec::new();
        for (a, _, keys, _) in TABLE {
            by_action.insert(*a, keys.iter().filter_map(|k| Key::parse(k)).collect());
        }
        for (name, keys) in overrides {
            match TABLE.iter().find(|(_, n, _, _)| n == name) {
                Some((a, ..)) => {
                    let parsed: Vec<Key> = keys
                        .iter()
                        .filter_map(|k| {
                            let p = Key::parse(k);
                            if p.is_none() {
                                problems.push(format!("[keys] {name}: can't read key {k:?}"));
                            }
                            p
                        })
                        .collect();
                    // A key taken by this action is freed from the others.
                    for k in &parsed {
                        for v in by_action.values_mut() {
                            v.retain(|x| x != k);
                        }
                    }
                    by_action.insert(*a, parsed);
                }
                None => problems.push(format!("[keys] no action called {name:?}")),
            }
        }
        let mut by_key = HashMap::new();
        for (a, keys) in &by_action {
            for k in keys {
                by_key.insert(*k, *a);
            }
        }
        // ctrl+c always quits, whatever the config says.
        by_key.insert(
            Key {
                code: KeyCode::Char('c'),
                ctrl: true,
                alt: false,
            },
            Action::Quit,
        );
        Self {
            by_key,
            by_action,
            problems,
        }
    }

    pub fn action(&self, e: &KeyEvent) -> Option<Action> {
        let k = Key::from_event(e);
        self.by_key.get(&k).copied().or_else(|| {
            // Terminals report shifted letters both ways; accept either.
            if let KeyCode::Char(c) = k.code
                && e.modifiers.contains(KeyModifiers::SHIFT)
                && c.is_ascii_lowercase()
            {
                return self
                    .by_key
                    .get(&Key {
                        code: KeyCode::Char(c.to_ascii_uppercase()),
                        ..k
                    })
                    .copied();
            }
            None
        })
    }

    /// The first key for an action, as shown in hints.
    pub fn first(&self, a: Action) -> Option<String> {
        self.by_action
            .get(&a)
            .and_then(|v| v.first())
            .map(Key::label)
    }

    pub fn all(&self, a: Action) -> Vec<String> {
        self.by_action
            .get(&a)
            .map(|v| v.iter().map(Key::label).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn defaults_and_overrides() {
        let km = Keymap::new(&BTreeMap::new());
        assert_eq!(km.action(&ev('x')), Some(Action::Done));
        assert_eq!(km.action(&ev('A')), Some(Action::AddEvent));
        let mut o = BTreeMap::new();
        o.insert("done".to_string(), vec!["c".to_string()]);
        o.insert("bogus".to_string(), vec!["z".to_string()]);
        let km = Keymap::new(&o);
        assert_eq!(
            km.action(&ev('c')),
            Some(Action::Done),
            "c moves from theme to done"
        );
        assert_eq!(km.action(&ev('x')), None);
        assert_eq!(km.problems.len(), 1);
        assert_eq!(km.first(Action::Done).as_deref(), Some("c"));
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(km.action(&ctrl_c), Some(Action::Quit));
    }

    #[test]
    fn parses_names() {
        assert_eq!(Key::parse("ctrl+r").unwrap().label(), "ctrl+r");
        assert_eq!(Key::parse("space").unwrap().code, KeyCode::Char(' '));
        assert_eq!(Key::parse("f5").unwrap().code, KeyCode::F(5));
        assert!(Key::parse("nope").is_none());
    }
}
