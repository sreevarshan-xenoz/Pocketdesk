//! Pocketdesk Server Library
//!
//! Re-exports for integration testing. The `main.rs` binary uses these same modules.

pub mod approvals;
pub mod audio_control;
pub mod bench;
pub mod browser_host;
pub mod cast_relay;
pub mod cli;
pub(crate) mod clipboard_push;
pub mod config;
pub mod conn_budget;
pub mod crash_report;
pub mod driving;
pub mod events;
pub mod home_assistant;
pub mod hypr_events;
pub mod hyprland;
mod idle_cost;
pub mod inbox;
pub mod input_injector;
#[cfg(windows)]
mod input_sendinput;
#[cfg(windows)]
pub mod install_windows;
pub mod iroh_endpoint;
pub mod kde;
pub mod live_sessions;
pub mod local_api;
pub mod media;
#[cfg(not(windows))]
pub mod mic_relay;
#[cfg(windows)]
#[path = "mic_relay_windows.rs"]
pub mod mic_relay;
#[cfg(windows)]
mod notification_listener_windows;
pub mod notification_monitor;
pub mod offers;
pub mod packets;
pub mod platform;
pub mod plugins;
pub mod privacy;
pub mod relay;
pub mod service;
pub mod session_telemetry;
pub mod state;
pub mod v2_multiplexer;
pub mod virtual_screen;
pub mod wake_info;
#[cfg(windows)]
pub mod windows_desktop;
