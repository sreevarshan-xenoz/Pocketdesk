//! The phone's screen on the computer (T10k): what passes between the server,
//! which receives a phone's cast, and the desktop app, which shows it. While
//! a cast is admitted the server listens on loopback and writes `cast.json`
//! (owner-only) naming the port and a token; the app connects, sends the
//! token, and reads framed messages. Both processes run as the owner, and
//! only the owner can read the token.
//!
//! A message is `[type u8][len u32 LE][payload]`.

use std::io::{self, Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::paths::CAST_FILE;

/// Server → app: the cast's size, `[w u16 LE][h u16 LE]`, on every encoder start.
pub const CONFIG: u8 = 1;
/// Server → app: one access unit, `[keyframe u8][seq u64 LE][Annex-B]`.
pub const FRAME: u8 = 2;
/// Server → app: the cast ended.
pub const END: u8 = 3;
/// Server → app: still here (a still phone sends no frames).
pub const HEARTBEAT: u8 = 4;
/// Server → app: whether the phone takes control now (C8), `[on u8]`.
pub const CONTROL_STATE: u8 = 5;
/// App → server: send a keyframe (the decoder started or lost its way).
pub const WANT_KEYFRAME: u8 = 0x11;
/// App → server: a batch of control events for the phone (C8,
/// `streaming::cast_control`), at most [`MAX_CONTROL`] bytes.
pub const CONTROL: u8 = 0x12;

/// The longest control batch the app sends.
pub const MAX_CONTROL: usize = 16 * 1024;

/// The longest message either side takes: a 1920-line keyframe fits well.
pub const MAX_MESSAGE: usize = 4 * 1024 * 1024;

/// The token's length in bytes (hex in `cast.json`).
pub const TOKEN_LEN: usize = 32;

/// One message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Config {
        width: u16,
        height: u16,
    },
    Frame {
        keyframe: bool,
        seq: u64,
        data: Vec<u8>,
    },
    End,
    Heartbeat,
    WantKeyframe,
    ControlState {
        on: bool,
    },
    Control(Vec<u8>),
}

impl Message {
    /// The message as it goes on the socket.
    pub fn encode(&self) -> Vec<u8> {
        let (kind, payload) = match self {
            Message::Config { width, height } => {
                let mut p = Vec::with_capacity(4);
                p.extend_from_slice(&width.to_le_bytes());
                p.extend_from_slice(&height.to_le_bytes());
                (CONFIG, p)
            }
            Message::Frame {
                keyframe,
                seq,
                data,
            } => {
                let mut p = Vec::with_capacity(9 + data.len());
                p.push(u8::from(*keyframe));
                p.extend_from_slice(&seq.to_le_bytes());
                p.extend_from_slice(data);
                (FRAME, p)
            }
            Message::End => (END, Vec::new()),
            Message::Heartbeat => (HEARTBEAT, Vec::new()),
            Message::WantKeyframe => (WANT_KEYFRAME, Vec::new()),
            Message::ControlState { on } => (CONTROL_STATE, vec![u8::from(*on)]),
            Message::Control(batch) => (CONTROL, batch.clone()),
        };
        let mut out = Vec::with_capacity(5 + payload.len());
        out.push(kind);
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&payload);
        out
    }

    /// Read one message; an error for one too long, of an unknown type, or
    /// whose payload does not fit its type.
    pub fn read(from: &mut impl Read) -> io::Result<Self> {
        let mut head = [0u8; 5];
        from.read_exact(&mut head)?;
        let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
        if len > MAX_MESSAGE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "a cast message too large",
            ));
        }
        let mut payload = vec![0u8; len];
        from.read_exact(&mut payload)?;
        let bad = || io::Error::new(io::ErrorKind::InvalidData, "a malformed cast message");
        Ok(match head[0] {
            CONFIG if len == 4 => Message::Config {
                width: u16::from_le_bytes([payload[0], payload[1]]),
                height: u16::from_le_bytes([payload[2], payload[3]]),
            },
            FRAME if len >= 9 => Message::Frame {
                keyframe: payload[0] != 0,
                seq: u64::from_le_bytes(payload[1..9].try_into().map_err(|_| bad())?),
                data: payload[9..].to_vec(),
            },
            END if len == 0 => Message::End,
            HEARTBEAT if len == 0 => Message::Heartbeat,
            WANT_KEYFRAME if len == 0 => Message::WantKeyframe,
            CONTROL_STATE if len == 1 => Message::ControlState {
                on: payload[0] != 0,
            },
            CONTROL if len > 0 && len <= MAX_CONTROL => Message::Control(payload),
            _ => return Err(bad()),
        })
    }

    pub fn write(&self, to: &mut impl Write) -> io::Result<()> {
        to.write_all(&self.encode())
    }
}

/// Whether `given` is `expected`, taking the same time whatever they hold.
pub fn token_matches(expected: &[u8], given: &[u8]) -> bool {
    if expected.len() != given.len() {
        return false;
    }
    expected
        .iter()
        .zip(given)
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

/// `cast.json`: the cast the server holds now, and how the app reaches it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CastFile {
    /// The live session (as in `live_sessions.json`).
    pub session_id: u64,
    pub device_id: String,
    /// The loopback port the server listens on.
    pub port: u16,
    /// [`TOKEN_LEN`] bytes, as hex.
    pub token: String,
}

impl CastFile {
    /// The token's bytes; `None` when the file's is not [`TOKEN_LEN`] bytes of hex.
    pub fn token_bytes(&self) -> Option<Vec<u8>> {
        let hex = self.token.as_bytes();
        if hex.len() != TOKEN_LEN * 2 {
            return None;
        }
        hex.chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
            .collect()
    }
}

/// The token as `cast.json` holds it.
pub fn token_hex(token: &[u8]) -> String {
    token.iter().map(|b| format!("{b:02x}")).collect()
}

/// The cast now, if any.
pub fn read(state_dir: &Path) -> Option<CastFile> {
    let raw = std::fs::read_to_string(state_dir.join(CAST_FILE)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Say where the cast is (the server's side): owner-only, written whole.
pub fn write(state_dir: &Path, cast: &CastFile) -> io::Result<()> {
    let json = serde_json::to_string(cast).map_err(io::Error::other)?;
    let path = state_dir.join(CAST_FILE);
    let tmp = crate::atomic_file::temp_beside(&path);
    std::fs::create_dir_all(state_dir)?;
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    crate::atomic_file::replace(&tmp, &path)
}

/// The cast ended: forget it, if it is still the one with `session_id`.
pub fn remove(state_dir: &Path, session_id: u64) -> io::Result<()> {
    if read(state_dir).is_some_and(|c| c.session_id == session_id) {
        match std::fs::remove_file(state_dir.join(CAST_FILE)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_reads_back_as_it_was_written() {
        let messages = [
            Message::Config {
                width: 864,
                height: 1920,
            },
            Message::Frame {
                keyframe: true,
                seq: 7,
                data: vec![0, 0, 0, 1, 0x67, 1, 2, 3],
            },
            Message::Frame {
                keyframe: false,
                seq: u64::MAX,
                data: Vec::new(),
            },
            Message::End,
            Message::Heartbeat,
            Message::WantKeyframe,
            Message::ControlState { on: true },
            Message::ControlState { on: false },
            Message::Control(vec![1, 2, 3]),
        ];
        let mut wire = Vec::new();
        for m in &messages {
            m.write(&mut wire).unwrap();
        }
        let mut reader = io::Cursor::new(wire);
        for m in &messages {
            assert_eq!(&Message::read(&mut reader).unwrap(), m);
        }
    }

    #[test]
    fn a_message_too_large_unknown_or_malformed_is_refused() {
        let mut huge = vec![FRAME];
        huge.extend_from_slice(&((MAX_MESSAGE + 1) as u32).to_le_bytes());
        assert!(Message::read(&mut io::Cursor::new(huge)).is_err());
        let unknown = [0x7f, 0, 0, 0, 0];
        assert!(Message::read(&mut io::Cursor::new(unknown)).is_err());
        let short_config = [CONFIG, 2, 0, 0, 0, 1, 2];
        assert!(Message::read(&mut io::Cursor::new(short_config)).is_err());
        let short_frame = [FRAME, 1, 0, 0, 0, 1];
        assert!(Message::read(&mut io::Cursor::new(short_frame)).is_err());
    }

    #[test]
    fn a_token_matches_only_itself() {
        let token = [7u8; TOKEN_LEN];
        assert!(token_matches(&token, &token));
        let mut other = token;
        other[TOKEN_LEN - 1] = 8;
        assert!(!token_matches(&token, &other));
        assert!(!token_matches(&token, &token[..TOKEN_LEN - 1]));
        let file = CastFile {
            session_id: 1,
            device_id: "phone".into(),
            port: 1,
            token: token_hex(&token),
        };
        assert_eq!(file.token_bytes().as_deref(), Some(&token[..]));
        let bad = CastFile {
            token: "zz".repeat(TOKEN_LEN),
            ..file
        };
        assert_eq!(bad.token_bytes(), None);
    }

    #[test]
    fn the_file_is_written_read_and_removed_only_for_its_own_session() {
        let dir = std::env::temp_dir().join(format!("pd-cast-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cast = CastFile {
            session_id: 4,
            device_id: "phone".into(),
            port: 50000,
            token: token_hex(&[1u8; TOKEN_LEN]),
        };
        write(&dir, &cast).unwrap();
        assert_eq!(read(&dir), Some(cast.clone()));
        remove(&dir, 5).unwrap();
        assert!(read(&dir).is_some(), "another session's end leaves it");
        remove(&dir, 4).unwrap();
        assert_eq!(read(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
