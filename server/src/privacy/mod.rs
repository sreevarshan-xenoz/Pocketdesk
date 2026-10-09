//! Desktop-side privacy mode (Tier-3 #15): while a remote session drives the
//! desktop, block local input, show a "session active" shield, and lock the
//! screen on request.
//!
//! The Linux backend (`linux.rs`) grabs physical devices with `EVIOCGRAB` and
//! paints a layer-shell shield; the Windows backend (`windows.rs`) swallows
//! physical input with low-level hooks and paints a capture-excluded ring.
//! Other OSes get `unsupported.rs`, which answers every request with a reason
//! instead of pretending.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(not(any(target_os = "linux", windows)))]
mod unsupported;
#[cfg(not(any(target_os = "linux", windows)))]
pub use unsupported::*;
