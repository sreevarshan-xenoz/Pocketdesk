pub mod backoff;
pub mod clipboard;
pub mod command;
pub mod computer_media;
pub mod connection;
pub mod conversation;
pub mod dial;
pub mod file_time;
pub mod kdeconnect;
pub mod link;
pub mod live;
pub mod notification_action;
pub mod partials;
pub mod permissions;
pub mod phone_files;
pub mod phone_media;
pub mod phone_photos;
pub mod quiet;
pub mod relpath;
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
