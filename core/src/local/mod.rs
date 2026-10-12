//! The files the server and the Pocketdesk desktop app share.
//!
//! The app manages the server through the same files the CLI already uses —
//! config, the pairing PIN, the trust store, the live-session mirror, the kick
//! request, the pid file — so it works whether or not the server is running
//! and needs no socket of its own. Everything here is plain std + serde.

pub mod config;
pub mod identity;
pub mod pair_link;
pub mod paths;
pub mod pin;
pub mod server;
pub mod sessions;
