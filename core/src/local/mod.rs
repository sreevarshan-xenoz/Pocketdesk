//! The files the server and the Pocketdesk desktop app share.
//!
//! The app manages the server through the same files the CLI already uses —
//! config, the pairing PIN, the trust store, the live-session mirror, the kick
//! request, session approvals, the pid file — so it works whether or not the
//! server is running and needs no socket of its own. Everything here is plain std + serde.

pub mod alerts;
pub mod answers;
pub mod app_open;
pub mod approval;
pub mod asks;
pub mod cast;
pub mod clip_filter;
pub mod clip_history;
pub mod clip_tool;
pub mod clipboard_files;
pub mod commands;
pub mod computers;
pub mod config;
pub mod deck;
pub mod device_keys;
pub mod diagnostics;
pub mod dials;
pub mod drive;
pub mod events;
pub mod hold_words;
pub mod home_assistant;
pub mod identity;
pub mod kdeconnect;
pub mod kdeconnect_devices;
pub mod link_rules;
pub mod links;
pub mod listening;
pub mod muted_apps;
pub mod network;
pub mod note_clears;
pub mod notes;
pub mod notification_icons;
pub mod outbox;
pub mod pair_link;
pub mod paths;
pub mod pause;
pub mod permission_profiles;
pub mod phone_actions;
pub mod phone_apps;
pub mod phone_contacts;
pub mod phone_files;
pub mod phone_live;
pub mod phone_media;
pub mod phone_notes;
pub mod phone_photos;
pub mod phone_quiet;
pub mod phone_status;
pub mod phone_switches;
pub mod pin;
pub mod quiet;
pub mod quiet_hours;
pub mod received;
pub mod rings;
pub mod send_menu;
pub mod sent;
pub mod server;
pub mod sessions;
pub mod sms;
pub mod tasks;
pub mod tel_ask;
pub mod transfers;
pub mod typing;
pub mod wake;
