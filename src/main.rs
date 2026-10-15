use std::io::IsTerminal;
use std::process::ExitCode;

use clap::Parser;

use ctui::cli::{self, Ambiguous, Cli, Cmd, Out};
use ctui::config::{Paths, View};
use ctui::session::Session;

fn main() -> ExitCode {
    let args = Cli::parse();
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let code = if e.downcast_ref::<Ambiguous>().is_some() {
                2
            } else {
                1
            };
            eprintln!("ctui: {e:#}");
            ExitCode::from(code)
        }
    }
}

fn run(args: Cli) -> anyhow::Result<()> {
    // Runs at every shell start: print the snippet without loading anything.
    if let Some(Cmd::Init { shell }) = args.cmd {
        print!("{}", ctui::shortcuts::init(shell));
        return Ok(());
    }
    let (paths, cfg) = Paths::resolve(args.config_dir, args.data_dir, args.state_dir)?;
    let mut session = Session::open(paths, cfg)?;
    let cmd = match args.cmd {
        Some(c) => c,
        // No command: the TUI (on --view's page), or that page as text when piped.
        None if std::io::stdout().is_terminal() && std::io::stdin().is_terminal() => {
            return ctui::tui::run(session, args.view);
        }
        None => match args.view.unwrap_or(View::Today) {
            View::Today => Cmd::Today {
                days: None,
                json: false,
            },
            View::Month => Cmd::Cal {
                month: Vec::new(),
                json: false,
            },
            View::Tasks => Cmd::Ls {
                filter: Vec::new(),
                json: false,
            },
        },
    };
    for w in &session.warnings {
        eprintln!("ctui: {w}");
    }
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let mut out = Out {
        w: &mut lock,
        color: cli::want_color(),
    };
    cli::run(&mut session, cmd, &mut out, true)
}
