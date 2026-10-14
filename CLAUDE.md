# ctui

Minimal terminal calendar + task list in Rust (ratatui + crossterm + clap). Binary `ctui`
(users alias `ct`). The full plan, decisions and milestones are in `DESIGN.md`: read it
first and keep it current when a decision changes.

Rules that override convenience:
- Usability first, no bloat (DESIGN.md §1, §15). A new dependency or feature must justify itself.
- One quick-add grammar (`core/parse.rs`) shared by CLI, TUI add bar, edit and `g`.
- Every data change is a `core::ops::Op` applied through `ops::apply` (writes file + undo).
- TUI state changes go through `App::dispatch(Action)`; views only render.
- Time-dependent code takes `now` as an argument.
- `.ics` writes must keep unknown properties (round-trip tests).

Commands: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt`,
`cargo run -- --config-dir /tmp/c --data-dir /tmp/d --state-dir /tmp/s` (sandboxed, once M0 lands).
Headless TUI checks: always a private tmux socket (`tmux -L ctui ...`), never the default server.
