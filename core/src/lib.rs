//! Pocketdesk Core Library
//!
//! Shared protocol types and cross-platform logic.

pub mod capabilities;
pub mod error;
pub mod input;
#[cfg(feature = "local")]
pub mod local;
pub mod metrics;
pub mod portal_grant;
pub mod protocol;
pub mod secure_fs;
pub mod streaming;
pub mod tailscale;
pub mod version;

/// Protocol version for compatibility checking.
pub const PROTOCOL_VERSION: u32 = 1;

/// Default port for control channel.
pub const DEFAULT_CONTROL_PORT: u16 = 1716;

/// Default port for screen streaming channel.
pub const DEFAULT_STREAMING_PORT: u16 = 4716;
