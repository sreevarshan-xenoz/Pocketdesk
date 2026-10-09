//! Desktop audio control for the phone (Tier-3 #16): the default output's
//! volume and mute, and which output device is the default.
//!
//! Every function blocks; callers use `spawn_blocking`. The Linux backend
//! (`linux.rs`) drives wpctl/pactl; the Windows backend (`windows.rs`) uses
//! the Core Audio endpoint APIs. Both answer with the same JSON shapes.

#[cfg(not(windows))]
mod linux;
#[cfg(not(windows))]
pub use linux::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;
