use std::io::IsTerminal;
use std::process::ExitCode;

use clap::Parser;

use ctui::cli::{self, Ambiguous, Cli, Cmd, Out};
use ctui::config::Paths;
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
    let (paths, cfg) = Paths::resolve(args.config_dir, args.data_dir, args.state_dir)?;
    let mut session = Session::open(paths, cfg)?;
    let cmd = match args.cmd {
        Some(c) => c,
        // No arguments: the TUI, or the Today list when piped.
        None if std::io::stdout().is_terminal() && std::io::stdin().is_terminal() => {
            return ctui::tui::run(session);
        }
        None => Cmd::Today {
            days: None,
            json: false,
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
