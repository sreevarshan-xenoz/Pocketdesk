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

/// Asks for a keyframe come at most this often.
pub const KEYFRAME_ASK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

/// A cast's decode chain, as the server receives it: each frame travels on
/// its own stream, so frames can finish out of order or be lost. A delta
/// frame after a gap means the chain broke, and the phone is asked for a
/// keyframe, at most every [`KEYFRAME_ASK_INTERVAL`].
#[derive(Debug, Default)]
pub struct CastChain {
    last_seq: Option<u64>,
    last_ask: Option<std::time::Instant>,
}

impl CastChain {
    /// A frame arrived; whether to ask for a keyframe now.
    pub fn on_frame(&mut self, keyframe: bool, seq: u64, now: std::time::Instant) -> bool {
        let gap = !keyframe
            && self
                .last_seq
                .is_some_and(|last| seq > last.saturating_add(1));
        // A late frame does not move the chain back.
        self.last_seq = Some(self.last_seq.map_or(seq, |last| last.max(seq)));
        gap && self.ask(now)
    }

    /// A frame was lost on the way to the viewer; whether to ask now.
    pub fn lost(&mut self, now: std::time::Instant) -> bool {
        self.ask(now)
    }

    /// The phone started its encoder again, which counts from 0.
    pub fn restart(&mut self) {
        self.last_seq = None;
    }

    fn ask(&mut self, now: std::time::Instant) -> bool {
        if self
            .last_ask
            .is_some_and(|t| now.duration_since(t) < KEYFRAME_ASK_INTERVAL)
        {
            return false;
        }
        self.last_ask = Some(now);
        true
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

    #[test]
    fn a_delta_after_a_gap_asks_for_a_keyframe_at_most_every_interval() {
        let t0 = std::time::Instant::now();
        let mut chain = CastChain::default();
        assert!(!chain.on_frame(true, 0, t0), "the first keyframe");
        assert!(!chain.on_frame(false, 1, t0), "in order");
        assert!(chain.on_frame(false, 3, t0), "frame 2 is missing");
        assert!(!chain.on_frame(false, 5, t0), "asked just now");
        assert!(!chain.on_frame(false, 2, t0), "a late frame is no gap");
        assert!(!chain.on_frame(false, 6, t0), "the chain did not move back");
        let later = t0 + KEYFRAME_ASK_INTERVAL;
        assert!(!chain.on_frame(true, 9, later), "a keyframe heals the gap");
        assert!(
            chain.on_frame(false, 11, later),
            "another gap, after the interval"
        );
    }

    #[test]
    fn a_restarted_encoder_counts_from_zero_without_a_gap() {
        let t0 = std::time::Instant::now();
        let mut chain = CastChain::default();
        assert!(!chain.on_frame(true, 0, t0));
        assert!(chain.on_frame(false, 40, t0 + KEYFRAME_ASK_INTERVAL));
        chain.restart();
        assert!(!chain.on_frame(false, 1, t0 + KEYFRAME_ASK_INTERVAL * 4));
        assert!(
            chain.lost(t0 + KEYFRAME_ASK_INTERVAL * 8),
            "a lost frame asks"
        );
        assert!(
            !chain.lost(t0 + KEYFRAME_ASK_INTERVAL * 8),
            "but not twice at once"
        );
    }
}
