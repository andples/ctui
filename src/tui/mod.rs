//! The TUI: terminal setup and the event loop. State is in `app.rs`,
//! drawing in `view.rs`, keys in `keys.rs`.

pub mod app;
pub mod keys;
pub mod view;

use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::cli::Out;
use crate::session::Session;
use app::App;

type Term = Terminal<CrosstermBackend<Stdout>>;

fn enter(mouse: bool) -> Result<Term> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen)?;
    if mouse {
        execute!(out, EnableMouseCapture)?;
    }
    Ok(Terminal::new(CrosstermBackend::new(out))?)
}

fn leave(term: &mut Term, mouse: bool) {
    let _ = disable_raw_mode();
    if mouse {
        let _ = execute!(term.backend_mut(), DisableMouseCapture);
    }
    let _ = execute!(term.backend_mut(), LeaveAlternateScreen);
    let _ = term.show_cursor();
}

pub fn run(session: Session) -> Result<()> {
    let mouse = session.cfg.mouse;
    // Put the terminal back even if something panics.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
        hook(info);
    }));
    let mut term = enter(mouse)?;
    let mut app = App::new(session);
    let result = event_loop(&mut term, &mut app, mouse);
    leave(&mut term, mouse);
    result
}

fn event_loop(term: &mut Term, app: &mut App, mouse: bool) -> Result<()> {
    while !app.quit {
        term.draw(|f| view::draw(f, app))?;
        if let Some(uid) = app.raw_edit.take() {
            leave(term, mouse);
            let msg = raw_edit(app, &uid);
            *term = enter(mouse)?;
            term.clear()?;
            app.after_raw_edit(msg);
            continue;
        }
        if !event::poll(Duration::from_secs(2))? {
            app.tick();
            continue;
        }
        match event::read()? {
            Event::Key(k) if k.kind != KeyEventKind::Release => {
                if month_enter(app, &k) {
                    continue;
                }
                app.key(k);
            }
            Event::Mouse(m) => app.mouse(m),
            Event::Resize(..) => {}
            _ => {}
        }
    }
    Ok(())
}

/// In the month view, enter opens the selected day in the agenda.
fn month_enter(app: &mut App, k: &crossterm::event::KeyEvent) -> bool {
    if app.view == crate::config::View::Month
        && matches!(app.mode, app::Mode::Normal)
        && k.code == crossterm::event::KeyCode::Enter
    {
        let d = app.month_day;
        app.switch_to_day(d);
        return true;
    }
    false
}

/// Runs $EDITOR on an item's file with the terminal handed over.
fn raw_edit(app: &mut App, uid: &str) -> Result<String> {
    let mut buf: Vec<u8> = Vec::new();
    let mut out = Out {
        w: &mut buf,
        color: false,
    };
    crate::cli::edit_uid(&mut app.s, &mut out, uid)?;
    Ok(String::from_utf8_lossy(&buf).trim().to_string())
}

#[cfg(test)]
mod tests;
