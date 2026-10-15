//! TUI state. Every change goes through [`App::dispatch`] (keys) or
//! [`App::input_key`] (typing); views in `view.rs` only draw.

use std::collections::HashSet;

use anyhow::Result;
use chrono::{Datelike, Duration, NaiveDate};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use super::keys::{Action, Keymap};
use crate::cli::{self, Cli, Out};
use crate::config::View;
use crate::core::model::Kind;
use crate::core::ops::{self, Defaults, Rid};
use crate::core::parse::{self, Draft};
use crate::core::query::{self, Entry, Opts, Sort};
use crate::core::time::{Moment, add_months, last_of_month, week_start_of};
use crate::session::Session;
use crate::theme;

#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Header {
        title: String,
        overdue: bool,
        today: bool,
    },
    Item {
        entry: Entry,
        depth: u8,
        has_children: bool,
        expanded: bool,
    },
    /// `+N more` under a collapsed overdue section.
    More(usize),
    Note(String),
}

impl Row {
    pub fn selectable(&self) -> bool {
        matches!(self, Row::Item { .. } | Row::More(_))
    }

    pub fn entry(&self) -> Option<&Entry> {
        match self {
            Row::Item { entry, .. } => Some(entry),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputKind {
    Add { event: bool },
    Edit { uid: String, rid: Rid },
    Move { uid: String, rid: Rid },
    Goto,
    Filter,
    Command,
}

#[derive(Debug, Clone)]
pub struct Input {
    pub kind: InputKind,
    pub text: String,
    /// Cursor position in chars.
    pub cursor: usize,
    /// Tab-completion candidates for the word at the cursor, and which one is shown.
    pub completions: Vec<String>,
    pub completion_idx: usize,
}

impl Input {
    fn new(kind: InputKind, text: String, cursor: usize) -> Self {
        Self {
            kind,
            text,
            cursor,
            completions: Vec::new(),
            completion_idx: 0,
        }
    }

    fn byte(&self, ci: usize) -> usize {
        self.text
            .char_indices()
            .nth(ci)
            .map(|(b, _)| b)
            .unwrap_or(self.text.len())
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }
}

/// An edit to a repeating event waits for "this one or all?".
#[derive(Debug, Clone, PartialEq)]
pub enum Pending {
    Edit { uid: String, rid: Moment },
    Move { uid: String, rid: Moment },
    Shift { uid: String, rid: Moment, days: i64 },
    Delete { uid: String, rid: Moment },
}

#[derive(Debug, Clone)]
pub enum Mode {
    Normal,
    Input(Input),
    Help {
        scroll: u16,
    },
    Themes {
        names: Vec<String>,
        idx: usize,
        original: String,
    },
    Choose(Pending),
    Output {
        lines: Vec<String>,
        scroll: u16,
    },
}

/// Where a mouse click lands.
#[derive(Debug, Clone, PartialEq)]
pub enum Hit {
    View(View),
    Row(usize),
    Check(usize),
    Day(NaiveDate),
    Tab(usize),
}

pub struct App {
    pub s: Session,
    pub keys: Keymap,
    pub view: View,
    pub mode: Mode,
    pub rows: Vec<Row>,
    pub sel: usize,
    pub scroll: usize,
    /// Agenda starts here (`None` = today).
    pub agenda_from: Option<NaiveDate>,
    pub someday: bool,
    pub all_overdue: bool,
    pub month_day: NaiveDate,
    /// Selected entry in the month view's day pane.
    pub day_sel: usize,
    pub day_rows: Vec<Entry>,
    /// Tasks view: index into `tabs()`.
    pub tab: usize,
    pub sort: Sort,
    pub expanded: HashSet<String>,
    pub filter: String,
    pub toast: Option<(String, bool)>,
    pub quit: bool,
    pub hits: Vec<(Rect, Hit)>,
    /// Lines the body had last frame (for paging).
    pub body_height: usize,
    pub history: Vec<String>,
    pub history_idx: Option<usize>,
    /// Set when the TUI must hand the terminal to $EDITOR.
    pub raw_edit: Option<String>,
}

impl App {
    pub fn new(s: Session) -> Self {
        let keys = Keymap::new(&s.cfg.keys);
        let today = s.now().date_naive();
        let mut toast = None;
        if let Some(w) = s
            .warnings
            .first()
            .cloned()
            .or_else(|| keys.problems.first().cloned())
        {
            toast = Some((w, true));
        }
        let view = if s.cfg.views().contains(&s.cfg.default_view) {
            s.cfg.default_view
        } else {
            s.cfg.views()[0]
        };
        let mut app = Self {
            keys,
            view,
            mode: Mode::Normal,
            rows: Vec::new(),
            sel: 0,
            scroll: 0,
            agenda_from: None,
            someday: false,
            all_overdue: false,
            month_day: today,
            day_sel: 0,
            day_rows: Vec::new(),
            tab: 0,
            sort: Sort::Due,
            expanded: HashSet::new(),
            filter: String::new(),
            toast,
            quit: false,
            hits: Vec::new(),
            body_height: 20,
            history: Vec::new(),
            history_idx: None,
            raw_edit: None,
            s,
        };
        app.tab = app.tabs().len().saturating_sub(1);
        app.rebuild();
        app
    }

    pub fn today(&self) -> NaiveDate {
        self.s.now().date_naive()
    }

    pub fn opts(&self) -> Opts {
        Opts {
            now: self.s.now(),
            tz: self.s.tz,
            hide_done_after: self.s.cfg.hide_done_after(),
        }
    }

    /// Tasks-view tabs: each list, then "all".
    pub fn tabs(&self) -> Vec<(Option<String>, String)> {
        let mut v: Vec<(Option<String>, String)> = self
            .s
            .store
            .lists
            .iter()
            .map(|l| (Some(l.id.clone()), l.name.clone()))
            .collect();
        v.push((None, "all".into()));
        v
    }

    fn tab_list(&self) -> Option<String> {
        self.tabs().get(self.tab).and_then(|(id, _)| id.clone())
    }

    fn passes(&self, e: &Entry) -> bool {
        self.filter.is_empty() || query::matches(&self.filter, &e.title, &e.list)
    }

    /// Recomputes the rows for the current view, keeping the selection on
    /// the same item when it's still there.
    pub fn rebuild(&mut self) {
        let keep = self.selected().map(|e| (e.uid.clone(), e.rid));
        let o = self.opts();
        let today = o.today();
        let mut rows = Vec::new();
        match self.view {
            View::Today => {
                let from = self.agenda_from.unwrap_or(today);
                let sections = query::agenda(
                    &self.s.store,
                    from,
                    self.s.cfg.days_ahead,
                    &o,
                    self.s.cfg.overdue_collapse,
                    self.all_overdue,
                );
                for sec in sections {
                    let entries: Vec<Entry> =
                        sec.entries.into_iter().filter(|e| self.passes(e)).collect();
                    let is_today = sec.day == Some(today);
                    if entries.is_empty() && !(is_today && self.filter.is_empty()) {
                        continue;
                    }
                    rows.push(Row::Header {
                        title: sec.title,
                        overdue: sec.day.is_none(),
                        today: is_today,
                    });
                    if entries.is_empty() {
                        rows.push(Row::Note("nothing today · a adds".into()));
                    }
                    for e in entries {
                        rows.push(Row::Item {
                            entry: e,
                            depth: 0,
                            has_children: false,
                            expanded: false,
                        });
                    }
                    if sec.hidden > 0 {
                        rows.push(Row::More(sec.hidden));
                    }
                }
                if self.someday {
                    let undated: Vec<Entry> = query::undated(&self.s.store, &o)
                        .into_iter()
                        .filter(|e| self.passes(e))
                        .collect();
                    rows.push(Row::Header {
                        title: "Someday".into(),
                        overdue: false,
                        today: false,
                    });
                    if undated.is_empty() {
                        rows.push(Row::Note("no undated tasks".into()));
                    }
                    for e in undated {
                        rows.push(Row::Item {
                            entry: e,
                            depth: 0,
                            has_children: false,
                            expanded: false,
                        });
                    }
                }
                if rows.iter().all(|r| !r.selectable()) && !self.filter.is_empty() {
                    rows.push(Row::Note(format!("nothing matches /{}", self.filter)));
                }
            }
            View::Tasks => {
                let list = self.tab_list();
                let all = query::tasks(&self.s.store, list.as_deref(), self.sort, &o);
                let mut parent_open = false;
                for r in all {
                    if r.depth == 0 {
                        parent_open =
                            self.expanded.contains(&r.entry.uid) || !self.filter.is_empty();
                    } else if !parent_open {
                        continue;
                    }
                    if !self.passes(&r.entry) {
                        continue;
                    }
                    let expanded = self.expanded.contains(&r.entry.uid);
                    rows.push(Row::Item {
                        entry: r.entry,
                        depth: r.depth,
                        has_children: r.has_children,
                        expanded,
                    });
                }
                if rows.is_empty() {
                    rows.push(Row::Note(if self.filter.is_empty() {
                        "no tasks here · a adds one".into()
                    } else {
                        format!("nothing matches /{}", self.filter)
                    }));
                }
            }
            View::Month => {
                self.day_rows = query::day(&self.s.store, self.month_day, &o)
                    .into_iter()
                    .filter(|e| self.passes(e))
                    .collect();
                if let Some((uid, rid)) = &keep
                    && let Some(i) = self
                        .day_rows
                        .iter()
                        .position(|e| &e.uid == uid && &e.rid == rid)
                {
                    self.day_sel = i;
                }
                self.day_sel = self.day_sel.min(self.day_rows.len().saturating_sub(1));
            }
        }
        self.rows = rows;
        if let Some((uid, rid)) = keep
            && let Some(i) = self
                .rows
                .iter()
                .position(|r| r.entry().is_some_and(|e| e.uid == uid && e.rid == rid))
        {
            self.sel = i;
        }
        self.fix_sel(1);
    }

    /// Moves `sel` onto a selectable row, searching in direction `dir`.
    fn fix_sel(&mut self, dir: i64) {
        if self.rows.is_empty() {
            self.sel = 0;
            return;
        }
        self.sel = self.sel.min(self.rows.len() - 1);
        if self.rows[self.sel].selectable() {
            return;
        }
        let n = self.rows.len() as i64;
        for d in [dir, -dir] {
            let mut i = self.sel as i64;
            while i >= 0 && i < n {
                if self.rows[i as usize].selectable() {
                    self.sel = i as usize;
                    return;
                }
                i += d;
            }
        }
    }

    pub fn selected(&self) -> Option<&Entry> {
        match self.view {
            View::Month => self.day_rows.get(self.day_sel),
            _ => self.rows.get(self.sel).and_then(Row::entry),
        }
    }

    fn move_sel(&mut self, by: i64) {
        if self.view == View::Month {
            let n = self.day_rows.len() as i64;
            if n > 0 {
                self.day_sel = (self.day_sel as i64 + by).clamp(0, n - 1) as usize;
            }
            return;
        }
        let selectable: Vec<usize> = (0..self.rows.len())
            .filter(|i| self.rows[*i].selectable())
            .collect();
        if selectable.is_empty() {
            return;
        }
        let pos = selectable.iter().position(|i| *i == self.sel).unwrap_or(0) as i64;
        let next = (pos + by).clamp(0, selectable.len() as i64 - 1) as usize;
        self.sel = selectable[next];
    }

    fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), false));
    }

    fn error(&mut self, e: impl std::fmt::Display) {
        self.toast = Some((format!("{e:#}"), true));
    }

    fn after_change(&mut self, r: Result<ops::Done>) {
        match r {
            Ok(d) => {
                let undo = self.keys.first(Action::Undo).unwrap_or_else(|| "u".into());
                let msg = if d.message.starts_with("undid")
                    || d.message.starts_with("redid")
                    || d.message.starts_with("nothing")
                {
                    d.message
                } else {
                    format!("{} · {undo} undo", d.message)
                };
                self.toast(msg);
                self.s.refresh_slots();
                if let Some(uid) = d.uid {
                    self.rebuild();
                    if let Some(i) = self
                        .rows
                        .iter()
                        .position(|r| r.entry().is_some_and(|e| e.uid == uid))
                    {
                        self.sel = i;
                    }
                    if self.view == View::Month
                        && let Some(i) = self.day_rows.iter().position(|e| e.uid == uid)
                    {
                        self.day_sel = i;
                    }
                }
            }
            Err(e) => self.error(e),
        }
        self.rebuild();
    }

    /// Opens on `v` instead of the default page (`ctui --view`). A page
    /// turned off in `views` stays off, with a toast, as with `1` `2` `3`.
    pub fn start_on(&mut self, v: View) {
        let today = self.month_day;
        self.switch(v);
        // Month opens on today, not on the first Today row's day.
        if self.month_day != today {
            self.month_day = today;
            self.rebuild();
        }
    }

    fn switch(&mut self, v: View) {
        if !self.s.cfg.views().contains(&v) {
            self.toast(format!(
                "the {} view is turned off (views in config)",
                v.name()
            ));
            return;
        }
        if v == View::Month
            && let Some(d) = self.selected().map(|e| e.day)
            && self.view == View::Today
        {
            self.month_day = d;
        }
        self.view = v;
        self.sel = 0;
        self.scroll = 0;
        self.rebuild();
    }

    fn cycle_view(&mut self, by: i64) {
        let views = self.s.cfg.views();
        let i = views.iter().position(|v| *v == self.view).unwrap_or(0) as i64;
        let next = views[(i + by).rem_euclid(views.len() as i64) as usize];
        self.switch(next);
    }

    /// The day the selection is on, for prefilling new items.
    fn context_day(&self) -> Option<NaiveDate> {
        match self.view {
            View::Month => Some(self.month_day),
            View::Today => {
                // The section the selection sits in.
                let mut day = None;
                for (i, r) in self.rows.iter().enumerate() {
                    if let Row::Item { entry, .. } = r
                        && i <= self.sel
                        && !entry.overdue
                    {
                        day = Some(entry.day);
                    }
                    if i >= self.sel {
                        break;
                    }
                }
                day.or(self.agenda_from)
            }
            View::Tasks => None,
        }
    }

    fn open_add(&mut self, event: bool) {
        let today = self.today();
        let mut suffix = String::new();
        if let Some(d) = self.context_day().filter(|d| *d != today || event) {
            suffix.push(' ');
            suffix.push_str(&parse::grammar_date(d, today));
        }
        if self.view == View::Tasks
            && let Some(l) = self.tab_list()
        {
            suffix.push_str(&format!(" #{l}"));
        }
        if event && self.view != View::Month {
            // Nothing else to prefill: the time is the user's to type.
        }
        self.mode = Mode::Input(Input::new(InputKind::Add { event }, suffix, 0));
    }

    fn open_edit(&mut self, uid: String, rid: Rid) {
        let Some(item) = self.s.store.get(&uid).cloned() else {
            return;
        };
        let text = ops::to_grammar(&item, rid, &self.s.env());
        let cursor = text.chars().count();
        self.mode = Mode::Input(Input::new(InputKind::Edit { uid, rid }, text, cursor));
    }

    fn open_move(&mut self, uid: String, rid: Rid) {
        self.mode = Mode::Input(Input::new(InputKind::Move { uid, rid }, String::new(), 0));
    }

    /// A repeating event's occurrence asks "this or all" first.
    fn occurrence(&self) -> Option<(String, Moment)> {
        let e = self.selected()?;
        match (e.kind, e.rid) {
            (Kind::Event, Some(r)) if e.recurring => Some((e.uid.clone(), r)),
            _ => None,
        }
    }

    pub fn dispatch(&mut self, a: Action) {
        self.toast = None;
        let page = (self.body_height / 2).max(1) as i64;
        match a {
            Action::Quit => self.quit = true,
            Action::Down => {
                if self.view == View::Month {
                    self.shift_day(7);
                } else {
                    self.move_sel(1);
                }
            }
            Action::Up => {
                if self.view == View::Month {
                    self.shift_day(-7);
                } else {
                    self.move_sel(-1);
                }
            }
            Action::Left | Action::Right => {
                let d = if a == Action::Left { -1 } else { 1 };
                match self.view {
                    View::Month => self.shift_day(d),
                    View::Tasks => self.shift_tab(d),
                    View::Today => self.move_sel(d),
                }
            }
            Action::ItemDown | Action::ItemUp if self.view == View::Month => {
                self.move_sel(if a == Action::ItemDown { 1 } else { -1 });
            }
            Action::PageDown => self.move_sel(page),
            Action::PageUp => self.move_sel(-page),
            Action::Top => self.move_sel(-100_000),
            Action::Bottom => self.move_sel(100_000),
            Action::ViewToday => self.switch(View::Today),
            Action::ViewMonth => self.switch(View::Month),
            Action::ViewTasks => self.switch(View::Tasks),
            Action::NextView => self.cycle_view(1),
            Action::PrevView => self.cycle_view(-1),
            Action::Add => self.open_add(false),
            Action::AddEvent => self.open_add(true),
            Action::Done | Action::Toggle => {
                if let Some(Row::More(_)) =
                    self.rows.get(self.sel).filter(|_| self.view != View::Month)
                {
                    self.all_overdue = true;
                    self.rebuild();
                    return;
                }
                let Some(e) = self.selected().cloned() else {
                    return;
                };
                if a == Action::Toggle && e.progress.is_some() && self.view == View::Tasks {
                    if !self.expanded.remove(&e.uid) {
                        self.expanded.insert(e.uid.clone());
                    }
                    self.rebuild();
                    return;
                }
                if e.kind == Kind::Event {
                    self.toast("events can't be ticked off · e edits, d deletes");
                    return;
                }
                let (store, journal, env) = self.s.parts();
                let r = ops::set_done(store, journal, &env, &e.uid, !e.done);
                self.after_change(r);
            }
            Action::Edit => {
                if let Some(Row::More(_)) =
                    self.rows.get(self.sel).filter(|_| self.view != View::Month)
                {
                    self.all_overdue = true;
                    self.rebuild();
                    return;
                }
                if let Some((uid, rid)) = self.occurrence() {
                    self.mode = Mode::Choose(Pending::Edit { uid, rid });
                } else if let Some(e) = self.selected().cloned() {
                    self.open_edit(e.uid, None);
                }
            }
            Action::EditRaw => {
                if let Some(e) = self.selected() {
                    self.raw_edit = Some(e.uid.clone());
                }
            }
            Action::Delete => {
                if let Some((uid, rid)) = self.occurrence() {
                    self.mode = Mode::Choose(Pending::Delete { uid, rid });
                } else if let Some(e) = self.selected().cloned() {
                    let (store, journal, env) = self.s.parts();
                    let r = ops::remove(store, journal, &env, &e.uid, None);
                    self.after_change(r);
                }
            }
            Action::Undo => {
                let r = ops::undo(&mut self.s.store, &mut self.s.journal);
                self.after_change(r);
            }
            Action::Redo => {
                let r = ops::redo(&mut self.s.store, &mut self.s.journal);
                self.after_change(r);
            }
            Action::Move => {
                if let Some((uid, rid)) = self.occurrence() {
                    self.mode = Mode::Choose(Pending::Move { uid, rid });
                } else if let Some(e) = self.selected().cloned() {
                    self.open_move(e.uid, None);
                }
            }
            Action::Later | Action::Earlier => {
                let days = if a == Action::Later { 1 } else { -1 };
                if let Some((uid, rid)) = self.occurrence() {
                    self.mode = Mode::Choose(Pending::Shift { uid, rid, days });
                } else if let Some(e) = self.selected().cloned() {
                    let (store, journal, env) = self.s.parts();
                    let r = ops::shift_days(store, journal, &env, &e.uid, None, days);
                    self.after_change(r);
                }
            }
            Action::Indent | Action::Outdent => self.indent(a == Action::Indent),
            Action::ItemDown | Action::ItemUp => {
                self.reorder(if a == Action::ItemDown { 1 } else { -1 })
            }
            Action::TabNext => self.shift_tab(1),
            Action::TabPrev => self.shift_tab(-1),
            Action::Today => {
                self.agenda_from = None;
                self.month_day = self.today();
                self.sel = 0;
                self.scroll = 0;
                self.rebuild();
                if self.view == View::Today
                    && let Some(i) = self
                        .rows
                        .iter()
                        .position(|r| matches!(r, Row::Header { today: true, .. }))
                {
                    self.sel = i;
                    self.fix_sel(1);
                }
            }
            Action::PrevPeriod | Action::NextPeriod => {
                let d: i64 = if a == Action::NextPeriod { 1 } else { -1 };
                match self.view {
                    View::Today => {
                        let from = self.agenda_from.unwrap_or(self.today())
                            + Duration::days(d * self.s.cfg.days_ahead as i64);
                        self.agenda_from = if from == self.today() {
                            None
                        } else {
                            Some(from)
                        };
                        self.sel = 0;
                    }
                    View::Month => self.month_day = add_months(self.month_day, d as i32),
                    View::Tasks => self.shift_tab(d),
                }
                self.rebuild();
            }
            Action::Goto => self.mode = Mode::Input(Input::new(InputKind::Goto, String::new(), 0)),
            Action::Filter => {
                let text = self.filter.clone();
                let n = text.chars().count();
                self.mode = Mode::Input(Input::new(InputKind::Filter, text, n));
            }
            Action::Command => {
                self.mode = Mode::Input(Input::new(InputKind::Command, String::new(), 0))
            }
            Action::Someday => {
                if self.view == View::Today {
                    self.someday = !self.someday;
                    self.rebuild();
                    self.toast(if self.someday {
                        "showing undated tasks"
                    } else {
                        "undated tasks hidden"
                    });
                } else {
                    self.switch(View::Today);
                    self.someday = true;
                    self.rebuild();
                }
            }
            Action::Sort => {
                if self.view == View::Tasks {
                    self.sort = self.sort.next();
                    self.rebuild();
                    self.toast(format!("sort · {}", self.sort.name()));
                }
            }
            Action::Theme => self.open_themes(),
            Action::Help => self.mode = Mode::Help { scroll: 0 },
        }
    }

    fn shift_day(&mut self, d: i64) {
        self.month_day += Duration::days(d);
        self.day_sel = 0;
        self.rebuild();
    }

    fn shift_tab(&mut self, d: i64) {
        let n = self.tabs().len() as i64;
        self.tab = (self.tab as i64 + d).rem_euclid(n) as usize;
        self.sel = 0;
        self.rebuild();
    }

    fn indent(&mut self, indent: bool) {
        if self.view != View::Tasks {
            self.toast("subtasks are made in the tasks view (3)");
            return;
        }
        let Some(e) = self.selected().cloned() else {
            return;
        };
        let parent = if indent {
            let above = self.rows[..self.sel].iter().rev().find_map(|r| match r {
                Row::Item {
                    entry, depth: 0, ..
                } => Some(entry.uid.clone()),
                _ => None,
            });
            match above {
                Some(p) => Some(p),
                None => {
                    self.toast("nothing above to go under");
                    return;
                }
            }
        } else {
            None
        };
        if let Some(p) = &parent {
            self.expanded.insert(p.clone());
        }
        let (store, journal, env) = self.s.parts();
        let r = ops::set_parent(store, journal, &env, &e.uid, parent.as_deref());
        self.after_change(r);
    }

    fn reorder(&mut self, by: i64) {
        if self.view != View::Tasks {
            return;
        }
        if self.sort != Sort::Manual {
            self.sort = Sort::Manual;
            self.rebuild();
        }
        let Some(Row::Item { entry, depth, .. }) = self.rows.get(self.sel).cloned() else {
            return;
        };
        let parent = self
            .s
            .store
            .get(&entry.uid)
            .and_then(|i| i.f.parent.clone());
        let siblings: Vec<String> = self
            .rows
            .iter()
            .filter_map(|r| match r {
                Row::Item {
                    entry: e, depth: d, ..
                } if *d == depth => {
                    let p = self.s.store.get(&e.uid).and_then(|i| i.f.parent.clone());
                    (p == parent).then(|| e.uid.clone())
                }
                _ => None,
            })
            .collect();
        let Some(i) = siblings.iter().position(|u| *u == entry.uid) else {
            return;
        };
        let j = i as i64 + by;
        if j < 0 || j >= siblings.len() as i64 {
            return;
        }
        let mut order = siblings;
        order.swap(i, j as usize);
        let (store, journal, env) = self.s.parts();
        let r = ops::reorder(store, journal, &env, &order, &entry.uid);
        self.after_change(r);
    }

    fn open_themes(&mut self) {
        let names: Vec<String> = theme::available(&self.s.paths.themes_dir())
            .into_keys()
            .collect();
        let idx = names
            .iter()
            .position(|n| *n == self.s.cfg.theme)
            .unwrap_or(0);
        self.mode = Mode::Themes {
            names,
            idx,
            original: self.s.cfg.theme.clone(),
        };
    }

    fn preview_theme(&mut self, name: &str) {
        match theme::load(name, &self.s.paths.themes_dir()) {
            Ok(t) => self.s.theme = t,
            Err(e) => self.error(e),
        }
    }

    /// Keys in a modal (help, theme picker, this/all, output).
    pub fn modal_key(&mut self, k: &KeyEvent) {
        let action = self.keys.action(k);
        match &mut self.mode {
            Mode::Help { scroll } | Mode::Output { scroll, .. } => match (k.code, action) {
                (KeyCode::Char('j') | KeyCode::Down, _) => *scroll = scroll.saturating_add(1),
                (KeyCode::Char('k') | KeyCode::Up, _) => *scroll = scroll.saturating_sub(1),
                (KeyCode::PageDown | KeyCode::Char(' '), _) => *scroll = scroll.saturating_add(10),
                (KeyCode::PageUp, _) => *scroll = scroll.saturating_sub(10),
                _ if k.modifiers.contains(KeyModifiers::CONTROL)
                    && k.code == KeyCode::Char('c') =>
                {
                    self.quit = true
                }
                _ => self.mode = Mode::Normal,
            },
            Mode::Themes {
                names,
                idx,
                original,
            } => {
                let n = names.len();
                let original = original.clone();
                match k.code {
                    KeyCode::Char('j') | KeyCode::Down => *idx = (*idx + 1) % n,
                    KeyCode::Char('k') | KeyCode::Up => *idx = (*idx + n - 1) % n,
                    KeyCode::Home => *idx = 0,
                    KeyCode::End | KeyCode::Char('G') => *idx = n - 1,
                    KeyCode::Enter => {
                        let name = names[*idx].clone();
                        let path = self.s.paths.config_file();
                        match self.s.cfg.set(&path, "theme", &name) {
                            Ok(()) => {
                                self.s.reload_theme();
                                self.toast(format!("theme · {name}"));
                            }
                            Err(e) => self.error(e),
                        }
                        self.mode = Mode::Normal;
                        return;
                    }
                    KeyCode::Esc | KeyCode::Char('q') => {
                        self.preview_theme(&original);
                        self.mode = Mode::Normal;
                        return;
                    }
                    _ => return,
                }
                let name = names[*idx].clone();
                self.preview_theme(&name);
            }
            Mode::Choose(p) => {
                let p = p.clone();
                let all = match k.code {
                    KeyCode::Char('t') | KeyCode::Char('1') => false,
                    KeyCode::Char('a') | KeyCode::Char('2') => true,
                    _ => {
                        self.mode = Mode::Normal;
                        return;
                    }
                };
                self.mode = Mode::Normal;
                let pick = |rid: Moment| if all { None } else { Some(rid) };
                match p {
                    Pending::Edit { uid, rid } => self.open_edit(uid, pick(rid)),
                    Pending::Move { uid, rid } => self.open_move(uid, pick(rid)),
                    Pending::Shift { uid, rid, days } => {
                        let (store, journal, env) = self.s.parts();
                        let r = ops::shift_days(store, journal, &env, &uid, pick(rid), days);
                        self.after_change(r);
                    }
                    Pending::Delete { uid, rid } => {
                        let (store, journal, env) = self.s.parts();
                        let r = ops::remove(store, journal, &env, &uid, pick(rid));
                        self.after_change(r);
                    }
                }
            }
            Mode::Normal | Mode::Input(_) => {}
        }
    }

    pub fn key(&mut self, k: KeyEvent) {
        match &self.mode {
            Mode::Input(_) => self.input_key(k),
            Mode::Normal => {
                if k.code == KeyCode::Esc {
                    if !self.filter.is_empty() {
                        self.filter.clear();
                        self.rebuild();
                        self.toast("filter cleared");
                    } else {
                        self.toast = None;
                    }
                    return;
                }
                if let Some(a) = self.keys.action(&k) {
                    self.dispatch(a);
                } else {
                    self.toast = None;
                }
            }
            _ => self.modal_key(&k),
        }
    }

    // ---------- typing ----------

    pub fn draft(&self, text: &str) -> Draft {
        let lists = &self.s.store.lists;
        parse::parse(text, &self.s.env().ctx(lists))
    }

    pub fn input_key(&mut self, k: KeyEvent) {
        let Mode::Input(inp) = &mut self.mode else {
            return;
        };
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let mut edited = true;
        match k.code {
            KeyCode::Esc => {
                let was_filter = inp.kind == InputKind::Filter;
                self.mode = Mode::Normal;
                if was_filter {
                    self.filter.clear();
                    self.rebuild();
                }
                return;
            }
            KeyCode::Enter => {
                let inp = inp.clone();
                self.mode = Mode::Normal;
                self.submit(inp);
                return;
            }
            KeyCode::Tab => {
                self.complete(1);
                return;
            }
            KeyCode::BackTab => {
                self.complete(-1);
                return;
            }
            KeyCode::Char('c') if ctrl => {
                self.mode = Mode::Normal;
                return;
            }
            KeyCode::Char('a') if ctrl => {
                inp.cursor = 0;
                edited = false;
            }
            KeyCode::Char('e') if ctrl => {
                inp.cursor = inp.len();
                edited = false;
            }
            KeyCode::Char('u') if ctrl => {
                let b = inp.byte(inp.cursor);
                inp.text.replace_range(..b, "");
                inp.cursor = 0;
            }
            KeyCode::Char('k') if ctrl => {
                let b = inp.byte(inp.cursor);
                inp.text.truncate(b);
            }
            KeyCode::Char('w') if ctrl => delete_word(inp),
            KeyCode::Backspace if ctrl || k.modifiers.contains(KeyModifiers::ALT) => {
                delete_word(inp)
            }
            KeyCode::Char(c) => {
                let b = inp.byte(inp.cursor);
                inp.text.insert(b, c);
                inp.cursor += 1;
            }
            KeyCode::Backspace => {
                if inp.cursor > 0 {
                    let b = inp.byte(inp.cursor - 1);
                    inp.text.remove(b);
                    inp.cursor -= 1;
                } else if inp.text.is_empty() {
                    self.mode = Mode::Normal;
                    return;
                }
            }
            KeyCode::Delete => {
                if inp.cursor < inp.len() {
                    let b = inp.byte(inp.cursor);
                    inp.text.remove(b);
                }
            }
            KeyCode::Left => {
                inp.cursor = inp.cursor.saturating_sub(1);
                edited = false;
            }
            KeyCode::Right => {
                inp.cursor = (inp.cursor + 1).min(inp.len());
                edited = false;
            }
            KeyCode::Home => {
                inp.cursor = 0;
                edited = false;
            }
            KeyCode::End => {
                inp.cursor = inp.len();
                edited = false;
            }
            KeyCode::Up | KeyCode::Down if inp.kind == InputKind::Command => {
                if self.history.is_empty() {
                    return;
                }
                let n = self.history.len();
                let i = match (self.history_idx, k.code) {
                    (None, KeyCode::Up) => n - 1,
                    (Some(i), KeyCode::Up) => i.saturating_sub(1),
                    (Some(i), KeyCode::Down) if i + 1 < n => i + 1,
                    _ => {
                        self.history_idx = None;
                        inp.text.clear();
                        inp.cursor = 0;
                        return;
                    }
                };
                self.history_idx = Some(i);
                inp.text = self.history[i].clone();
                inp.cursor = inp.len();
                return;
            }
            _ => edited = false,
        }
        if edited {
            inp.completions.clear();
            if inp.kind == InputKind::Filter {
                self.filter = inp.text.clone();
                self.sel = 0;
                self.rebuild();
            }
        }
    }

    /// Tab: completes `#list`, theme names, command names and settings.
    fn complete(&mut self, dir: i64) {
        let lists: Vec<String> = self.s.store.lists.iter().map(|l| l.id.clone()).collect();
        let themes: Vec<String> = theme::available(&self.s.paths.themes_dir())
            .into_keys()
            .collect();
        let Mode::Input(inp) = &mut self.mode else {
            return;
        };
        let b = inp.byte(inp.cursor);
        let before = &inp.text[..b];
        let start = before.rfind(' ').map(|i| i + 1).unwrap_or(0);
        if inp.completions.is_empty() {
            let word = &inp.text[start..b];
            let words_before: Vec<&str> = inp.text[..start].split_whitespace().collect();
            let pool: Vec<String> = if let Some(w) = word.strip_prefix(['#', '@']) {
                let sigil = &word[..1];
                lists
                    .iter()
                    .filter(|l| l.starts_with(&w.to_lowercase()))
                    .map(|l| format!("{sigil}{l}"))
                    .collect()
            } else if inp.kind == InputKind::Command {
                match words_before.as_slice() {
                    [] => command_names()
                        .into_iter()
                        .filter(|c| c.starts_with(word))
                        .collect(),
                    ["theme"] | ["set", "theme"] => themes
                        .iter()
                        .filter(|t| t.starts_with(word))
                        .cloned()
                        .collect(),
                    ["set"] => crate::config::SETTINGS
                        .iter()
                        .map(|(k, _)| k.to_string())
                        .filter(|k| k.starts_with(word))
                        .collect(),
                    ["lists"] => ["add", "rename", "rm", "color"]
                        .iter()
                        .map(|s| s.to_string())
                        .filter(|s| s.starts_with(word))
                        .collect(),
                    ["lists", _] | ["ls"] | ["export"] => lists
                        .iter()
                        .filter(|l| l.starts_with(word))
                        .cloned()
                        .collect(),
                    _ => Vec::new(),
                }
            } else {
                Vec::new()
            };
            if pool.is_empty() {
                return;
            }
            inp.completions = pool;
            inp.completion_idx = if dir > 0 {
                0
            } else {
                inp.completions.len() - 1
            };
        } else {
            let n = inp.completions.len() as i64;
            inp.completion_idx = (inp.completion_idx as i64 + dir).rem_euclid(n) as usize;
        }
        let pick = inp.completions[inp.completion_idx].clone();
        let end = inp.text[b..]
            .find(' ')
            .map(|i| b + i)
            .unwrap_or(inp.text.len());
        inp.text.replace_range(start..end, &pick);
        inp.cursor = inp.text[..start].chars().count() + pick.chars().count();
        if inp.completions.len() == 1 {
            let b = inp.byte(inp.cursor);
            if !inp.text[b..].starts_with(' ') {
                inp.text.insert(b, ' ');
            }
            inp.cursor += 1;
            inp.completions.clear();
        }
    }

    fn submit(&mut self, inp: Input) {
        let text = inp.text.trim().to_string();
        match inp.kind.clone() {
            InputKind::Add { event } => {
                if text.is_empty() {
                    return;
                }
                let defaults = Defaults {
                    date: self.context_day(),
                    list: self.tab_list().filter(|_| self.view == View::Tasks),
                };
                let (store, journal, env) = self.s.parts();
                let r = ops::add(
                    store,
                    journal,
                    &env,
                    &text,
                    event.then_some(Kind::Event),
                    &defaults,
                );
                if r.is_err() {
                    // Keep what was typed so it can be fixed.
                    let err = r.err().map(|e| format!("{e:#}")).unwrap_or_default();
                    let n = inp.text.chars().count();
                    self.mode = Mode::Input(Input { cursor: n, ..inp });
                    self.toast = Some((err, true));
                    return;
                }
                self.after_change(r);
            }
            InputKind::Edit { uid, rid } => {
                let (store, journal, env) = self.s.parts();
                let r = ops::edit(store, journal, &env, &uid, rid, &text);
                if let Err(e) = &r {
                    let err = format!("{e:#}");
                    self.mode = Mode::Input(inp);
                    self.toast = Some((err, true));
                    return;
                }
                self.after_change(r);
            }
            InputKind::Move { uid, rid } => {
                if text.is_empty() {
                    return;
                }
                let (store, journal, env) = self.s.parts();
                let r = ops::move_to(store, journal, &env, &uid, rid, &text);
                self.after_change(r);
            }
            InputKind::Goto => {
                let lists = self.s.store.lists.clone();
                match parse::parse_date(&text, &self.s.env().ctx(&lists)) {
                    Some(d) => {
                        self.month_day = d;
                        self.agenda_from = if d == self.today() { None } else { Some(d) };
                        self.sel = 0;
                        self.rebuild();
                        self.toast(format!("→ {}", d.format("%a %-d %b %Y")));
                    }
                    None => self.error(format!("not a date: {text:?}")),
                }
            }
            InputKind::Filter => {
                self.filter = text;
                self.rebuild();
                if !self.filter.is_empty() {
                    self.toast("esc clears the filter");
                }
            }
            InputKind::Command => self.command(&text),
        }
    }

    /// Runs a `:` command line: TUI words first, then any `ct` command.
    pub fn command(&mut self, line: &str) {
        if line.is_empty() {
            return;
        }
        self.history.retain(|h| h != line);
        self.history.push(line.to_string());
        self.history_idx = None;
        let words = split_words(line);
        match words.first().map(String::as_str) {
            Some("q" | "quit" | "exit" | "wq") => {
                self.quit = true;
                return;
            }
            Some("help" | "h") if words.len() == 1 => {
                self.mode = Mode::Help { scroll: 0 };
                return;
            }
            Some("theme" | "themes") if words.len() == 1 => {
                self.open_themes();
                return;
            }
            // Page names, and the shell shortcut names (`:ctm`, `:cttasks`).
            Some(w)
                if words.len() == 1
                    && let Some(v) =
                        View::parse(w).ok().or_else(|| crate::shortcuts::page_of(w)) =>
            {
                self.switch(v);
                return;
            }
            _ => {}
        }
        let mut argv = vec!["ctui".to_string()];
        argv.extend(words);
        let parsed = <Cli as clap::Parser>::try_parse_from(&argv);
        let mut buf: Vec<u8> = Vec::new();
        let result = match parsed {
            Ok(cli) => match cli.cmd {
                Some(cmd) => {
                    let mut out = Out {
                        w: &mut buf,
                        color: false,
                    };
                    cli::run(&mut self.s, cmd, &mut out, false)
                }
                None => Ok(()),
            },
            Err(e) => {
                buf.extend_from_slice(e.render().to_string().as_bytes());
                Ok(())
            }
        };
        let text = String::from_utf8_lossy(&buf).trim_end().to_string();
        // Settings may have changed.
        self.keys = Keymap::new(&self.s.cfg.keys);
        self.s.refresh_slots();
        if !self.s.cfg.views().contains(&self.view) {
            self.view = self.s.cfg.views()[0];
        }
        self.rebuild();
        match result {
            Err(e) => self.error(e),
            Ok(()) => {
                let lines: Vec<String> = text.lines().map(str::to_string).collect();
                match lines.len() {
                    0 => {}
                    1 => self.toast(lines[0].clone()),
                    _ => self.mode = Mode::Output { lines, scroll: 0 },
                }
            }
        }
    }

    // ---------- mouse ----------

    pub fn mouse(&mut self, m: MouseEvent) {
        if !matches!(self.mode, Mode::Normal) {
            if let Mode::Help { scroll } | Mode::Output { scroll, .. } = &mut self.mode {
                match m.kind {
                    MouseEventKind::ScrollDown => *scroll = scroll.saturating_add(3),
                    MouseEventKind::ScrollUp => *scroll = scroll.saturating_sub(3),
                    MouseEventKind::Down(_) => self.mode = Mode::Normal,
                    _ => {}
                }
            }
            return;
        }
        match m.kind {
            MouseEventKind::ScrollDown => self.move_sel(3),
            MouseEventKind::ScrollUp => self.move_sel(-3),
            MouseEventKind::Down(MouseButton::Left) => {
                let hit = self
                    .hits
                    .iter()
                    .rev()
                    .find(|(r, _)| contains(*r, m.column, m.row))
                    .map(|(_, h)| h.clone());
                self.toast = None;
                match hit {
                    Some(Hit::View(v)) => self.switch(v),
                    Some(Hit::Row(i)) => {
                        if self.view == View::Month {
                            self.day_sel = i;
                        } else if self.rows.get(i).is_some_and(Row::selectable) {
                            if self.sel == i && matches!(self.rows[i], Row::More(_)) {
                                self.dispatch(Action::Edit);
                            }
                            self.sel = i;
                        }
                    }
                    Some(Hit::Check(i)) => {
                        if self.view == View::Month {
                            self.day_sel = i;
                        } else {
                            self.sel = i;
                        }
                        self.dispatch(Action::Done);
                    }
                    Some(Hit::Day(d)) => {
                        if self.month_day == d {
                            self.switch_to_day(d);
                        } else {
                            self.month_day = d;
                            self.day_sel = 0;
                            self.rebuild();
                        }
                    }
                    Some(Hit::Tab(t)) => {
                        self.tab = t;
                        self.sel = 0;
                        self.rebuild();
                    }
                    None => {}
                }
            }
            _ => {}
        }
    }

    pub fn after_raw_edit(&mut self, r: Result<String>) {
        match r {
            Ok(msg) => self.toast(msg),
            Err(e) => self.error(e),
        }
        self.rebuild();
    }

    /// Opens a day on the Today page (enter or a second click in the month view).
    pub fn switch_to_day(&mut self, d: NaiveDate) {
        self.agenda_from = if d == self.today() { None } else { Some(d) };
        self.switch(View::Today);
    }

    /// Month grid bounds: the first shown day and the month's last day.
    pub fn month_bounds(&self) -> (NaiveDate, NaiveDate, NaiveDate) {
        let first = self.month_day.with_day(1).expect("day 1");
        let last = last_of_month(first);
        (week_start_of(first, self.s.cfg.week_start), first, last)
    }

    /// Called when the screen is idle: picks up outside changes.
    pub fn tick(&mut self) {
        if self.s.store.changed_on_disk() {
            if let Err(e) = self.s.store.reload() {
                self.error(e);
            }
            self.s.refresh_slots();
            self.rebuild();
        } else if matches!(self.mode, Mode::Normal) {
            // The clock moved: "now", overdue and today can change.
            self.rebuild();
        }
    }
}

fn contains(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

fn delete_word(inp: &mut Input) {
    let b = inp.byte(inp.cursor);
    let before = &inp.text[..b];
    let trimmed = before.trim_end();
    let start = trimmed.rfind(' ').map(|i| i + 1).unwrap_or(0);
    let removed = inp.text[start..b].chars().count();
    inp.text.replace_range(start..b, "");
    inp.cursor -= removed;
}

/// Splits a command line into words, honouring "double" and 'single' quotes.
pub fn split_words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;
    for c in line.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() || any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if !cur.is_empty() || any {
        out.push(cur);
    }
    out
}

/// Every `ct` command name and alias, plus the TUI's own words.
pub fn command_names() -> Vec<String> {
    use clap::CommandFactory;
    let cmd = Cli::command();
    let mut v: Vec<String> = cmd
        .get_subcommands()
        .flat_map(|c| {
            std::iter::once(c.get_name().to_string())
                .chain(c.get_visible_aliases().map(str::to_string))
        })
        .filter(|n| n != "completions" && n != "init")
        .collect();
    v.extend(["quit", "help", "month", "tasks"].map(String::from));
    v.extend(
        crate::shortcuts::candidates()
            .into_iter()
            .flat_map(|(_, n)| n),
    );
    v.sort();
    v.dedup();
    v
}

/// The about text of a command, for the command line's hint.
pub fn command_about(name: &str) -> Option<String> {
    use clap::CommandFactory;
    let cmd = Cli::command();
    cmd.get_subcommands()
        .find(|c| c.get_name() == name || c.get_visible_aliases().any(|a| a == name))
        .and_then(|c| c.get_about().map(|a| a.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words() {
        assert_eq!(
            split_words(r#"add "fri" night 'a b' x"#),
            vec!["add", "fri", "night", "a b", "x"]
        );
        assert_eq!(split_words("  "), Vec::<String>::new());
    }
}
