pub mod ask;
pub mod backoff;
pub mod clipboard;
pub mod command;
pub mod computer_media;
pub mod connection;
pub mod conversation;
pub mod dial;
pub mod file_time;
pub mod in_use;
pub mod introductions;
pub mod kdeconnect;
pub mod link;
pub mod live;
pub mod note;
pub mod notification_action;
pub mod partials;
pub mod peek;
pub mod permissions;
pub mod phone_action;
pub mod phone_files;
pub mod phone_media;
pub mod phone_photos;
pub mod phone_screen;
pub mod quiet;
pub mod relpath;
pub mod remote_devices;
pub mod transfer;
pub mod typing;
pub mod v2;

#[cfg(test)]
mod connection_test;
#[cfg(test)]
mod kdeconnect_test;

pub const HANDSHAKE_HELLO: &str = "POCKETDESK_HELLO 1";
pub const HANDSHAKE_OK: &str = "POCKETDESK_OK 1";
pub const PROTOCOL_VERSION: &str = "1";

/// ALPN for the v1 (pre-v2-handshake) streaming connection. The v2 media/control
/// multiplexer negotiates [`v2::ALPN_V2`] on the same UDP port; the server offers
/// both and dispatches on what the peer picks.
pub const ALPN_V1_STREAM: &[u8] = b"pocketdesk-stream";

/// ALPN for the TLS control channel on the control port (phase B). Inside
/// it runs the same handshake and packets the control channel always had.
pub const ALPN_CONTROL: &[u8] = b"pocketdesk-control/1";

/// ALPN for the control channel between two computers over iroh (BK11):
/// the same channel as [`ALPN_CONTROL`], with the same handshake and the
/// same trust checks, on a congestion controller that does not pace a quiet
/// link (Cubic, not BBR), so a key after a pause leaves at once. A
/// computer dials it first and offers [`ALPN_CONTROL`] beside it, so an
/// older computer still answers. It grants nothing [`ALPN_CONTROL`] does
/// not.
pub const ALPN_COMPUTER: &[u8] = b"pocketdesk-computer/1";

/// Whether a connection that negotiated `alpn` carries the control channel:
/// a phone's or a computer's ([`ALPN_CONTROL`]), or a computer's on its
/// own controller ([`ALPN_COMPUTER`]). Both go through the same gates.
pub fn is_control_alpn(alpn: &[u8]) -> bool {
    alpn == ALPN_CONTROL || alpn == ALPN_COMPUTER
}

/// ALPN for a file transfer's own TLS connection (phase C). Version 2 is
/// the resumable, hash-checked exchange in [`transfer`] (T6a); a peer that
/// speaks only version 1 fails the handshake instead of misreading it.
pub const ALPN_TRANSFER: &[u8] = b"pocketdesk-transfer/2";

/// The first byte of a TLS handshake record. The control port refuses a
/// connection that starts with anything else.
pub const TLS_HANDSHAKE_RECORD: u8 = 0x16;

/// What a control port answers a client that does not speak TLS.
pub const HANDSHAKE_TLS_REQUIRED: &str = "POCKETDESK_ERR 1 tls-required";

/// What the control channel over the internet (T10a) answers a phone it
/// does not admit, in place of the desktop's identity.
pub const HANDSHAKE_REFUSED: &str = "POCKETDESK_ERR 1 refused";
