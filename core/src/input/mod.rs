//! Input-side types shared by both ends of the wire.
//!
//! The packet shapes live in [`crate::streaming::input_packet`]; the injection
//! backends live in `server`. What belongs here is what neither of them can own
//! without the other having to depend on it — see [`keys`].

pub mod control;
pub mod evdev_codes;
pub mod keys;
/// Typing any character through a Wayland virtual keyboard (roadmap F4).
#[cfg(all(target_os = "linux", feature = "injection"))]
pub mod vkeyboard;
pub mod xkb_keymap;
