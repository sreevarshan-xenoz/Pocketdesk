//! The files the server and the Pocketdesk desktop app share.
//!
//! The app manages the server through the same files the CLI already uses —
//! config, the pairing PIN, the trust store, the live-session mirror, the kick
//! request, session approvals, the pid file — so it works whether or not the
//! server is running and needs no socket of its own. Everything here is plain std + serde.

pub mod approval;
pub mod config;
pub mod device_keys;
pub mod diagnostics;
pub mod identity;
pub mod links;
pub mod network;
pub mod outbox;
pub mod pair_link;
pub mod phone_status;
pub mod paths;
pub mod pin;
pub mod received;
pub mod sent;
pub mod server;
pub mod sessions;
pub mod sms;
pub mod wake;
