//! Desktop-side privacy mode (Tier-3 #15): while a remote session drives the
//! desktop, block local input, show a "session active" shield, and lock the
//! screen on request.
//!
//! The Linux backend (`linux.rs`) grabs physical devices with `EVIOCGRAB` and
//! paints a layer-shell shield. Other OSes get `unsupported.rs`, which answers
//! every request with a reason instead of pretending, until their backend
//! lands.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(not(target_os = "linux"))]
mod unsupported;
#[cfg(not(target_os = "linux"))]
pub use unsupported::*;
