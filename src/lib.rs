//! ctui: a minimal terminal calendar and task list.

pub mod cli;
pub mod config;
pub mod core;
pub mod mcp;
pub mod session;
pub mod shortcuts;
pub mod theme;
pub mod tui;
#[cfg(target_os = "macos")]
pub mod utheme_client;
