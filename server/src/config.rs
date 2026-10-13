//! The server's `config.toml`. Shared with the desktop app, which edits it,
//! so the type lives in `core::local::config`.

pub use pocketdesk_core::local::config::{Config, ConfigError};
