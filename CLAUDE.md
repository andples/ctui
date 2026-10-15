# ctui

Minimal terminal calendar + task list in Rust (ratatui + crossterm + clap). Binary `ctui`
(users alias `ct`). The plan, decisions and milestones are in `DESIGN.md` (§12 has the
source layout): read it first and keep it current when a decision changes. The repo is
public; ctui is MIT, on crates.io and in the Homebrew tap andples/homebrew-ctui (clone it at
../homebrew-ctui). Releases need the owner's explicit go-ahead.

Releasing: `scripts/release.sh patch|minor|major` (pipe `y` to its prompt) bumps the version, tags,
creates the GitHub release, waits for `.github/workflows/release.yml` to attach the four binaries
(with the Monkeytype themes as separate GPL files) and rewrites `Formula/ctui.rb` in the tap.
crates.io is separate: `cargo publish`.

Rules that override convenience:
- Usability first, no bloat (DESIGN.md §1, §17); companion apps are fine outside the binary (§16). A new dependency or feature must justify itself.
- One quick-add grammar (`src/core/parse.rs`) shared by CLI, TUI add bar, edit, move and `g`.
- Every data change goes through `core::ops` → `Store::commit` (writes files + undo entry).
- TUI state changes go through `App::dispatch(Action)` / `App::input_key`; `view.rs` only draws.
- Time-dependent code takes `now` as an argument (`Env`, `Opts`); `CTUI_NOW` fixes it for tests.
- `.ics` writes must keep unknown properties: `Fields::write` writes only fields that changed.
- MCP (`src/mcp.rs`) takes structured fields that must map onto `parse::Draft`; `apply_fields`
  destructures `Draft` exhaustively on purpose, so a new grammar field needs MCP support too
  (and a row in `ITEM_FIELDS` plus a case in the grammar-vs-fields test).
- ctui is MIT. The Monkeytype themes (GPL-3.0) are never compiled in or packaged; they load from
  a themes folder at run time. The crate's `include` list in Cargo.toml keeps them out.

Commands: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt`,
`cargo run -- --config-dir /tmp/c --data-dir /tmp/d --state-dir /tmp/s` (sandboxed).
Headless TUI checks: always a private tmux socket (`tmux -L ctui new -d -s t -x 100 -y 30 "..."`,
`tmux -L ctui send-keys`, `tmux -L ctui capture-pane -p`, `tmux -L ctui kill-server`), never
the default server. TUI tests use ratatui's TestBackend (`src/tui/tests.rs`); CLI tests run
the binary (`tests/cli.rs`).

Adding a key: an `Action` variant, a `TABLE` row in `src/tui/keys.rs` (help and `[keys]`
pick it up), a `dispatch` arm, and a `hints()` entry if it belongs on the bottom line.
Adding a command: a `Cmd` variant in `src/cli.rs` and its `run` arm; the TUI's `:` line and
completion get it for free.
Shell shortcuts (`ctui init`, DESIGN.md §6.1): the page→name rule lives only in
`shortcuts::candidates()`; the bash/zsh/fish snippets, `:ctm`-style page words and completion
all come from it. Adding a page means no shortcut code changes. `tests/cli.rs` sources the bash
snippet for real.
