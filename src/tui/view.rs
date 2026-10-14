//! Drawing only: nothing here changes state except recording where things
//! are on screen for mouse clicks (`App::hits`) and the scroll position.

use chrono::{Datelike, Duration};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use super::app::{App, Hit, InputKind, Mode, Pending, Row};
use super::keys::{Action, TABLE};
use crate::config::View;
use crate::core::model::Kind;
use crate::core::parse::{self, Role, fmt_time};
use crate::core::query::{self, Entry};
use crate::theme::{self, Color};

pub fn color(c: Color) -> ratatui::style::Color {
    match c {
        Color::Reset => ratatui::style::Color::Reset,
        Color::Ansi(n) => ratatui::style::Color::Indexed(n),
        Color::Rgb(r, g, b) => ratatui::style::Color::Rgb(r, g, b),
    }
}

struct Pal {
    base: Style,
    fg: ratatui::style::Color,
    sub: ratatui::style::Color,
    main: ratatui::style::Color,
    error: ratatui::style::Color,
    done: ratatui::style::Color,
    overdue: ratatui::style::Color,
    today: ratatui::style::Color,
    border: ratatui::style::Color,
    select: Option<ratatui::style::Color>,
}

fn pal(app: &App) -> Pal {
    let t = &app.s.theme;
    Pal {
        base: Style::default().fg(color(t.fg)).bg(color(t.bg)),
        fg: color(t.fg),
        sub: color(t.sub),
        main: color(t.main),
        error: color(t.error),
        done: color(t.done),
        overdue: color(t.overdue),
        today: color(t.today),
        border: color(t.border),
        select: t.select.map(color),
    }
}

fn width(s: &str) -> usize {
    s.chars().count()
}

fn truncate(s: &str, w: usize) -> String {
    if width(s) <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(w.saturating_sub(1)).collect();
    out.push('…');
    out
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let p = pal(app);
    let area = f.area();
    f.render_widget(Block::default().style(p.base), area);
    app.hits.clear();
    let input_h = if matches!(app.mode, Mode::Input(_)) {
        2
    } else {
        0
    };
    let [top, body, input, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(input_h),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_top(f, app, &p, top);
    let body = Rect {
        y: body.y + 1,
        height: body.height.saturating_sub(1),
        ..body
    };
    app.body_height = body.height as usize;
    match app.view {
        View::Agenda | View::Tasks => draw_list(f, app, &p, body),
        View::Month => draw_month(f, app, &p, body),
    }
    if input_h > 0 {
        draw_input(f, app, &p, input);
    }
    draw_foot(f, app, &p, foot);
    match &app.mode {
        Mode::Help { scroll } => draw_help(f, app, &p, *scroll),
        Mode::Themes { names, idx, .. } => draw_themes(f, app, &p, names, *idx),
        Mode::Choose(pending) => draw_choose(f, &p, pending),
        Mode::Output { lines, scroll } => draw_output(f, &p, lines, *scroll),
        _ => {}
    }
}

fn draw_top(f: &mut Frame, app: &mut App, p: &Pal, area: Rect) {
    let mut spans = vec![Span::styled(
        " ctui ",
        Style::default().fg(p.main).add_modifier(Modifier::BOLD),
    )];
    let mut x = area.x + 6;
    for (i, v) in app.s.cfg.views().into_iter().enumerate() {
        let label = format!(" {} {} ", i + 1, capital(v.name()));
        let w = width(&label) as u16;
        let style = if v == app.view {
            Style::default()
                .fg(p.main)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(p.sub)
        };
        spans.push(Span::styled(label, style));
        app.hits.push((
            Rect {
                x,
                y: area.y,
                width: w,
                height: 1,
            },
            Hit::View(v),
        ));
        x += w;
    }
    let now = app.s.now();
    let mut right = now.format("%a %-d %b %Y").to_string();
    if let Some(t) = Some(fmt_time(now.time(), app.s.cfg.time_format)) {
        right = format!("{right}  {t} ");
    }
    let mut filter = String::new();
    if !app.filter.is_empty() {
        filter = format!("/{}  ", app.filter);
    }
    let left_w: usize = spans.iter().map(|s| width(&s.content)).sum();
    let gap = (area.width as usize).saturating_sub(left_w + width(&right) + width(&filter));
    spans.push(Span::raw(" ".repeat(gap)));
    spans.push(Span::styled(filter, Style::default().fg(p.main)));
    spans.push(Span::styled(right, Style::default().fg(p.sub)));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(x) => x.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// The time column for an entry: `09:30–10:00`, `all day`, `due Mon 13`.
fn time_text(app: &App, e: &Entry, in_tasks: bool) -> String {
    let fmt = app.s.cfg.time_format;
    let today = app.today();
    if in_tasks || (e.overdue && e.due.is_some_and(|d| d < today)) {
        let Some(d) = e.due else { return String::new() };
        let mut s = if in_tasks {
            crate::cli::short_day(d, today)
        } else if (today - d).num_days() < 7 {
            d.format("due %a %-d").to_string()
        } else {
            d.format("due %-d %b").to_string()
        };
        if let Some(t) = e.start {
            s.push(' ');
            s.push_str(&fmt_time(t, fmt));
        }
        return s;
    }
    match (e.start, e.end, e.all_day) {
        (_, _, true) if e.continued => "… all day".into(),
        (_, _, true) => "all day".into(),
        (Some(a), Some(b), _) => format!("{}–{}", fmt_time(a, fmt), fmt_time(b, fmt)),
        (Some(a), None, _) => fmt_time(a, fmt),
        (None, Some(b), _) => format!("…{}", fmt_time(b, fmt)),
        (None, None, _) => String::new(),
    }
}

fn time_width(app: &App) -> usize {
    match app.s.cfg.time_format {
        crate::config::TimeFormat::H24 => 11,
        crate::config::TimeFormat::H12 => 15,
    }
}

/// One entry as a line; returns the line and the checkbox's x offset.
#[allow(clippy::too_many_arguments)]
fn entry_line(
    app: &App,
    p: &Pal,
    e: &Entry,
    depth: u8,
    has_children: bool,
    expanded: bool,
    w: usize,
    selected: bool,
    in_tasks: bool,
) -> (Line<'static>, usize) {
    let list_color = color(app.s.list_color(&e.list));
    let mut spans: Vec<Span> = Vec::new();
    let marker = if selected { " › " } else { "   " };
    spans.push(Span::styled(
        marker,
        Style::default().fg(p.main).add_modifier(Modifier::BOLD),
    ));
    let mut used = 3;
    if depth > 0 {
        spans.push(Span::raw("   ".repeat(depth as usize)));
        used += 3 * depth as usize;
    }
    if in_tasks && depth == 0 {
        let exp = if has_children {
            if expanded { "▾ " } else { "▸ " }
        } else {
            "  "
        };
        spans.push(Span::styled(exp, Style::default().fg(p.sub)));
        used += 2;
    }
    let check_x = used;
    let (glyph, gstyle) = match (e.kind, e.done) {
        (Kind::Task, true) => ("☑ ", Style::default().fg(p.done)),
        (Kind::Task, false) => (
            "☐ ",
            Style::default().fg(if e.overdue { p.overdue } else { p.fg }),
        ),
        (Kind::Event, _) => ("┃ ", Style::default().fg(list_color)),
    };
    spans.push(Span::styled(glyph, gstyle));
    used += 2;
    let show_list = w >= 52;
    let show_time = w >= 36;
    let tw = if in_tasks {
        if w < 60 { 11 } else { 16 }
    } else {
        time_width(app)
    };
    if show_time {
        let t = time_text(app, e, in_tasks);
        let style = if e.now {
            Style::default().fg(p.main).add_modifier(Modifier::BOLD)
        } else if e.overdue {
            Style::default().fg(p.overdue)
        } else {
            Style::default().fg(p.sub)
        };
        spans.push(Span::styled(format!("{:<tw$} ", truncate(&t, tw)), style));
        used += tw + 1;
    }
    let mut right: Vec<Span> = Vec::new();
    let mut right_w = 0;
    if show_list {
        let tag = format!("  #{}", truncate(&e.list, 14));
        right_w += width(&tag);
        right.push(Span::styled(tag, Style::default().fg(list_color)));
    }
    if e.priority > 0 {
        let bang = format!(" {}", "!".repeat(e.priority as usize));
        right_w += width(&bang);
        right.push(Span::styled(
            bang,
            Style::default().fg(p.error).add_modifier(Modifier::BOLD),
        ));
    }
    let mut title = e.title.clone();
    if let Some((d, t)) = e.progress {
        title.push_str(&format!("  {d}/{t}"));
    }
    if e.recurring {
        title.push_str(" ↻");
    }
    if e.now && e.kind == Kind::Event {
        title.push_str("  · now");
    }
    let avail = w.saturating_sub(used + right_w + 1);
    let title = truncate(&title, avail);
    let mut tstyle = Style::default().fg(p.fg);
    if e.done {
        tstyle = Style::default()
            .fg(p.done)
            .add_modifier(Modifier::CROSSED_OUT);
    } else if e.now {
        tstyle = tstyle.add_modifier(Modifier::BOLD);
    }
    let pad = avail.saturating_sub(width(&title));
    spans.push(Span::styled(title, tstyle));
    spans.push(Span::raw(" ".repeat(pad)));
    spans.extend(right);
    let mut line = Line::from(spans);
    if selected {
        line = match p.select {
            Some(bg) => line.style(Style::default().bg(bg)),
            None => line.style(Style::default().add_modifier(Modifier::BOLD)),
        };
    }
    (line, check_x)
}

fn draw_list(f: &mut Frame, app: &mut App, p: &Pal, area: Rect) {
    let mut area = area;
    if app.view == View::Tasks {
        let tabs_area = Rect { height: 1, ..area };
        draw_tabs(f, app, p, tabs_area);
        area = Rect {
            y: area.y + 2,
            height: area.height.saturating_sub(2),
            ..area
        };
    }
    let w = area.width as usize;
    // Visual lines: (row index, line, checkbox x).
    let mut lines: Vec<(Option<usize>, Line<'static>, usize)> = Vec::new();
    for (i, r) in app.rows.iter().enumerate() {
        let selected =
            i == app.sel && matches!(app.mode, Mode::Normal | Mode::Input(_) | Mode::Choose(_));
        match r {
            Row::Header {
                title,
                overdue,
                today,
            } => {
                if !lines.is_empty() {
                    lines.push((None, Line::raw(""), 0));
                }
                let style = if *overdue {
                    Style::default().fg(p.overdue).add_modifier(Modifier::BOLD)
                } else if *today {
                    Style::default().fg(p.today).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(p.fg).add_modifier(Modifier::BOLD)
                };
                lines.push((
                    None,
                    Line::from(Span::styled(format!(" {title}"), style)),
                    0,
                ));
            }
            Row::Item {
                entry,
                depth,
                has_children,
                expanded,
            } => {
                let (line, cx) = entry_line(
                    app,
                    p,
                    entry,
                    *depth,
                    *has_children,
                    *expanded,
                    w,
                    selected,
                    app.view == View::Tasks,
                );
                lines.push((Some(i), line, cx));
            }
            Row::More(n) => {
                let marker = if selected { " › " } else { "   " };
                let mut line = Line::from(vec![
                    Span::styled(marker, Style::default().fg(p.main)),
                    Span::styled(
                        format!("  +{n} more · enter shows them"),
                        Style::default().fg(p.sub),
                    ),
                ]);
                if selected && let Some(bg) = p.select {
                    line = line.style(Style::default().bg(bg));
                }
                lines.push((Some(i), line, 0));
            }
            Row::Note(t) => lines.push((
                None,
                Line::from(Span::styled(
                    format!("     {t}"),
                    Style::default().fg(p.sub),
                )),
                0,
            )),
        }
    }
    let h = area.height as usize;
    let sel_line = lines
        .iter()
        .position(|(r, ..)| *r == Some(app.sel))
        .unwrap_or(0);
    // Keep the selection in view with a little margin; show the header above it.
    let margin = 2.min(h / 4);
    if sel_line < app.scroll + margin {
        app.scroll = sel_line.saturating_sub(margin.max(1));
    } else if sel_line + margin >= app.scroll + h {
        app.scroll = (sel_line + margin + 1).saturating_sub(h);
    }
    app.scroll = app
        .scroll
        .min(lines.len().saturating_sub(h.min(lines.len())));
    for (k, (row, line, cx)) in lines.into_iter().skip(app.scroll).take(h).enumerate() {
        let r = Rect {
            x: area.x,
            y: area.y + k as u16,
            width: area.width,
            height: 1,
        };
        f.render_widget(Paragraph::new(line), r);
        if let Some(i) = row {
            app.hits.push((r, Hit::Row(i)));
            app.hits.push((
                Rect {
                    x: area.x + cx as u16,
                    width: 2,
                    ..r
                },
                Hit::Check(i),
            ));
        }
    }
}

fn draw_tabs(f: &mut Frame, app: &mut App, p: &Pal, area: Rect) {
    let o = app.opts();
    let mut spans = vec![Span::raw(" ")];
    let mut x = area.x + 1;
    for (i, (id, name)) in app.tabs().into_iter().enumerate() {
        let open = app
            .s
            .store
            .items
            .iter()
            .filter(|t| {
                t.kind == Kind::Task
                    && !t.f.done
                    && id.as_ref().is_none_or(|l| &t.list == l)
                    && o.done_visible(t)
            })
            .count();
        let label = format!(" {name} {open} ");
        let w = width(&label) as u16;
        let c = match &id {
            Some(l) => color(app.s.list_color(l)),
            None => p.fg,
        };
        let style = if i == app.tab {
            Style::default()
                .fg(c)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(p.sub)
        };
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
        app.hits.push((
            Rect {
                x,
                y: area.y,
                width: w,
                height: 1,
            },
            Hit::Tab(i),
        ));
        x += w + 1;
    }
    let sort = format!("sort: {} ", app.sort.name());
    let used: usize = spans.iter().map(|s| width(&s.content)).sum();
    if used + width(&sort) < area.width as usize {
        spans.push(Span::raw(
            " ".repeat(area.width as usize - used - width(&sort)),
        ));
        spans.push(Span::styled(sort, Style::default().fg(p.sub)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_month(f: &mut Frame, app: &mut App, p: &Pal, area: Rect) {
    let big = area.width >= 100 && area.height >= 26;
    let side = area.width >= 64;
    let (grid, pane) = if big {
        let [g, d] = Layout::horizontal([Constraint::Min(60), Constraint::Length(38)]).areas(area);
        (g, d)
    } else if side {
        let [g, d] = Layout::horizontal([Constraint::Length(32), Constraint::Min(20)]).areas(area);
        (g, d)
    } else {
        let (start, _, last) = app.month_bounds();
        let weeks = ((last - start).num_days() / 7 + 1) as u16;
        let [g, d] =
            Layout::vertical([Constraint::Length(weeks + 2), Constraint::Min(2)]).areas(area);
        (g, d)
    };
    draw_grid(f, app, p, grid, big);
    draw_day_pane(f, app, p, pane);
}

fn draw_grid(f: &mut Frame, app: &mut App, p: &Pal, area: Rect, big: bool) {
    let (start, first, last) = app.month_bounds();
    let o = app.opts();
    let busy = query::busy_days(&app.s.store, start, start + Duration::days(42), &o);
    let today = app.today();
    let title = first.format("%B %Y").to_string();
    let cell_w: u16 = if big {
        (area.width.saturating_sub(2) / 7).max(4)
    } else {
        4
    };
    let grid_w = cell_w * 7;
    let x0 = area.x + 1;
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("{title:^w$}", w = grid_w as usize),
            Style::default().fg(p.main).add_modifier(Modifier::BOLD),
        ))),
        Rect {
            x: x0,
            y: area.y,
            width: grid_w.min(area.width),
            height: 1,
        },
    );
    let days = crate::cli::weekday_order(app.s.cfg.week_start);
    let heads: String = if big {
        days.iter()
            .map(|d| format!(" {:<w$}", &d.to_string()[..2], w = cell_w as usize - 1))
            .collect()
    } else {
        days.iter()
            .map(|d| format!("{:>3} ", &d.to_string()[..2]))
            .collect()
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(heads, Style::default().fg(p.sub)))),
        Rect {
            x: x0,
            y: area.y + 1,
            width: grid_w.min(area.width),
            height: 1,
        },
    );
    let weeks = ((last - start).num_days() / 7 + 1) as u16;
    let cell_h: u16 = if big {
        ((area.height.saturating_sub(2)) / weeks.max(1)).clamp(1, 6)
    } else {
        1
    };
    let mut d = start;
    for wk in 0..weeks {
        for col in 0..7u16 {
            let x = x0 + col * cell_w;
            let y = area.y + 2 + wk * cell_h;
            if y >= area.y + area.height {
                break;
            }
            let cell = Rect {
                x,
                y,
                width: cell_w,
                height: cell_h.min(area.y + area.height - y),
            };
            let in_month = d.month() == first.month();
            let lists = busy.get(&d);
            let selected = d == app.month_day;
            let mut num_style = Style::default().fg(if in_month { p.fg } else { p.sub });
            if d == today {
                num_style = Style::default()
                    .fg(p.today)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
            }
            let mut spans = Vec::new();
            if big {
                spans.push(Span::styled(format!(" {:<2}", d.day()), num_style));
                if let Some(l) = lists {
                    let dots: String = "•".repeat(l.len().min(cell_w as usize - 4));
                    spans.push(Span::styled(
                        dots,
                        Style::default().fg(color(app.s.list_color(&l[0]))),
                    ));
                }
            } else {
                spans.push(Span::styled(format!("{:>3}", d.day()), num_style));
                let dot = match lists {
                    Some(l) if l.len() >= 3 => {
                        Span::styled("•", Style::default().fg(color(app.s.list_color(&l[0]))))
                    }
                    Some(l) => {
                        Span::styled("·", Style::default().fg(color(app.s.list_color(&l[0]))))
                    }
                    None => Span::raw(" "),
                };
                spans.push(dot);
            }
            let mut line = Line::from(spans);
            if selected {
                line = match p.select {
                    Some(bg) => line.style(Style::default().bg(bg).add_modifier(Modifier::BOLD)),
                    None => line.style(Style::default().add_modifier(Modifier::REVERSED)),
                };
            }
            let mut text = vec![line];
            if big && cell.height > 1 {
                let entries = query::day(&app.s.store, d, &o);
                let room = cell.height as usize - 1;
                for (k, e) in entries.iter().enumerate().take(room) {
                    if k + 1 == room && entries.len() > room {
                        text.push(Line::from(Span::styled(
                            format!(" +{}", entries.len() - k),
                            Style::default().fg(p.sub),
                        )));
                        break;
                    }
                    let c = if e.done {
                        p.done
                    } else {
                        color(app.s.list_color(&e.list))
                    };
                    let t = match e.start.filter(|_| e.kind == Kind::Event && cell_w >= 16) {
                        Some(s) => format!(" {} {}", fmt_time(s, app.s.cfg.time_format), e.title),
                        None => format!(" {}", e.title),
                    };
                    text.push(Line::from(Span::styled(
                        truncate(&t, cell_w as usize - 1),
                        Style::default().fg(c),
                    )));
                }
            }
            f.render_widget(Paragraph::new(text), cell);
            app.hits.push((cell, Hit::Day(d)));
            d += Duration::days(1);
        }
    }
}

fn draw_day_pane(f: &mut Frame, app: &mut App, p: &Pal, area: Rect) {
    let today = app.today();
    let d = app.month_day;
    let title = query::day_title(d, today);
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(p.border));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let head_style = if d == today {
        Style::default().fg(p.today)
    } else {
        Style::default().fg(p.fg)
    }
    .add_modifier(Modifier::BOLD);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(format!(" {title}"), head_style))),
        Rect { height: 1, ..inner },
    );
    let list = Rect {
        y: inner.y + 1,
        height: inner.height.saturating_sub(1),
        ..inner
    };
    if app.day_rows.is_empty() {
        let hint = format!(
            "     nothing · {} adds",
            app.keys.first(Action::Add).unwrap_or_default()
        );
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(hint, Style::default().fg(p.sub)))),
            Rect { height: 1, ..list },
        );
        return;
    }
    let w = list.width as usize;
    let rows = app.day_rows.clone();
    let h = list.height as usize;
    let skip = app.day_sel.saturating_sub(h.saturating_sub(1));
    for (k, (i, e)) in rows.iter().enumerate().skip(skip).take(h).enumerate() {
        let (line, cx) = entry_line(app, p, e, 0, false, false, w, i == app.day_sel, false);
        let r = Rect {
            x: list.x,
            y: list.y + k as u16,
            width: list.width,
            height: 1,
        };
        f.render_widget(Paragraph::new(line), r);
        app.hits.push((r, Hit::Row(i)));
        app.hits.push((
            Rect {
                x: list.x + cx as u16,
                width: 2,
                ..r
            },
            Hit::Check(i),
        ));
    }
}

fn draw_input(f: &mut Frame, app: &App, p: &Pal, area: Rect) {
    let Mode::Input(inp) = &app.mode else { return };
    let (label, grammar) = match &inp.kind {
        InputKind::Add { event: false } => ("add ", true),
        InputKind::Add { event: true } => ("add event ", true),
        InputKind::Edit { rid: Some(_), .. } => ("edit this one ", true),
        InputKind::Edit { .. } => ("edit ", true),
        InputKind::Move { .. } => ("move to ", true),
        InputKind::Goto => ("go to ", true),
        InputKind::Filter => ("/", false),
        InputKind::Command => (":", false),
    };
    let prompt = if label.len() > 1 {
        format!(" {label}› ")
    } else {
        format!(" {label}")
    };
    let mut spans = vec![Span::styled(
        prompt.clone(),
        Style::default().fg(p.main).add_modifier(Modifier::BOLD),
    )];
    let draft = grammar.then(|| app.draft(&inp.text));
    // Colour the tokens the grammar recognised.
    let mut pos = 0;
    let text = &inp.text;
    if let Some(d) = &draft {
        for sp in &d.spans {
            if sp.range.start > pos {
                spans.push(Span::styled(
                    text[pos..sp.range.start].to_string(),
                    Style::default().fg(p.fg),
                ));
            }
            let c = match sp.role {
                Role::Date | Role::Time => p.main,
                Role::List => match &d.list {
                    Some(parse::ListPick::Existing(id)) => color(app.s.list_color(id)),
                    _ => p.main,
                },
                Role::Priority | Role::Error => p.error,
                Role::Repeat | Role::Remind | Role::Flag => p.today,
            };
            spans.push(Span::styled(
                text[sp.range.clone()].to_string(),
                Style::default().fg(c).add_modifier(Modifier::BOLD),
            ));
            pos = sp.range.end;
        }
    }
    if pos < text.len() {
        spans.push(Span::styled(
            text[pos..].to_string(),
            Style::default().fg(p.fg),
        ));
    }
    let line_area = Rect { height: 1, ..area };
    f.render_widget(Paragraph::new(Line::from(spans)), line_area);
    let cx = area.x + width(&prompt) as u16 + inp.cursor as u16;
    f.set_cursor_position((cx.min(area.x + area.width.saturating_sub(1)), area.y));
    // The preview line: what this will do, in words.
    let (msg, bad) = preview(app, inp, draft.as_ref());
    let style = if bad {
        Style::default().fg(p.error)
    } else {
        Style::default().fg(p.sub)
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("   {}", truncate(&msg, area.width as usize - 4)),
            style,
        ))),
        Rect {
            y: area.y + 1,
            height: 1,
            ..area
        },
    );
}

fn preview(app: &App, inp: &super::app::Input, draft: Option<&parse::Draft>) -> (String, bool) {
    let fmt = app.s.cfg.time_format;
    if !inp.completions.is_empty() {
        return (inp.completions.join("  "), false);
    }
    match (&inp.kind, draft) {
        (InputKind::Add { event }, Some(d)) => {
            if inp.text.trim().is_empty() {
                return (
                    "type a title, then when: dentist fri 2-3pm #health   (? for the grammar)"
                        .into(),
                    false,
                );
            }
            let default = app.s.cfg.default_list.clone();
            if let Some(prob) = d
                .problem()
                .filter(|p| !p.starts_with(&format!("no list #{default}")))
            {
                return (prob, true);
            }
            let kind = crate::core::ops::kind_for(d, event.then_some(Kind::Event), None);
            let fallback = if app.view == View::Tasks {
                app.tabs().get(app.tab).and_then(|t| t.0.clone())
            } else {
                None
            };
            let list = fallback.unwrap_or(default);
            (d.preview(kind, fmt, Some(&list)), false)
        }
        (InputKind::Edit { uid, .. }, Some(d)) => {
            if let Some(prob) = d.problem() {
                return (prob, true);
            }
            let existing = app.s.store.get(uid).map(|i| i.kind);
            let kind = crate::core::ops::kind_for(d, None, existing);
            (d.preview(kind, fmt, None), false)
        }
        (InputKind::Move { .. }, Some(d)) => {
            if inp.text.trim().is_empty() {
                return (
                    "a date, a time or a #list: fri · 3pm · +2d · #work".into(),
                    false,
                );
            }
            if !d.title.is_empty() {
                return (format!("not a date, time or list: {}", d.title), true);
            }
            let mut parts = Vec::new();
            if let Some(day) = d.date.filter(|_| !d.implied_date) {
                parts.push(day.format("%a %-d %b").to_string());
            }
            if let Some(t) = d.time {
                parts.push(fmt_time(t, fmt));
            }
            if let Some(parse::ListPick::Existing(l) | parse::ListPick::Create(l)) = &d.list {
                parts.push(format!("#{l}"));
            }
            if let Some(prob) = d.problem().filter(|p| p.contains('#')) {
                return (prob, true);
            }
            (format!("→ {}", parts.join(" · ")), false)
        }
        (InputKind::Goto, Some(d)) => match d.date.filter(|_| d.title.is_empty()) {
            Some(day) => (format!("→ {}", day.format("%A %-d %B %Y")), false),
            None if inp.text.trim().is_empty() => {
                ("a date: fri · dec 24 · +2w · 2027-01-01".into(), false)
            }
            None => ("not a date".into(), true),
        },
        (InputKind::Filter, _) => {
            let n = match app.view {
                View::Month => app.day_rows.len(),
                _ => app.rows.iter().filter(|r| r.entry().is_some()).count(),
            };
            (
                format!("{n} shown · enter keeps the filter · esc clears it"),
                false,
            )
        }
        (InputKind::Command, _) => {
            let first = inp.text.split_whitespace().next().unwrap_or("");
            if first.is_empty() {
                return ("any ct command: add, ls, import ~/cal.ics, theme nord, set week_start sunday, lists … · tab completes".into(), false);
            }
            match super::app::command_about(first) {
                Some(a) => (a, false),
                None => {
                    let names: Vec<String> = super::app::command_names()
                        .into_iter()
                        .filter(|c| c.starts_with(first))
                        .collect();
                    if names.is_empty() {
                        ("no such command".into(), true)
                    } else {
                        (names.join("  "), false)
                    }
                }
            }
        }
        _ => (String::new(), false),
    }
}

fn hints(app: &App) -> Vec<(Action, &'static str)> {
    use Action::*;
    if let Mode::Input(_) = app.mode {
        return Vec::new();
    }
    let sel = app.selected();
    let mut v = vec![(Add, "add")];
    match sel {
        Some(e) if e.kind == Kind::Task => v.extend([
            (Done, if e.done { "undo done" } else { "done" }),
            (Edit, "edit"),
        ]),
        Some(_) => v.push((Edit, "edit")),
        None => {}
    }
    if sel.is_some() {
        v.extend([(Later, "+1 day"), (Delete, "delete")]);
    }
    match app.view {
        View::Month => v.extend([(PrevPeriod, "month"), (ItemDown, "pick"), (Today, "today")]),
        View::Tasks => v.extend([(TabNext, "list"), (Sort, "sort")]),
        View::Agenda => v.extend([(Someday, "undated"), (Today, "today")]),
    }
    v.extend([
        (Undo, "undo"),
        (Filter, "filter"),
        (Command, "cmd"),
        (Help, "help"),
        (Quit, "quit"),
    ]);
    v
}

fn draw_foot(f: &mut Frame, app: &App, p: &Pal, area: Rect) {
    if let Some((msg, bad)) = &app.toast {
        let style = if *bad {
            Style::default().fg(p.error)
        } else {
            Style::default().fg(p.fg)
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {}", truncate(msg, area.width as usize - 2)),
                style,
            ))),
            area,
        );
        return;
    }
    if let Mode::Input(inp) = &app.mode {
        let tab = if matches!(
            inp.kind,
            InputKind::Command | InputKind::Add { .. } | InputKind::Edit { .. }
        ) {
            "  tab complete"
        } else {
            ""
        };
        let s = format!(" enter ok  esc cancel{tab}  ctrl+w delete word");
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(s, Style::default().fg(p.sub)))),
            area,
        );
        return;
    }
    let mut spans = vec![Span::raw(" ")];
    let mut used = 1;
    for (a, label) in hints(app) {
        let Some(k) = app.keys.first(a) else { continue };
        let k = if a == Action::PrevPeriod {
            "[ ]".to_string()
        } else if a == Action::ItemDown {
            "J K".into()
        } else {
            k
        };
        let w = width(&k) + width(label) + 3;
        if used + w > area.width as usize {
            break;
        }
        spans.push(Span::styled(k, Style::default().fg(p.main)));
        spans.push(Span::styled(
            format!(" {label}  "),
            Style::default().fg(p.sub),
        ));
        used += w;
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

fn modal_block<'a>(p: &Pal, title: &'a str) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(p.border))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(p.main).add_modifier(Modifier::BOLD),
        ))
        .style(p.base)
}

pub const GRAMMAR_HELP: &[(&str, &str)] = &[
    ("buy milk", "a task with no date"),
    ("pay rent fri !", "due Friday, priority ! (up to !!!)"),
    ("call mum tom 6pm", "due tomorrow at 18:00"),
    (
        "dentist sat 2-3pm #health",
        "an event (a time range makes it one)",
    ),
    (
        "standup 9:30 15m every weekday",
        "an event with a length, repeating",
    ),
    ("trip oct 20-22", "an all-day event over three days"),
    ("birthday allday dec 3 every year", "a yearly all-day event"),
    ("gym every mon,wed 7am", "repeats on those days"),
    ("rent every month on 1", "monthly on the 1st"),
    ("report next fri remind 1h", "a reminder (ct remind)"),
    ("#work  @work", "the list (#name! creates one)"),
    ("today tom mon next fri oct 20 20/10 +3d eom", "dates"),
    ("3pm 15:00 9.30 noon 3-4pm for 45m", "times and lengths"),
    ("\"fri\" night", "quotes keep words as the title"),
];

fn draw_help(f: &mut Frame, app: &App, p: &Pal, scroll: u16) {
    let area = centered(f.area(), 78, f.area().height.saturating_sub(2));
    f.render_widget(Clear, area);
    let block = modal_block(p, "help · j/k scroll · any key closes");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let mut lines: Vec<Line> = Vec::new();
    let head = |t: &str| {
        Line::from(Span::styled(
            t.to_string(),
            Style::default().fg(p.main).add_modifier(Modifier::BOLD),
        ))
    };
    lines.push(head("Quick-add grammar (add, edit, move, go to)"));
    for (ex, what) in GRAMMAR_HELP {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:<36}", truncate(ex, 35)),
                Style::default().fg(p.fg),
            ),
            Span::styled(what.to_string(), Style::default().fg(p.sub)),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(head("Keys (change them under [keys] in config.toml)"));
    for (a, name, _, what) in TABLE {
        let keys = app.keys.all(*a).join(" ");
        if keys.is_empty() {
            continue;
        }
        lines.push(Line::from(vec![
            Span::styled(format!("  {keys:<18}"), Style::default().fg(p.main)),
            Span::styled(format!("{what:<40}"), Style::default().fg(p.fg)),
            Span::styled(name.to_string(), Style::default().fg(p.sub)),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(head("Command line (:)"));
    for (c, what) in [
        (
            ":add …  :ls  :ag  :cal",
            "the same commands as ct in a shell",
        ),
        (":import ~/takeout.zip", "bring a calendar in (one way)"),
        (":theme nord  (or c)", "colour themes, previewed live"),
        (
            ":set week_start sunday",
            "change a setting (:set lists them)",
        ),
        (
            ":lists color work #a3be8c",
            "list colours, names, new lists",
        ),
    ] {
        lines.push(Line::from(vec![
            Span::styled(format!("  {c:<30}"), Style::default().fg(p.fg)),
            Span::styled(what.to_string(), Style::default().fg(p.sub)),
        ]));
    }
    let max = (lines.len() as u16).saturating_sub(inner.height);
    f.render_widget(Paragraph::new(lines).scroll((scroll.min(max), 0)), inner);
}

fn draw_themes(f: &mut Frame, app: &App, p: &Pal, names: &[String], idx: usize) {
    let h = (names.len() as u16 + 2).min(f.area().height.saturating_sub(4));
    let area = Rect {
        x: f.area().width.saturating_sub(40),
        y: 2,
        width: 38.min(f.area().width),
        height: h,
    };
    f.render_widget(Clear, area);
    let block = modal_block(p, "theme · enter keeps · esc");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let rows = inner.height as usize;
    let skip = idx
        .saturating_sub(rows.saturating_sub(1) / 2)
        .min(names.len().saturating_sub(rows));
    let dir = app.s.paths.themes_dir();
    let lines: Vec<Line> = names
        .iter()
        .enumerate()
        .skip(skip)
        .take(rows)
        .map(|(i, n)| {
            let sel = i == idx;
            let mut spans = vec![
                Span::styled(if sel { " › " } else { "   " }, Style::default().fg(p.main)),
                Span::styled(
                    format!("{:<20}", truncate(n, 20)),
                    if sel {
                        Style::default().fg(p.main).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(p.fg)
                    },
                ),
            ];
            if let Ok(t) = theme::load(n, &dir) {
                for c in t.lists.iter().take(6) {
                    spans.push(Span::styled("■", Style::default().fg(color(*c))));
                }
            }
            Line::from(spans)
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_choose(f: &mut Frame, p: &Pal, pending: &Pending) {
    let what = match pending {
        Pending::Edit { .. } => "Edit",
        Pending::Move { .. } | Pending::Shift { .. } => "Move",
        Pending::Delete { .. } => "Delete",
    };
    let area = centered(f.area(), 44, 5);
    f.render_widget(Clear, area);
    let block = modal_block(p, "repeating event");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let lines = vec![
        Line::from(Span::styled(
            format!(" {what} which?"),
            Style::default().fg(p.fg),
        )),
        Line::from(vec![
            Span::styled(
                " t",
                Style::default().fg(p.main).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" this one   ", Style::default().fg(p.fg)),
            Span::styled(
                "a",
                Style::default().fg(p.main).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" all of them   ", Style::default().fg(p.fg)),
            Span::styled("esc", Style::default().fg(p.sub)),
        ]),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_output(f: &mut Frame, p: &Pal, lines: &[String], scroll: u16) {
    let w = lines.iter().map(|l| width(l)).max().unwrap_or(20).max(30) as u16 + 4;
    let area = centered(f.area(), w, lines.len() as u16 + 2);
    f.render_widget(Clear, area);
    let block = modal_block(p, "output · any key closes");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let text: Vec<Line> = lines
        .iter()
        .map(|l| Line::from(Span::styled(l.clone(), Style::default().fg(p.fg))))
        .collect();
    let max = (lines.len() as u16).saturating_sub(inner.height);
    f.render_widget(Paragraph::new(text).scroll((scroll.min(max), 0)), inner);
}
