//! Pocketdesk Server Library
//!
//! Re-exports for integration testing. The `main.rs` binary uses these same modules.

pub mod approvals;
pub mod audio_control;
pub mod bench;
pub mod cli;
pub mod config;
pub mod hypr_events;
pub mod hyprland;
pub mod input_injector;
#[cfg(windows)]
mod input_sendinput;
#[cfg(windows)]
pub mod install_windows;
pub mod iroh_endpoint;
pub mod kde;
pub mod live_sessions;
#[cfg(not(windows))]
pub mod mic_relay;
#[cfg(windows)]
#[path = "mic_relay_windows.rs"]
pub mod mic_relay;
#[cfg(windows)]
mod notification_listener_windows;
pub mod notification_monitor;
pub mod offers;
pub mod platform;
pub mod plugins;
pub mod privacy;
pub mod relay;
pub mod service;
pub mod session_telemetry;
pub mod state;
pub mod v2_multiplexer;
#[cfg(windows)]
pub mod windows_desktop;
