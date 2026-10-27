//! `ffmpeg-next` for core's in-process encoder on Windows (BG8).
//!
//! Core names `ffmpeg-next` directly on Linux, and Cargo will not let one
//! crate depend on it twice under two names, so the Windows build reaches it
//! through this crate, which only the `inproc-windows` feature builds.

pub use ffmpeg_next;
