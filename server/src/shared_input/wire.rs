//! The messages two computers send each other while one shares its keyboard
//! and mouse with the other.
//!
//! One message is one tag byte and its fields, big-endian and fixed-size, with
//! a string as a `u16` length and its UTF-8. On a stream a message travels as
//! a [`frame`]: a `u16` length and then the message. The mouse and the keys
//! mean what [`InputPacket`](pocketdesk_core::streaming::input_packet::InputPacket)
//! says of them (buttons 0 left, 1 middle, 2 right, 3 back, 4 forward; keys are
//! evdev codes; the wheel counts notches).

use std::io;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt};

/// The wire's version, the first thing a [`Msg::Hello`] says. A `Hello`'s own
/// layout never changes between versions, so a peer on another version can
/// still be told apart from one that is not speaking this at all.
pub const VERSION: u8 = 1;

/// What opens a stream of frames, so a stream meant for something else is not
/// read as these.
pub const STREAM_MAGIC: [u8; 4] = *b"PDSI";

/// A monitor ([`Screen`]); the longest name a message carries, in bytes (a
/// longer one is cut short at a character when encoded); and the most
/// screens a message lists (the rest are left off when encoded). They are
/// core's, which keeps what each computer said in `shared_screens.json`.
pub use pocketdesk_core::local::shared_input::{MAX_NAME_BYTES, MAX_SCREENS, Screen};

const TAG_HELLO: u8 = 0;
const TAG_SCREENS: u8 = 1;
const TAG_ENTER: u8 = 2;
const TAG_LEAVE: u8 = 3;
const TAG_MOVE: u8 = 4;
const TAG_BUTTON: u8 = 5;
const TAG_WHEEL: u8 = 6;
const TAG_KEY: u8 = 7;
const TAG_RELEASE_ALL: u8 = 8;
const TAG_ACTIVE: u8 = 9;
const TAG_IDENTIFY: u8 = 10;
const TAG_PING: u8 = 11;
const TAG_PONG: u8 = 12;
const TAG_REFUSE: u8 = 13;
const TAG_ASK_TIMINGS: u8 = 14;
const TAG_TIMINGS: u8 = 15;

/// The most timings one [`Msg::Timings`] carries (the rest are left off
/// when encoded), so it fits a frame.
pub const MAX_TIMINGS: usize = 3500;

/// How long one key or move took on the computer driven, from the moment
/// it came off the link until it was injected ([`Msg::Timings`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timing {
    /// A move (its `seq` is `n`), or a key, button or wheel turn (the
    /// `n`th since the cursor arrived, 1 the first).
    pub moved: bool,
    pub n: u32,
    /// When it came off the link, in microseconds since the Unix epoch, on
    /// the clock of the computer driven.
    pub arrived_us: u64,
    /// From then until it was injected, in microseconds.
    pub took_us: u32,
}

/// Why a computer refuses to be driven ([`Msg::Refuse`]): the shared
/// keyboard and mouse are off there.
pub const REFUSE_SHARING_OFF: u8 = 1;
/// Why a computer refuses to be driven: it does not let the sender (it has
/// not the Keyboard and mouse permission there, or is not one of the
/// owner's computers to it).
pub const REFUSE_NOT_ALLOWED: u8 = 2;
/// Why a computer refuses to be driven: its link to the sender rides a
/// relay there (the keyboard and mouse are shared on a direct link only).
pub const REFUSE_RELAYED: u8 = 3;

/// One message between two computers sharing input. Its serde form is for
/// records such as `share-test`'s, never the wire, which is [`encode`]'s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Msg {
    /// The first message each way: the wire's version, who is speaking and
    /// what screens it has.
    Hello {
        version: u8,
        name: String,
        screens: Vec<Screen>,
    },
    /// The sender's screens changed.
    Screens(Vec<Screen>),
    /// The cursor came onto the receiver's screens at this position (0..=65535
    /// over all of them, as `InputPacket::MouseMoveAbs`).
    Enter {
        x: u16,
        y: u16,
    },
    /// The cursor went back to the sender; the receiver stops being driven.
    Leave,
    /// Where the cursor is now, on the same scale as [`Msg::Enter`]. `seq`
    /// counts up so a late one can be told from the latest.
    Move {
        seq: u32,
        x: u16,
        y: u16,
    },
    Button {
        button: u8,
        pressed: bool,
    },
    /// Notches, positive down and to the right.
    Wheel {
        dx: i16,
        dy: i16,
    },
    /// An evdev key code.
    Key {
        code: u16,
        pressed: bool,
    },
    /// Let go of every key and button that is down.
    ReleaseAll,
    /// The person at the sender last touched it this long ago.
    Active {
        idle_ms: u32,
    },
    /// Ask the receiver to flash its own name (Identify); `name` is the
    /// asking computer's, for its log.
    Identify {
        name: String,
    },
    /// A clock reading in microseconds; the peer answers with a [`Msg::Pong`]
    /// of the same reading.
    Ping {
        t_us: u64,
    },
    /// The answer to a [`Msg::Ping`]: its reading, and the answering
    /// computer's own clock as it answered (microseconds since the Unix
    /// epoch), so the two clocks can be lined up.
    Pong {
        t_us: u64,
        at_us: u64,
    },
    /// The receiver will not be driven by the sender: its answer to a
    /// `Hello` it will not take, and its word to a computer it took when it
    /// stops ([`REFUSE_SHARING_OFF`], [`REFUSE_NOT_ALLOWED`],
    /// [`REFUSE_RELAYED`]; another reason is still a refusal).
    Refuse {
        reason: u8,
    },
    /// Ask the receiver how long what this computer sent since its cursor
    /// last arrived took there (`share-test`); answered with a
    /// [`Msg::Timings`].
    AskTimings,
    /// How long each key and move from the asking computer took here since
    /// its cursor last arrived, at most [`MAX_TIMINGS`].
    Timings(Vec<Timing>),
}

/// Why bytes are not a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireError {
    /// The bytes ended before the message did.
    Truncated,
    /// The first byte is no message this knows.
    UnknownTag(u8),
    /// A name that is not UTF-8.
    InvalidUtf8,
    /// A flag that is neither 0 nor 1.
    InvalidBool(u8),
    /// A whole message and then more bytes.
    TrailingBytes,
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WireError::Truncated => write!(f, "message cut short"),
            WireError::UnknownTag(t) => write!(f, "unknown message tag {t}"),
            WireError::InvalidUtf8 => write!(f, "name is not UTF-8"),
            WireError::InvalidBool(b) => write!(f, "flag byte {b} is neither 0 nor 1"),
            WireError::TrailingBytes => write!(f, "bytes after the end of the message"),
        }
    }
}

impl std::error::Error for WireError {}

/// A message as bytes.
pub fn encode(msg: &Msg) -> Vec<u8> {
    let mut out = Vec::with_capacity(16);
    match msg {
        Msg::Hello {
            version,
            name,
            screens,
        } => {
            out.push(TAG_HELLO);
            out.push(*version);
            put_str(&mut out, name);
            put_screens(&mut out, screens);
        }
        Msg::Screens(screens) => {
            out.push(TAG_SCREENS);
            put_screens(&mut out, screens);
        }
        Msg::Enter { x, y } => {
            out.push(TAG_ENTER);
            out.extend_from_slice(&x.to_be_bytes());
            out.extend_from_slice(&y.to_be_bytes());
        }
        Msg::Leave => out.push(TAG_LEAVE),
        Msg::Move { seq, x, y } => {
            out.push(TAG_MOVE);
            out.extend_from_slice(&seq.to_be_bytes());
            out.extend_from_slice(&x.to_be_bytes());
            out.extend_from_slice(&y.to_be_bytes());
        }
        Msg::Button { button, pressed } => {
            out.push(TAG_BUTTON);
            out.push(*button);
            out.push(u8::from(*pressed));
        }
        Msg::Wheel { dx, dy } => {
            out.push(TAG_WHEEL);
            out.extend_from_slice(&dx.to_be_bytes());
            out.extend_from_slice(&dy.to_be_bytes());
        }
        Msg::Key { code, pressed } => {
            out.push(TAG_KEY);
            out.extend_from_slice(&code.to_be_bytes());
            out.push(u8::from(*pressed));
        }
        Msg::ReleaseAll => out.push(TAG_RELEASE_ALL),
        Msg::Active { idle_ms } => {
            out.push(TAG_ACTIVE);
            out.extend_from_slice(&idle_ms.to_be_bytes());
        }
        Msg::Identify { name } => {
            out.push(TAG_IDENTIFY);
            put_str(&mut out, name);
        }
        Msg::Ping { t_us } => {
            out.push(TAG_PING);
            out.extend_from_slice(&t_us.to_be_bytes());
        }
        Msg::Pong { t_us, at_us } => {
            out.push(TAG_PONG);
            out.extend_from_slice(&t_us.to_be_bytes());
            out.extend_from_slice(&at_us.to_be_bytes());
        }
        Msg::Refuse { reason } => {
            out.push(TAG_REFUSE);
            out.push(*reason);
        }
        Msg::AskTimings => out.push(TAG_ASK_TIMINGS),
        Msg::Timings(timings) => {
            out.push(TAG_TIMINGS);
            let timings = &timings[..timings.len().min(MAX_TIMINGS)];
            out.extend_from_slice(&(timings.len() as u16).to_be_bytes());
            for t in timings {
                out.push(u8::from(t.moved));
                out.extend_from_slice(&t.n.to_be_bytes());
                out.extend_from_slice(&t.arrived_us.to_be_bytes());
                out.extend_from_slice(&t.took_us.to_be_bytes());
            }
        }
    }
    out
}

/// A message from the bytes [`encode`] made, all of them and nothing more.
pub fn decode(bytes: &[u8]) -> Result<Msg, WireError> {
    let mut r = Reader { rest: bytes };
    let msg = match r.u8()? {
        TAG_HELLO => Msg::Hello {
            version: r.u8()?,
            name: r.string()?,
            screens: r.screens()?,
        },
        TAG_SCREENS => Msg::Screens(r.screens()?),
        TAG_ENTER => Msg::Enter {
            x: r.u16()?,
            y: r.u16()?,
        },
        TAG_LEAVE => Msg::Leave,
        TAG_MOVE => Msg::Move {
            seq: r.u32()?,
            x: r.u16()?,
            y: r.u16()?,
        },
        TAG_BUTTON => Msg::Button {
            button: r.u8()?,
            pressed: r.flag()?,
        },
        TAG_WHEEL => Msg::Wheel {
            dx: r.i16()?,
            dy: r.i16()?,
        },
        TAG_KEY => Msg::Key {
            code: r.u16()?,
            pressed: r.flag()?,
        },
        TAG_RELEASE_ALL => Msg::ReleaseAll,
        TAG_ACTIVE => Msg::Active { idle_ms: r.u32()? },
        TAG_IDENTIFY => Msg::Identify { name: r.string()? },
        TAG_PING => Msg::Ping { t_us: r.u64()? },
        TAG_PONG => Msg::Pong {
            t_us: r.u64()?,
            at_us: r.u64()?,
        },
        TAG_REFUSE => Msg::Refuse { reason: r.u8()? },
        TAG_ASK_TIMINGS => Msg::AskTimings,
        TAG_TIMINGS => Msg::Timings(r.timings()?),
        tag => return Err(WireError::UnknownTag(tag)),
    };
    if r.rest.is_empty() {
        Ok(msg)
    } else {
        Err(WireError::TrailingBytes)
    }
}

/// A message ready for a stream: its length as a big-endian `u16`, then it.
pub fn frame(msg: &Msg) -> Vec<u8> {
    let body = encode(msg);
    // Names, screen lists and timings are clipped when encoded, so a
    // message always fits: the largest is a `Timings` of about 60 KB.
    let len = u16::try_from(body.len()).expect("an encoded message fits a u16 length");
    let mut out = Vec::with_capacity(2 + body.len());
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&body);
    out
}

/// The next frame off a stream. `Ok(None)` is the peer closing between frames;
/// closing in the middle of one, or sending bytes that are no message, is an
/// error.
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Option<Msg>> {
    let mut len = [0u8; 2];
    if reader.read(&mut len[..1]).await? == 0 {
        return Ok(None);
    }
    reader.read_exact(&mut len[1..]).await?;
    let mut body = vec![0u8; usize::from(u16::from_be_bytes(len))];
    reader.read_exact(&mut body).await?;
    decode(&body)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    let mut end = s.len().min(MAX_NAME_BYTES);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    // At most `MAX_NAME_BYTES`, which is well inside a `u16`.
    out.extend_from_slice(&(end as u16).to_be_bytes());
    out.extend_from_slice(&s.as_bytes()[..end]);
}

fn put_screens(out: &mut Vec<u8>, screens: &[Screen]) {
    let screens = &screens[..screens.len().min(MAX_SCREENS)];
    out.extend_from_slice(&(screens.len() as u16).to_be_bytes());
    for s in screens {
        out.extend_from_slice(&s.x.to_be_bytes());
        out.extend_from_slice(&s.y.to_be_bytes());
        out.extend_from_slice(&s.width.to_be_bytes());
        out.extend_from_slice(&s.height.to_be_bytes());
    }
}

/// Bytes being read off the front.
struct Reader<'a> {
    rest: &'a [u8],
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], WireError> {
        let (head, rest) = self
            .rest
            .split_first_chunk::<N>()
            .ok_or(WireError::Truncated)?;
        self.rest = rest;
        Ok(*head)
    }

    fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.take::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, WireError> {
        self.take().map(u16::from_be_bytes)
    }

    fn i16(&mut self) -> Result<i16, WireError> {
        self.take().map(i16::from_be_bytes)
    }

    fn u32(&mut self) -> Result<u32, WireError> {
        self.take().map(u32::from_be_bytes)
    }

    fn i32(&mut self) -> Result<i32, WireError> {
        self.take().map(i32::from_be_bytes)
    }

    fn u64(&mut self) -> Result<u64, WireError> {
        self.take().map(u64::from_be_bytes)
    }

    fn flag(&mut self) -> Result<bool, WireError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(WireError::InvalidBool(other)),
        }
    }

    fn string(&mut self) -> Result<String, WireError> {
        let len = usize::from(self.u16()?);
        if self.rest.len() < len {
            return Err(WireError::Truncated);
        }
        let (head, rest) = self.rest.split_at(len);
        self.rest = rest;
        String::from_utf8(head.to_vec()).map_err(|_| WireError::InvalidUtf8)
    }

    fn screens(&mut self) -> Result<Vec<Screen>, WireError> {
        let count = usize::from(self.u16()?);
        // Not `with_capacity(count)`: the count is the sender's to claim.
        let mut screens = Vec::with_capacity(count.min(self.rest.len() / 16));
        for _ in 0..count {
            screens.push(Screen {
                x: self.i32()?,
                y: self.i32()?,
                width: self.u32()?,
                height: self.u32()?,
            });
        }
        Ok(screens)
    }

    fn timings(&mut self) -> Result<Vec<Timing>, WireError> {
        let count = usize::from(self.u16()?);
        let mut timings = Vec::with_capacity(count.min(self.rest.len() / 17));
        for _ in 0..count {
            timings.push(Timing {
                moved: self.flag()?,
                n: self.u32()?,
                arrived_us: self.u64()?,
                took_us: self.u32()?,
            });
        }
        Ok(timings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screens() -> Vec<Screen> {
        vec![
            Screen {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            Screen {
                x: 1920,
                y: 0,
                width: 1536,
                height: 960,
            },
            Screen {
                x: -1280,
                y: -200,
                width: 1280,
                height: 1024,
            },
        ]
    }

    fn every_variant() -> Vec<Msg> {
        vec![
            Msg::Hello {
                version: VERSION,
                name: "Desk laptop \u{e9}\u{4e2d}".to_string(),
                screens: screens(),
            },
            Msg::Hello {
                version: VERSION,
                name: String::new(),
                screens: Vec::new(),
            },
            Msg::Screens(screens()),
            Msg::Screens(Vec::new()),
            Msg::Enter { x: 0, y: 65535 },
            Msg::Leave,
            Msg::Move {
                seq: u32::MAX,
                x: 32768,
                y: 1,
            },
            Msg::Button {
                button: 4,
                pressed: true,
            },
            Msg::Button {
                button: 0,
                pressed: false,
            },
            Msg::Wheel {
                dx: i16::MIN,
                dy: i16::MAX,
            },
            Msg::Key {
                code: 30,
                pressed: true,
            },
            Msg::Key {
                code: u16::MAX,
                pressed: false,
            },
            Msg::ReleaseAll,
            Msg::Active { idle_ms: 200 },
            Msg::Identify {
                name: "Linux".to_string(),
            },
            Msg::Ping { t_us: u64::MAX },
            Msg::Pong {
                t_us: 0,
                at_us: u64::MAX,
            },
            Msg::Refuse {
                reason: REFUSE_SHARING_OFF,
            },
            Msg::Refuse { reason: u8::MAX },
            Msg::AskTimings,
            Msg::Timings(vec![
                Timing {
                    moved: true,
                    n: u32::MAX,
                    arrived_us: 1_700_000_000_000_000,
                    took_us: 250,
                },
                Timing {
                    moved: false,
                    n: 1,
                    arrived_us: 0,
                    took_us: u32::MAX,
                },
            ]),
            Msg::Timings(Vec::new()),
        ]
    }

    #[test]
    fn a_refusal_round_trips_with_its_reason() {
        for reason in [REFUSE_SHARING_OFF, REFUSE_NOT_ALLOWED, REFUSE_RELAYED] {
            let m = Msg::Refuse { reason };
            let bytes = encode(&m);
            assert_eq!(bytes, vec![13, reason]);
            assert_eq!(decode(&bytes), Ok(m));
        }
        assert_ne!(REFUSE_SHARING_OFF, REFUSE_NOT_ALLOWED);
    }

    #[test]
    fn every_message_round_trips() {
        for m in every_variant() {
            assert_eq!(decode(&encode(&m)), Ok(m.clone()), "{m:?}");
        }
    }

    #[test]
    fn truncated_and_unknown_are_errors() {
        // Every proper prefix of every message is cut short, never a message.
        for m in every_variant() {
            let bytes = encode(&m);
            for n in 0..bytes.len() {
                assert_eq!(decode(&bytes[..n]), Err(WireError::Truncated), "{m:?}/{n}");
            }
        }
        assert_eq!(decode(&[200]), Err(WireError::UnknownTag(200)));
        // A whole message and then more is not one message.
        let mut long = encode(&Msg::Leave);
        long.push(0);
        assert_eq!(decode(&long), Err(WireError::TrailingBytes));
        // A flag is 0 or 1.
        assert_eq!(decode(&[7, 0, 30, 2]), Err(WireError::InvalidBool(2)));
        // A name is UTF-8.
        assert_eq!(decode(&[10, 0, 1, 0xff]), Err(WireError::InvalidUtf8));
    }

    #[test]
    fn hello_carries_its_version_first() {
        let bytes = encode(&Msg::Hello {
            version: VERSION,
            name: "A".to_string(),
            screens: Vec::new(),
        });
        assert_eq!(bytes[1], VERSION);
        // A peer on another version still decodes, so it can be named.
        let other = Msg::Hello {
            version: VERSION + 1,
            name: "A".to_string(),
            screens: Vec::new(),
        };
        assert_eq!(decode(&encode(&other)), Ok(other));
    }

    #[test]
    fn long_names_and_many_screens_still_fit_a_frame() {
        let m = Msg::Hello {
            version: VERSION,
            name: "\u{4e2d}".repeat(10_000),
            screens: vec![screens()[0]; 5_000],
        };
        let framed = frame(&m);
        let len = usize::from(u16::from_be_bytes([framed[0], framed[1]]));
        assert_eq!(len, framed.len() - 2);
        let Ok(Msg::Hello { name, screens, .. }) = decode(&framed[2..]) else {
            panic!("the clipped hello decodes");
        };
        assert!(name.len() <= MAX_NAME_BYTES);
        assert!(name.chars().all(|c| c == '\u{4e2d}'));
        assert_eq!(screens.len(), MAX_SCREENS);
    }

    #[test]
    fn many_timings_still_fit_a_frame() {
        let one = Timing {
            moved: true,
            n: 1,
            arrived_us: u64::MAX,
            took_us: u32::MAX,
        };
        let framed = frame(&Msg::Timings(vec![one; 10_000]));
        let Ok(Msg::Timings(timings)) = decode(&framed[2..]) else {
            panic!("the clipped timings decode");
        };
        assert_eq!(timings.len(), MAX_TIMINGS);
    }

    #[tokio::test]
    async fn frames_read_back_in_order() {
        let a = Msg::Enter { x: 10, y: 20 };
        let b = Msg::Key {
            code: 57,
            pressed: true,
        };
        let mut bytes = frame(&a);
        bytes.extend(frame(&b));
        let mut r = &bytes[..];
        assert_eq!(read_frame(&mut r).await.unwrap(), Some(a));
        assert_eq!(read_frame(&mut r).await.unwrap(), Some(b));
        // The peer closing between frames is a clean end.
        assert_eq!(read_frame(&mut r).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_frame_cut_short_or_malformed_is_an_error() {
        let bytes = frame(&Msg::Move { seq: 1, x: 2, y: 3 });
        for n in 1..bytes.len() {
            let mut r = &bytes[..n];
            let err = read_frame(&mut r).await.unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof, "{n}");
        }
        // A frame whose body is not a message.
        let mut r = &[0u8, 1, 200][..];
        let err = read_frame(&mut r).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        // An empty frame is not a message either.
        let mut r = &[0u8, 0][..];
        let err = read_frame(&mut r).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn frame_is_a_big_endian_length_then_the_message() {
        let m = Msg::Pong { t_us: 7, at_us: 9 };
        let body = encode(&m);
        let framed = frame(&m);
        assert_eq!(&framed[..2], &(body.len() as u16).to_be_bytes());
        assert_eq!(&framed[2..], &body[..]);
        assert_eq!(STREAM_MAGIC, *b"PDSI");
    }
}
