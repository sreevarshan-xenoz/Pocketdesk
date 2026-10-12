//! The server's `config.toml`. Shared with the desktop app, which edits it,
//! so the type lives in `core::local::config`.

pub use linux_link_core::local::config::{Config, ConfigError};
