pub mod backoff;
pub mod connection;
pub mod kdeconnect;
pub mod permissions;
pub mod v2;

#[cfg(test)]
mod connection_test;
#[cfg(test)]
mod kdeconnect_test;

pub const HANDSHAKE_HELLO: &str = "LINUX_LINK_HELLO 1";
pub const HANDSHAKE_OK: &str = "LINUX_LINK_OK 1";
pub const PROTOCOL_VERSION: &str = "1";

/// ALPN for the v1 (pre-v2-handshake) streaming connection. The v2 media/control
/// multiplexer negotiates [`v2::ALPN_V2`] on the same UDP port; the server offers
/// both and dispatches on what the peer picks.
pub const ALPN_V1_STREAM: &[u8] = b"linux-link-stream";

/// ALPN for the TLS control channel on the control port (phase B). Inside
/// it runs the same handshake and packets the control channel always had.
pub const ALPN_CONTROL: &[u8] = b"linux-link-control/1";

/// The first byte of a TLS handshake record. The control port refuses a
/// connection that starts with anything else.
pub const TLS_HANDSHAKE_RECORD: u8 = 0x16;

/// What a control port answers a client that does not speak TLS.
pub const HANDSHAKE_TLS_REQUIRED: &str = "LINUX_LINK_ERR 1 tls-required";
