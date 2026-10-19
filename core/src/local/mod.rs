//! The files the server and the Pocketdesk desktop app share.
//!
//! The app manages the server through the same files the CLI already uses —
//! config, the pairing PIN, the trust store, the live-session mirror, the kick
//! request, session approvals, the pid file — so it works whether or not the
//! server is running and needs no socket of its own. Everything here is plain std + serde.

pub mod answers;
pub mod approval;
pub mod cast;
pub mod clip_history;
pub mod computers;
pub mod config;
pub mod device_keys;
pub mod diagnostics;
pub mod drive;
pub mod events;
pub mod home_assistant;
pub mod identity;
pub mod kdeconnect;
pub mod kdeconnect_devices;
pub mod links;
pub mod network;
pub mod outbox;
pub mod pair_link;
pub mod paths;
pub mod phone_files;
pub mod phone_media;
pub mod phone_notes;
pub mod phone_photos;
pub mod phone_quiet;
pub mod phone_status;
pub mod pin;
pub mod quiet;
pub mod received;
pub mod rings;
pub mod send_menu;
pub mod sent;
pub mod server;
pub mod sessions;
pub mod sms;
pub mod tasks;
pub mod transfers;
pub mod wake;
