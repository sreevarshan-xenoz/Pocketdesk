//! The low setting for a session on a narrow road (the Bluetooth fallback,
//! phase 2b): a byte-stream connection (`TransportFamily::ByteStream`,
//! TLS over an RFCOMM channel) carries a megabit or two at best, shared
//! with the clipboard, notifications and the phone's own audio. Its picture
//! is made smaller before the encoder (a whole factor, to about
//! [`HEIGHT`]), its frame rate held to [`FPS`] and its bitrate to
//! [`BITRATE_BPS`]; the session's other ceilings still apply below these.

use super::connection::TransportFamily;

/// The height the picture is brought down to, about.
pub const HEIGHT: u32 = 480;

/// The most frames a second.
pub const FPS: u32 = 15;

/// The most bits a second.
pub const BITRATE_BPS: u32 = 900_000;

/// Whether a session over `family` takes the low setting.
pub fn applies(family: TransportFamily) -> bool {
    family == TransportFamily::ByteStream
}

/// The whole factor a picture `height` pixels high is made smaller by: the
/// largest that keeps it at least [`HEIGHT`] high, 1 for a picture already
/// about that small (1080 by 2 is 540, 1440 by 3 is 480, 2160 by 4 is 540).
pub fn factor(height: u32) -> u32 {
    (height / HEIGHT).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_byte_stream_takes_it() {
        assert!(applies(TransportFamily::ByteStream));
        assert!(!applies(TransportFamily::Quinn));
        assert!(!applies(TransportFamily::Iroh));
    }

    #[test]
    fn the_picture_comes_down_to_about_480_lines() {
        assert_eq!(factor(1080), 2);
        assert_eq!(factor(1440), 3);
        assert_eq!(factor(2160), 4);
        assert_eq!(factor(720), 1);
        assert_eq!(factor(400), 1);
        assert_eq!(factor(0), 1);
    }
}
