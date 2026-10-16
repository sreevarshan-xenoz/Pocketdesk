//! The phone's screen on the computer (T10k): what the phone sends up a cast
//! session, and what the server hands its relay. A cast session is one with
//! [`super::client::SESSION_MODE_CAST`]; the phone sends its screen as
//! uni streams, each starting with a marker no input packet starts with:
//!
//! - [`CAST_CONFIG`] `+ w u16 LE + h u16 LE`, on every encoder start;
//! - [`CAST_FRAME`] `+ keyframe u8 + seq u64 LE + Annex-B access unit`.

/// The size of the phone's screen as it is encoded now.
pub const CAST_CONFIG: [u8; 2] = [0xFA, 0x01];
/// One encoded access unit.
pub const CAST_FRAME: [u8; 2] = [0xFA, 0x00];

/// The first byte of every cast stream; no input packet tag is this high.
pub const CAST_PREFIX: u8 = 0xFA;

/// A cast stream's largest size: a keyframe at 1920 lines fits well.
pub const MAX_CAST_STREAM: usize = 2 * 1024 * 1024;

/// What one cast stream carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CastPayload {
    Config {
        width: u16,
        height: u16,
    },
    Frame {
        keyframe: bool,
        seq: u64,
        data: Vec<u8>,
    },
}

/// The stream the phone sends for a config.
pub fn config_bytes(width: u16, height: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(6);
    out.extend_from_slice(&CAST_CONFIG);
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out
}

/// The stream the phone sends for an access unit.
pub fn frame_bytes(keyframe: bool, seq: u64, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(11 + data.len());
    out.extend_from_slice(&CAST_FRAME);
    out.push(u8::from(keyframe));
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(data);
    out
}

/// A whole cast stream as the server read it; `None` for one that is not a
/// well-formed config or frame.
pub fn parse(stream: &[u8]) -> Option<CastPayload> {
    match stream.get(..2)? {
        m if m == CAST_CONFIG && stream.len() == 6 => Some(CastPayload::Config {
            width: u16::from_le_bytes([stream[2], stream[3]]),
            height: u16::from_le_bytes([stream[4], stream[5]]),
        }),
        m if m == CAST_FRAME && stream.len() >= 11 => Some(CastPayload::Frame {
            keyframe: stream[2] != 0,
            seq: u64::from_le_bytes(stream[3..11].try_into().ok()?),
            data: stream[11..].to_vec(),
        }),
        _ => None,
    }
}

/// What the server tells its relay about the cast sessions it runs.
#[derive(Debug)]
pub enum CastEvent {
    /// A cast session was admitted. A send on `want_keyframe` asks the phone
    /// for a keyframe (a viewer joined or lost its way).
    Start {
        session_id: u64,
        device_id: Option<String>,
        want_keyframe: tokio::sync::mpsc::Sender<()>,
    },
    /// Its config or one of its frames.
    Payload {
        session_id: u64,
        payload: CastPayload,
    },
    /// It ended.
    End { session_id: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_config_and_a_frame_read_back_as_sent() {
        assert_eq!(
            parse(&config_bytes(864, 1920)),
            Some(CastPayload::Config {
                width: 864,
                height: 1920
            })
        );
        let au = [0, 0, 0, 1, 0x65, 9, 9];
        assert_eq!(
            parse(&frame_bytes(true, 42, &au)),
            Some(CastPayload::Frame {
                keyframe: true,
                seq: 42,
                data: au.to_vec()
            })
        );
    }

    #[test]
    fn what_is_not_a_cast_stream_is_not_read_as_one() {
        assert_eq!(parse(&[]), None);
        assert_eq!(parse(&[0xFA, 0x01, 1, 2]), None, "a short config");
        assert_eq!(parse(&[0xFA, 0x00, 1, 2, 3]), None, "a short frame");
        assert_eq!(parse(&[0xFA, 0x07, 0, 0, 0, 0]), None, "an unknown marker");
        // No input packet tag reaches the cast prefix.
        assert!(super::super::input_packet::InputPacket::decode(&[CAST_PREFIX]).is_err());
    }
}
