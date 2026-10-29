//! Pocketdesk Server Library
//!
//! Re-exports for integration testing. The `main.rs` binary uses these same modules.

pub mod alerts;
pub mod approvals;
pub mod asks;
pub mod audio_control;
pub(crate) mod away_lock;
pub mod bench;
pub mod browser_host;
pub mod cast_relay;
pub mod cli;
pub(crate) mod clip_history;
pub(crate) mod clipboard_push;
pub mod computer_view;
pub mod config;
pub mod conn_budget;
pub mod crash_report;
pub mod done;
pub(crate) mod drive;
pub mod driving;
pub mod events;
pub mod gamepad_pad;
pub mod home_assistant;
pub mod hypr_events;
pub mod hyprland;
mod idle_cost;
pub mod inbox;
pub mod input_injector;
#[cfg(windows)]
mod input_pen_windows;
#[cfg(windows)]
mod input_sendinput;
#[cfg(windows)]
pub mod install_windows;
pub mod iroh_endpoint;
pub mod kde;
pub mod kdeconnect_compat;
pub mod laser;
#[cfg(target_os = "linux")]
mod laser_linux;
#[cfg(windows)]
mod laser_windows;
pub mod live_sessions;
pub mod local_api;
pub mod media;
#[cfg(not(windows))]
pub mod mic_relay;
#[cfg(windows)]
#[path = "mic_relay_windows.rs"]
pub mod mic_relay;
pub(crate) mod nearby;
#[cfg(windows)]
mod notification_listener_windows;
pub mod notification_monitor;
pub mod offers;
pub mod overlay;
pub mod packets;
pub mod paused;
pub mod peer_computers;
pub mod pen;
pub mod phone_sound;
pub mod platform;
pub mod plugins;
pub mod privacy;
pub mod quiet_hours;
pub mod quiet_phone;
pub mod relay;
pub mod remote_sound;
pub mod ring_phone;
pub mod service;
pub mod session_telemetry;
pub mod shared_input;
pub mod state;
pub mod system_state;
pub mod tel_handler;
pub mod toast_answers;
pub mod transfers;
pub mod v2_multiplexer;
pub mod virtual_screen;
pub mod wake_info;
#[cfg(windows)]
pub mod windows_desktop;
