//! Input-side types shared by both ends of the wire.
//!
//! The packet shapes live in [`crate::streaming::input_packet`]; the injection
//! backends live in `server`. What belongs here is what neither of them can own
//! without the other having to depend on it — see [`keys`].

pub mod control;
pub mod evdev_codes;
/// The keyboard's and mouse's idle time from the compositor (T8).
#[cfg(all(target_os = "linux", feature = "capture"))]
pub mod idle_notify;
pub mod keys;
/// Typing any character through a Wayland virtual keyboard (roadmap F4).
#[cfg(all(target_os = "linux", feature = "injection"))]
pub mod vkeyboard;
/// Pointer and keys on wlroots without uinput (T10f).
#[cfg(all(target_os = "linux", feature = "injection"))]
pub mod wlr_input;
pub mod xkb_keymap;
