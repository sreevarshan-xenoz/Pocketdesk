//! The phone's screen on the computer (T10k): what the phone sends up a cast
//! session, and what the server hands its relay. A cast session is one with
//! [`super::client::SESSION_MODE_CAST`]; the phone sends its screen as
//! uni streams, each starting with a marker no input packet starts with:
//!
//! - [`CAST_CONFIG`] `+ w u16 LE + h u16 LE`, on every encoder start;
//! - [`CAST_FRAME`] `+ keyframe u8 + seq u64 LE + Annex-B access unit`;
//! - [`CAST_CONTROL_STATE`] `+ flags u8` (C8: bit 0, the computer may
//!   control the phone now).

/// The size of the phone's screen as it is encoded now.
pub const CAST_CONFIG: [u8; 2] = [0xFA, 0x01];
/// One encoded access unit.
pub const CAST_FRAME: [u8; 2] = [0xFA, 0x00];
/// Whether the phone takes control from the computer now (C8).
pub const CAST_CONTROL_STATE: [u8; 2] = [0xFA, 0x02];
/// A stream of the phone's sound (K5): an Opus packet follows.
pub const CAST_AUDIO: [u8; 2] = [CAST_PREFIX, 0x03];

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
    /// The phone takes control now, or no longer (C8).
    ControlState {
        on: bool,
    },
    /// One Opus packet of what the phone's apps play (K5): 48 kHz stereo.
    Audio {
        opus: Vec<u8>,
    },
}

/// The largest Opus packet a cast's sound may carry: 20 ms at 96 kbit/s
/// is about 240 bytes, and Opus allows at most 1275 a frame.
pub const MAX_CAST_AUDIO: usize = 4000;

/// The stream the phone sends for one Opus packet of its sound (K5).
pub fn audio_bytes(opus: &[u8]) -> Vec<u8> {
    let mut out = CAST_AUDIO.to_vec();
    out.extend_from_slice(opus);
    out
}

/// The stream the phone sends to say whether it takes control (C8).
pub fn control_state_bytes(on: bool) -> Vec<u8> {
    let mut out = CAST_CONTROL_STATE.to_vec();
    out.push(u8::from(on));
    out
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
        m if m == CAST_CONTROL_STATE && stream.len() == 3 => Some(CastPayload::ControlState {
            on: stream[2] & 1 != 0,
        }),
        m if m == CAST_AUDIO && stream.len() > 2 && stream.len() <= 2 + MAX_CAST_AUDIO => {
            Some(CastPayload::Audio {
                opus: stream[2..].to_vec(),
            })
        }
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
        /// Control batches for the phone (C8), sent while it said it takes
        /// them.
        control: tokio::sync::mpsc::Sender<Vec<u8>>,
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
    /// K5: the phone's sound travels as its own cast stream, within a size.
    #[test]
    fn a_cast_carries_its_sound() {
        let opus = vec![0xFC, 1, 2, 3];
        assert_eq!(
            parse(&audio_bytes(&opus)),
            Some(CastPayload::Audio { opus: opus.clone() })
        );
        assert_eq!(parse(&CAST_AUDIO), None, "empty");
        assert!(parse(&audio_bytes(&vec![0; MAX_CAST_AUDIO])).is_some());
        assert_eq!(
            parse(&audio_bytes(&vec![0; MAX_CAST_AUDIO + 1])),
            None,
            "too large"
        );
    }

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
    fn the_phone_says_whether_it_takes_control() {
        assert_eq!(
            parse(&control_state_bytes(true)),
            Some(CastPayload::ControlState { on: true })
        );
        assert_eq!(
            parse(&control_state_bytes(false)),
            Some(CastPayload::ControlState { on: false })
        );
    }

    #[test]
    fn what_is_not_a_cast_stream_is_not_read_as_one() {
        assert_eq!(parse(&[]), None);
        assert_eq!(parse(&[0xFA, 0x01, 1, 2]), None, "a short config");
        assert_eq!(parse(&[0xFA, 0x00, 1, 2, 3]), None, "a short frame");
        assert_eq!(parse(&[0xFA, 0x07, 0, 0, 0, 0]), None, "an unknown marker");
        assert_eq!(
            parse(&[0xFA, 0x02]),
            None,
            "a control state without its flags"
        );
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
