//! Another of the owner's computers' screens in a window on this one (BK7):
//! what passes between the server, which holds the session with the other
//! computer, and the desktop app, which shows it. As for a phone's cast
//! ([`super::cast`]): while a view runs the server listens on loopback and
//! writes `view.json` (owner-only) naming the port and a token; the app
//! connects, sends the token, and reads framed messages. The newest viewer
//! replaces an older one. A view and a phone's cast can run at once, so each
//! has its own file.
//!
//! A message is `[type u8][len u32 LE][payload]`, as the cast's.

use std::io::{self, Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::cast::{MAX_MESSAGE, TOKEN_LEN};
use super::paths::VIEW_FILE;
use crate::streaming::InputPacket;

/// Server → app: the picture's size, `[w u16 LE][h u16 LE]`, read from the
/// stream's SPS, before the first frame and whenever it changes.
pub const CONFIG: u8 = 1;
/// Server → app: one access unit, `[keyframe u8][seq u64 LE][Annex-B]`.
pub const FRAME: u8 = 2;
/// Server → app: the view ended, and why in words (UTF-8, may be empty).
pub const END: u8 = 3;
/// Server → app: still here (an idle screen sends few frames).
pub const HEARTBEAT: u8 = 4;
/// Server → app: how the view stands ([`ViewStatus`], as JSON).
pub const STATUS: u8 = 5;
/// Server → app: the picture comes as HEVC from the next frame, a keyframe
/// with new parameter sets (PC5); no payload. Its size is the first
/// decoded picture's. Sent only for a view that offered HEVC.
pub const HEVC_CONFIG: u8 = 6;
/// App → server: send a keyframe (the decoder started or lost its way).
pub const WANT_KEYFRAME: u8 = 0x11;
/// App → server: one input packet for the other computer
/// ([`InputPacket::encode`]), at most [`MAX_INPUT`] bytes.
pub const INPUT: u8 = 0x12;

/// App → server: the window gained (`1`) or lost (`0`) the focus while it
/// controls the other computer (PC3), `[focused u8]`: the clipboard follows.
pub const FOCUS: u8 = 0x13;

/// App → server: the window's privacy ask for the other computer (PC8),
/// `[ask u8]`: [`PRIVACY_HIDE`] to hide what is done there (the host's own
/// privacy mode), [`PRIVACY_SHOW`] to end it, [`PRIVACY_LOCK`] to end it and
/// lock that computer as the window closes. No other value.
pub const PRIVACY: u8 = 0x14;
pub const PRIVACY_SHOW: u8 = 0;
pub const PRIVACY_HIDE: u8 = 1;
pub const PRIVACY_LOCK: u8 = 2;

/// App → server: the window locked (`1`) or freed (`0`) its pointer (PC7),
/// `[locked u8]`. Only while the window's latest word is "locked" does the
/// relay pass on relative moves ([`InputPacket::MouseMove`]), and the shared
/// keyboard and mouse here leave the screens' edges alone meanwhile.
pub const POINTER_LOCK: u8 = 0x15;

/// App → server: the window's "Send my mic" went on (`1`) or off (`0`)
/// (PC10), `[on u8]`. Only while the window's latest word on this
/// connection is "on" does the relay pass its [`InputPacket::Mic`] frames
/// on; as it goes off, or the connection ends with it on, the relay sends
/// the other computer the mic's stop itself.
pub const MIC: u8 = 0x16;

/// The longest input packet the app sends (a pasted text).
pub const MAX_INPUT: usize = 16 * 1024;
/// The longest status or reason.
pub const MAX_WORDS: usize = 4 * 1024;

/// How the view stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Dialling the other computer.
    #[default]
    Connecting,
    /// Connected; the other computer has not let this one in yet (its
    /// owner may be asked first).
    Waiting,
    /// Let in: the picture comes.
    Showing,
}

/// What the window says about the view, as the server knows it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewStatus {
    #[serde(default)]
    pub phase: Phase,
    /// The road: `home network` or `internet`, once connected.
    #[serde(default)]
    pub route: String,
    /// Whether the other computer takes this one's input: `None` until it
    /// says (its Devices page's Control switch for this computer).
    #[serde(default)]
    pub control: Option<bool>,
    /// The other computer's screens, by name, when known (empty: not known).
    #[serde(default)]
    pub monitors: Vec<String>,
    /// The screen shown.
    #[serde(default)]
    pub monitor: u32,
    /// What keeps the picture from showing, when the server knows (BK7
    /// review M6: frames too large for the window).
    #[serde(default)]
    pub note: Option<String>,
    /// The view dropped and the server is dialling the other computer again
    /// by itself (PC2): the try it is on, from 1; 0 when it is not.
    #[serde(default)]
    pub reconnecting: u32,
    /// How the picture comes, about once a second while connected (PC2);
    /// `None` before.
    #[serde(default)]
    pub stats: Option<ViewStats>,
    /// Whether the other computer runs Windows (else Linux), as its screen
    /// session said (PC4): the window's Send keys menu offers that system's
    /// keys. `None` until it says, or from a computer before PC4.
    #[serde(default)]
    pub windows: Option<bool>,
    /// Whether the other computer hides what the window does there (PC8):
    /// its privacy mode, asked from the window's bar. Off for a server
    /// before PC8.
    #[serde(default)]
    pub privacy: ViewPrivacy,
}

/// The other computer's privacy mode, as the window asked for it (PC8).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewPrivacy {
    /// Not asked for, or ended by the window.
    #[default]
    Off,
    /// Asked; that computer has not answered yet.
    Asking,
    /// That computer blocks its own keyboard and mouse and shows its ring.
    On,
    /// Off, and why: that computer refused it, did not answer, or it ended
    /// (the link or the view dropped, or control was taken away).
    Refused(String),
}

/// How a view's picture comes, as the server measures it (PC2). Each field
/// has a default, so either side may know more fields than the other.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewStats {
    /// Frames that came from the other computer in the last second.
    #[serde(default)]
    pub fps: u32,
    /// The picture's bitrate over the last second, in kilobits a second.
    #[serde(default)]
    pub kbps: u32,
    /// The connection's round trip, when known.
    #[serde(default)]
    pub rtt_ms: Option<u32>,
    /// Whether the internet road rides a relay now (it may turn direct
    /// later); never on the home network.
    #[serde(default)]
    pub relayed: bool,
    /// How long the road that won took to reach the other computer.
    #[serde(default)]
    pub dial_ms: Option<u32>,
    /// Since the view connected: frames that never came (gaps in their
    /// numbers), frames that came after a newer one (dropped), and frames
    /// dropped here (too large, or the window slow to read).
    #[serde(default)]
    pub lost: u64,
    #[serde(default)]
    pub late: u64,
    #[serde(default)]
    pub dropped: u64,
    /// The picture's codec, as the window should name it ("H.264").
    #[serde(default)]
    pub codec: String,
}

/// One message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Config {
        width: u16,
        height: u16,
    },
    /// The stream is HEVC from here (PC5).
    HevcConfig,
    Frame {
        keyframe: bool,
        seq: u64,
        data: Vec<u8>,
    },
    End {
        why: String,
    },
    Heartbeat,
    Status(ViewStatus),
    WantKeyframe,
    Input(Vec<u8>),
    Focus(bool),
    /// [`PRIVACY_SHOW`], [`PRIVACY_HIDE`] or [`PRIVACY_LOCK`].
    Privacy(u8),
    /// The window's pointer locked or freed (PC7).
    PointerLock(bool),
    /// The window's mic on or off (PC10).
    Mic(bool),
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
            Message::End { why } => (END, cut(why).as_bytes().to_vec()),
            Message::Heartbeat => (HEARTBEAT, Vec::new()),
            Message::HevcConfig => (HEVC_CONFIG, Vec::new()),
            Message::Status(status) => (STATUS, serde_json::to_vec(status).unwrap_or_default()),
            Message::WantKeyframe => (WANT_KEYFRAME, Vec::new()),
            Message::Input(packet) => (INPUT, packet.clone()),
            Message::Focus(focused) => (FOCUS, vec![u8::from(*focused)]),
            Message::Privacy(ask) => (PRIVACY, vec![*ask]),
            Message::PointerLock(locked) => (POINTER_LOCK, vec![u8::from(*locked)]),
            Message::Mic(on) => (MIC, vec![u8::from(*on)]),
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
                "a view message too large",
            ));
        }
        let mut payload = vec![0u8; len];
        from.read_exact(&mut payload)?;
        Self::parse(head[0], payload)
    }

    /// The message of type `kind` with `payload`.
    pub fn parse(kind: u8, payload: Vec<u8>) -> io::Result<Self> {
        let bad = || io::Error::new(io::ErrorKind::InvalidData, "a malformed view message");
        let len = payload.len();
        Ok(match kind {
            CONFIG if len == 4 => Message::Config {
                width: u16::from_le_bytes([payload[0], payload[1]]),
                height: u16::from_le_bytes([payload[2], payload[3]]),
            },
            FRAME if len >= 9 => Message::Frame {
                keyframe: payload[0] != 0,
                seq: u64::from_le_bytes(payload[1..9].try_into().map_err(|_| bad())?),
                data: payload[9..].to_vec(),
            },
            END if len <= MAX_WORDS => Message::End {
                why: String::from_utf8(payload).map_err(|_| bad())?,
            },
            HEARTBEAT if len == 0 => Message::Heartbeat,
            HEVC_CONFIG if len == 0 => Message::HevcConfig,
            STATUS if len <= MAX_WORDS => {
                Message::Status(serde_json::from_slice(&payload).map_err(|_| bad())?)
            }
            WANT_KEYFRAME if len == 0 => Message::WantKeyframe,
            INPUT if len > 0 && len <= MAX_INPUT => Message::Input(payload),
            FOCUS if len == 1 && payload[0] <= 1 => Message::Focus(payload[0] == 1),
            PRIVACY if len == 1 && payload[0] <= PRIVACY_LOCK => Message::Privacy(payload[0]),
            POINTER_LOCK if len == 1 && payload[0] <= 1 => Message::PointerLock(payload[0] == 1),
            MIC if len == 1 && payload[0] <= 1 => Message::Mic(payload[0] == 1),
            _ => return Err(bad()),
        })
    }

    pub fn write(&self, to: &mut impl Write) -> io::Result<()> {
        to.write_all(&self.encode())
    }
}

/// `words` cut to [`MAX_WORDS`] bytes, at a character.
fn cut(words: &str) -> &str {
    if words.len() <= MAX_WORDS {
        return words;
    }
    let mut end = MAX_WORDS;
    while !words.is_char_boundary(end) {
        end -= 1;
    }
    &words[..end]
}

/// Whether the window may send `packet` to the other computer: the pointer,
/// the keys, typed text, a keyframe ask, the quality preset and view-only;
/// relative moves only while its pointer is locked (`pointer_locked`, as
/// the window last said with [`POINTER_LOCK`], PC7); and its mic's frames
/// only while its "Send my mic" is on (`mic_on`, as it last said with
/// [`MIC`], PC10). Nothing that measures the link, nor a pen, a pad, a crop
/// or the laser: those are a phone's.
pub fn input_allowed(packet: &InputPacket, pointer_locked: bool, mic_on: bool) -> bool {
    match packet {
        InputPacket::MouseMove { .. } => pointer_locked,
        InputPacket::Mic { .. } => mic_on,
        _ => matches!(
            packet,
            InputPacket::MouseMoveAbs { .. }
                | InputPacket::MouseClick { .. }
                | InputPacket::MouseScroll { .. }
                | InputPacket::KeyEvent { .. }
                | InputPacket::Text(_)
                | InputPacket::RequestKeyframe
                | InputPacket::QualityPreset { .. }
                | InputPacket::ViewOnly { .. }
        ),
    }
}

/// `view.json`: the view the server holds now, and how the app reaches it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewFile {
    /// This view's number, new for each (a reconnect, another screen).
    pub session_id: u64,
    /// The other computer, and its name as this one keeps it.
    pub device_id: String,
    pub name: String,
    /// The loopback port the server listens on.
    pub port: u16,
    /// [`TOKEN_LEN`] bytes, as hex.
    pub token: String,
}

impl ViewFile {
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

/// The view now, if any.
pub fn read(state_dir: &Path) -> Option<ViewFile> {
    let raw = std::fs::read_to_string(state_dir.join(VIEW_FILE)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Say where the view is (the server's side): owner-only, written whole.
pub fn write(state_dir: &Path, view: &ViewFile) -> io::Result<()> {
    let json = serde_json::to_string(view).map_err(io::Error::other)?;
    let path = state_dir.join(VIEW_FILE);
    let tmp = path.with_extension("json.tmp");
    std::fs::create_dir_all(state_dir)?;
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &path)
}

/// The view ended: forget it, if it is still the one with `session_id`.
pub fn remove(state_dir: &Path, session_id: u64) -> io::Result<()> {
    if read(state_dir).is_some_and(|v| v.session_id == session_id) {
        match std::fs::remove_file(state_dir.join(VIEW_FILE)) {
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
                width: 1920,
                height: 1080,
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
            Message::End {
                why: "Lost the connection".into(),
            },
            Message::End { why: String::new() },
            Message::Heartbeat,
            Message::HevcConfig,
            Message::Status(ViewStatus {
                phase: Phase::Showing,
                route: "home network".into(),
                control: Some(false),
                monitors: vec!["DP-1".into(), "HDMI-A-1".into()],
                monitor: 1,
                note: Some("Its frames are too large to show here.".into()),
                windows: Some(true),
                ..ViewStatus::default()
            }),
            Message::Status(ViewStatus::default()),
            Message::WantKeyframe,
            Message::Input(
                InputPacket::KeyEvent {
                    key: 28,
                    pressed: true,
                }
                .encode(),
            ),
            Message::Focus(true),
            Message::Focus(false),
            Message::Privacy(PRIVACY_SHOW),
            Message::Privacy(PRIVACY_HIDE),
            Message::Privacy(PRIVACY_LOCK),
            Message::PointerLock(true),
            Message::PointerLock(false),
            Message::Mic(true),
            Message::Mic(false),
            Message::Status(ViewStatus {
                privacy: ViewPrivacy::Refused("Power is off".into()),
                ..ViewStatus::default()
            }),
            Message::Status(ViewStatus {
                privacy: ViewPrivacy::On,
                ..ViewStatus::default()
            }),
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
        assert!(Message::read(&mut io::Cursor::new([0x7f, 0, 0, 0, 0])).is_err());
        assert!(Message::read(&mut io::Cursor::new([CONFIG, 2, 0, 0, 0, 1, 2])).is_err());
        assert!(
            Message::read(&mut io::Cursor::new([INPUT, 0, 0, 0, 0])).is_err(),
            "empty"
        );
        let mut long = vec![INPUT];
        long.extend_from_slice(&((MAX_INPUT + 1) as u32).to_le_bytes());
        long.resize(5 + MAX_INPUT + 1, 0);
        assert!(Message::read(&mut io::Cursor::new(long)).is_err());
        // PC5: HEVC's config carries nothing.
        assert!(Message::parse(HEVC_CONFIG, vec![0]).is_err());
        // PC3: a focus is one byte, 0 or 1.
        assert!(Message::parse(FOCUS, Vec::new()).is_err());
        assert!(Message::parse(FOCUS, vec![2]).is_err());
        assert!(Message::parse(FOCUS, vec![1, 0]).is_err());
        // PC8: a privacy ask is one byte, show, hide or lock, nothing else.
        assert!(Message::parse(PRIVACY, Vec::new()).is_err());
        assert!(Message::parse(PRIVACY, vec![3]).is_err());
        assert!(Message::parse(PRIVACY, vec![0xff]).is_err());
        assert!(Message::parse(PRIVACY, vec![1, 1]).is_err());
        // PC7: a pointer lock is one byte, 0 or 1.
        assert!(Message::parse(POINTER_LOCK, Vec::new()).is_err());
        assert!(Message::parse(POINTER_LOCK, vec![2]).is_err());
        assert!(Message::parse(POINTER_LOCK, vec![1, 0]).is_err());
        // PC10: the mic's word is one byte, 0 or 1.
        assert!(Message::parse(MIC, Vec::new()).is_err());
        assert!(Message::parse(MIC, vec![2]).is_err());
        assert!(Message::parse(MIC, vec![1, 0]).is_err());
        let not_json = Message::parse(STATUS, b"{nope".to_vec());
        assert!(not_json.is_err());
        // A status from a later server, with more to say, still reads.
        let later = Message::parse(
            STATUS,
            br#"{"phase":"waiting","route":"internet","more":1}"#.to_vec(),
        )
        .unwrap();
        assert_eq!(
            later,
            Message::Status(ViewStatus {
                phase: Phase::Waiting,
                route: "internet".into(),
                ..ViewStatus::default()
            })
        );
    }

    /// PC2: the stats and the reconnect count read back as written, and a
    /// status from a server before them (or an app before them reading a
    /// later one) still reads, with none.
    #[test]
    fn the_stats_and_a_reconnect_read_back_and_are_optional() {
        let status = ViewStatus {
            phase: Phase::Connecting,
            route: "internet".into(),
            reconnecting: 3,
            stats: Some(ViewStats {
                fps: 59,
                kbps: 8_400,
                rtt_ms: Some(23),
                relayed: true,
                dial_ms: Some(640),
                lost: 2,
                late: 1,
                dropped: 4,
                codec: "H.264".into(),
            }),
            ..ViewStatus::default()
        };
        let wire = Message::Status(status.clone()).encode();
        assert_eq!(
            Message::read(&mut io::Cursor::new(wire)).unwrap(),
            Message::Status(status)
        );
        let before = Message::parse(
            STATUS,
            br#"{"phase":"showing","route":"home network"}"#.to_vec(),
        )
        .unwrap();
        let Message::Status(before) = before else {
            panic!("a status");
        };
        assert_eq!((before.reconnecting, before.stats), (0, None));
        // PC8: a server before privacy says none: off.
        assert_eq!(before.privacy, ViewPrivacy::Off);
        // Stats from a later server with a field this one does not know,
        // and with some left out.
        let later = Message::parse(
            STATUS,
            br#"{"phase":"showing","stats":{"fps":30,"jitter_ms":4}}"#.to_vec(),
        )
        .unwrap();
        let Message::Status(later) = later else {
            panic!("a status");
        };
        assert_eq!(
            later.stats,
            Some(ViewStats {
                fps: 30,
                ..ViewStats::default()
            })
        );
    }

    #[test]
    fn a_long_reason_is_cut_at_a_character() {
        let long = "é".repeat(MAX_WORDS);
        let Message::End { why } =
            Message::read(&mut io::Cursor::new(Message::End { why: long }.encode())).unwrap()
        else {
            panic!("an end");
        };
        assert!(why.len() <= MAX_WORDS && why.chars().all(|c| c == 'é'));
    }

    /// The window drives the pointer and the keys and picks the quality;
    /// what a phone alone sends (a pad, a crop, the laser) never goes.
    #[test]
    fn only_the_pointer_keys_and_the_views_own_asks_are_sent() {
        let gates = [(false, false), (true, false), (false, true), (true, true)];
        for allowed in [
            InputPacket::MouseMoveAbs {
                x_norm: 1,
                y_norm: 2,
            },
            InputPacket::MouseClick {
                button: 0,
                pressed: true,
            },
            InputPacket::MouseScroll { dx: -1, dy: 2 },
            InputPacket::KeyEvent {
                key: 30,
                pressed: false,
            },
            InputPacket::Text("hé".into()),
            InputPacket::RequestKeyframe,
            InputPacket::QualityPreset { preset: 2 },
            InputPacket::ViewOnly { enabled: true },
        ] {
            for (locked, mic) in gates {
                assert!(
                    input_allowed(&allowed, locked, mic),
                    "{allowed:?} locked {locked} mic {mic}"
                );
            }
        }
        // PC7: a relative move only while the window's pointer is locked.
        let relative = InputPacket::MouseMove { dx: 1, dy: -1 };
        assert!(!input_allowed(&relative, false, false));
        assert!(!input_allowed(&relative, false, true));
        assert!(input_allowed(&relative, true, false));
        // PC10: the mic's frames, its start and its stop only while the
        // window says its mic is on.
        for mic in [
            InputPacket::Mic {
                enabled: true,
                opus: Vec::new(),
            },
            InputPacket::Mic {
                enabled: true,
                opus: vec![0xF8, 0xFF, 0xFE],
            },
            InputPacket::Mic {
                enabled: false,
                opus: Vec::new(),
            },
        ] {
            assert!(!input_allowed(&mic, false, false), "{mic:?}");
            assert!(!input_allowed(&mic, true, false), "{mic:?} locked");
            assert!(input_allowed(&mic, false, true), "{mic:?} mic on");
        }
        for refused in [
            InputPacket::FullQuality { enabled: true },
            InputPacket::WindowCrop {
                x: None,
                y: None,
                width: None,
                height: None,
                window: 0,
            },
            InputPacket::Laser {
                dx: 1,
                dy: 1,
                show: true,
            },
            InputPacket::Gamepad {
                axes: [0; 6],
                buttons: 0,
            },
            InputPacket::ClientSamples {
                kind: 0,
                values: Vec::new(),
            },
        ] {
            for (locked, mic) in gates {
                assert!(
                    !input_allowed(&refused, locked, mic),
                    "{refused:?} locked {locked} mic {mic}"
                );
            }
        }
    }

    #[test]
    fn the_file_is_written_read_and_removed_only_for_its_own_view() {
        let dir = std::env::temp_dir().join(format!("pd-view-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let view = ViewFile {
            session_id: 4,
            device_id: "desk".into(),
            name: "Desk".into(),
            port: 50000,
            token: super::super::cast::token_hex(&[1u8; TOKEN_LEN]),
        };
        assert_eq!(view.token_bytes(), Some(vec![1u8; TOKEN_LEN]));
        write(&dir, &view).unwrap();
        assert_eq!(read(&dir), Some(view.clone()));
        assert_eq!(super::super::cast::read(&dir), None, "a view is not a cast");
        remove(&dir, 5).unwrap();
        assert!(read(&dir).is_some(), "another view's end leaves it");
        remove(&dir, 4).unwrap();
        assert_eq!(read(&dir), None);
        let bad = ViewFile {
            token: "zz".repeat(TOKEN_LEN),
            ..view
        };
        assert_eq!(bad.token_bytes(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
