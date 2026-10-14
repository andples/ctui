//! UI-free core: the data model, storage, grammar and queries. The CLI and
//! the TUI are thin layers over this; nothing else touches the files.

pub mod ical;
pub mod import;
pub mod model;
pub mod ops;
pub mod parse;
pub mod query;
pub mod recur;
pub mod remind;
pub mod store;
pub mod time;
pub mod undo;
