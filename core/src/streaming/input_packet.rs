//! Binary input packet protocol for real-time input forwarding over QUIC.
//!
//! Provides a compact binary encoding for mouse, keyboard, and scroll events,
//! replacing the JSON-over-TCP KDE Connect protocol with a much more efficient
//! format (~3-5 bytes per event vs ~80 bytes JSON).
//!
//! The same tag space also carries the client's own measurements back the other
//! way (`LinkFeedback`, `ClientSamples`): one stream of frames the server can
//! dispatch by tag, so nothing on this connection is ever untagged.

use anyhow::{Context, Result};

/// Tag byte for each packet variant
const TAG_MOUSE_MOVE: u8 = 0;
const TAG_MOUSE_CLICK: u8 = 1;
const TAG_MOUSE_SCROLL: u8 = 2;
const TAG_KEY_EVENT: u8 = 3;
const TAG_TEXT: u8 = 4;
const TAG_GAMEPAD: u8 = 5;
const TAG_MOUSE_MOVE_ABS: u8 = 6;
const TAG_REQUEST_KEYFRAME: u8 = 7;
const TAG_WINDOW_CROP: u8 = 8;
const TAG_VIEW_ONLY: u8 = 9;
const TAG_FULL_QUALITY: u8 = 10;
const TAG_MIC: u8 = 11;
const TAG_QUALITY_PRESET: u8 = 12;
const TAG_LINK_FEEDBACK: u8 = 13;
const TAG_CLIENT_SAMPLES: u8 = 14;
const TAG_PEN: u8 = 15;

/// A [`PenSample`]'s flags (E3). Bits not named here are ignored.
pub const PEN_CONTACT: u8 = 1;
/// The pen is within reach of the screen: hovering, or in contact. A sample
/// without it says the pen left.
pub const PEN_IN_RANGE: u8 = 1 << 1;
/// The stylus's side button is held.
pub const PEN_BARREL: u8 = 1 << 2;
/// The stylus's eraser end (sent; not yet acted on).
pub const PEN_ERASER: u8 = 1 << 3;

/// The steepest tilt a [`PenSample`] says, in degrees either way.
pub const PEN_MAX_TILT: i8 = 90;

/// One sample of a pen on the phone's screen (E3): where it is, as
/// [`InputPacket::MouseMoveAbs`] says it (0..=65535 over the picture), how
/// hard it presses (0..=65535), its tilt in degrees each way (a finger's
/// is 0), and [`PEN_CONTACT`] and the other flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PenSample {
    pub x_norm: u16,
    pub y_norm: u16,
    pub pressure: u16,
    pub tilt_x: i8,
    pub tilt_y: i8,
    pub flags: u8,
}

impl PenSample {
    pub fn in_contact(&self) -> bool {
        self.flags & PEN_CONTACT != 0
    }

    /// In range: in contact counts, whatever the flags say.
    pub fn in_range(&self) -> bool {
        self.flags & (PEN_IN_RANGE | PEN_CONTACT) != 0
    }

    pub fn barrel(&self) -> bool {
        self.flags & PEN_BARREL != 0
    }
}

/// Which client-side distribution a [`InputPacket::ClientSamples`] batch
/// belongs to. Shared client↔server like the preset ids, so these must never
/// be renumbered.
pub const SAMPLE_DECODE: u8 = 0;
pub const SAMPLE_RENDER: u8 = 1;
pub const SAMPLE_E2E: u8 = 2;

/// Longest sample batch one frame can carry: the count is a `u8`, so a client
/// producing more than this flushes in chunks.
pub const MAX_SAMPLES_PER_FRAME: usize = 255;

/// Largest sample expressible on the wire, in microseconds. Durations travel
/// as `u32` micros, so a frame that took longer than this to decode or render
/// is reported at the ceiling — a stall that extreme is already obvious in the
/// frame gap, and the ceiling keeps it from being reported as a tiny number.
pub const MAX_SAMPLE_MICROS: u32 = u32::MAX;

/// Clamp one duration in microseconds into the wire range (see
/// [`MAX_SAMPLE_MICROS`]). Pure, so the truncation rule has one definition.
pub fn clamp_sample_micros(value_us: u64) -> u32 {
    value_us.try_into().unwrap_or(MAX_SAMPLE_MICROS)
}

/// How many samples [`SampleBatch`] holds before it starts discarding. Eight
/// flushes' worth at 255 per frame: a client whose reporting loop is starved
/// keeps its recent history rather than its oldest, which is where a stall that
/// is still happening lives.
pub const SAMPLE_BATCH_CAPACITY: usize = MAX_SAMPLES_PER_FRAME * 8;

/// Durations waiting to be reported to the server, in microseconds.
///
/// The client cannot know how long a session will run or how fast it is
/// sampling, so it holds a bounded batch and hands it to the server in
/// [`MAX_SAMPLES_PER_FRAME`]-sized chunks; the server's reservoir is what
/// computes percentiles. Shared behind an `Arc` by whoever measures and whoever
/// flushes — both sides only ever need `&self`.
#[derive(Default)]
pub struct SampleBatch {
    values: std::sync::Mutex<Vec<u32>>,
}

impl SampleBatch {
    /// An empty batch. `const` so a client can hold one in a `static` instead
    /// of behind a lazy-initialisation wrapper.
    pub const fn new() -> Self {
        Self {
            values: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Record one duration. Returns `false` when the batch was full and the
    /// oldest sample had to be dropped to keep this one.
    pub fn push(&self, value_us: u64) -> bool {
        let mut values = self.values.lock().unwrap_or_else(|p| p.into_inner());
        if values.len() >= SAMPLE_BATCH_CAPACITY {
            values.remove(0);
            values.push(clamp_sample_micros(value_us));
            return false;
        }
        values.push(clamp_sample_micros(value_us));
        true
    }

    /// Take the next chunk to send, oldest sample first, or `None` when nothing
    /// is pending. A batch longer than one frame's worth is drained over
    /// several calls.
    pub fn take_chunk(&self) -> Option<Vec<u32>> {
        let mut values = self.values.lock().unwrap_or_else(|p| p.into_inner());
        if values.is_empty() {
            return None;
        }
        let take = values.len().min(MAX_SAMPLES_PER_FRAME);
        Some(values.drain(..take).collect())
    }

    /// Samples currently held. For tests and for a caller deciding whether to
    /// open a stream at all.
    pub fn len(&self) -> usize {
        self.values.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// Forget everything pending, without reporting it. A new session must not
    /// inherit the last one's measurements, and a leftover batch would otherwise
    /// be shipped to whoever happens to be listening next.
    pub fn clear(&self) {
        self.values
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// R4 E5: named link-profile presets, switchable from the phone HUD. A
/// preset is a *ceiling* the user asks for; the server folds it together
/// with the R4 A3 relay floor into the live encoder bitrate. Values are
/// shared client↔server, so they must never be renumbered.
pub const PRESET_AUTO: u8 = 0;
pub const PRESET_QUALITY: u8 = 1;
pub const PRESET_BALANCED: u8 = 2;
pub const PRESET_ECONOMY: u8 = 3;

/// Map a [`PRESET_*`] id to the bitrate ceiling it implies, expressed
/// relative to the session's `configured_bps` (native resolution differs
/// per monitor, so bands are caps, never floors). `Auto` and `Quality`
/// impose no preset ceiling — `Auto` leaves the rate to the link (the A3
/// relay floor still applies) while `Quality` is the user explicitly
/// asking for the full configured rate. Pure, so it is unit-testable on
/// both the server and the (client-only) bridge build.
pub fn preset_bitrate_ceil(preset: u8, configured_bps: u32) -> u32 {
    match preset {
        PRESET_BALANCED => configured_bps.min(5_000_000),
        PRESET_ECONOMY => configured_bps.min(1_500_000),
        _ => configured_bps,
    }
}

/// A compact binary input event for real-time remote control.
///
/// Each variant is encoded as a single tag byte followed by a fixed-size
/// or length-prefixed payload.
#[derive(Debug, Clone)]
pub enum InputPacket {
    /// Relative mouse movement.
    MouseMove { dx: i16, dy: i16 },
    /// Absolute pointer position, normalized to the remote screen:
    /// 0..=65535 maps to the full width/height (resolution-independent,
    /// so the client never needs the pixel dimensions).
    MouseMoveAbs { x_norm: u16, y_norm: u16 },
    /// Mouse button press or release.
    MouseClick {
        button: u8, // 0=Left, 1=Middle, 2=Right, 3=Back, 4=Forward
        pressed: bool,
    },
    /// Scroll wheel movement.
    MouseScroll {
        dx: i16, // horizontal scroll
        dy: i16, // vertical scroll
    },
    /// Keyboard key press or release.
    KeyEvent {
        key: u16, // Linux evdev keycode
        pressed: bool,
    },
    /// Raw text input (typed via clipboard paste or IME).
    Text(String),
    /// Client detected a video sequence gap and asks the server to emit an
    /// IDR frame immediately.
    RequestKeyframe,
    /// Restrict the stream to a region of the desktop (window-crop mode).
    /// `None` fields mean 0; an all-`None` packet clears the crop and
    /// restores the full desktop. `window` (R4 B2) carries the Hyprland
    /// window address (0 = none): when nonzero and the compositor offers
    /// `hyprland_toplevel_export_v1`, the capture is that window's own
    /// buffer — occlusion-correct, no software crop of the rect.
    WindowCrop {
        x: Option<u32>,
        y: Option<u32>,
        width: Option<u32>,
        height: Option<u32>,
        window: u64,
    },
    /// R4 D1 view-only mode: while enabled the **server** drops every
    /// injected-input packet from this session (mouse, keyboard, gamepad,
    /// text). Control packets (keyframe requests, crops, further view-only
    /// toggles) keep flowing, and video is unaffected.
    ViewOnly { enabled: bool },
    /// R4 A3: opt OUT of the server's relay bitrate floor — stream at the
    /// configured bitrate even while the path is a relay (user override,
    /// default false). `true` disables the clamp for this session; video
    /// quality then depends on relay capacity. Control-plane, never injected.
    FullQuality { enabled: bool },
    /// R4 E2 reverse audio (phone mic → desktop virtual microphone).
    /// `enabled` is the mic-session latch: `true` with an `opus` payload
    /// carries one 20 ms Opus frame (48 kHz mono); `true` with an empty
    /// payload is a start/keep-alive; `false` tears the sink down.
    /// Never injected as input and NOT blocked by view-only — it is media
    /// the phone user chooses to broadcast, handled server-side by the
    /// mic relay (decode Opus → feed a PipeWire source).
    Mic { enabled: bool, opus: Vec<u8> },
    /// R4 E5 link-profile preset: a named bitrate ceiling the user picks
    /// from the HUD (see [`PRESET_AUTO`]..[`PRESET_ECONOMY`]). Control-plane
    /// — never injected and it survives view-only, like `FullQuality` — the
    /// server folds it with the A3 relay floor and steers the live encoder
    /// bitrate. `preset` carries the id directly.
    QualityPreset { preset: u8 },
    /// The client's own view of the link, sent periodically on a feedback
    /// stream. `rtt` is what the client's transport measured (the same path the
    /// server samples, from the other end), and `lost_packets` is
    /// **receive-side** loss, which the sender's statistics cannot see at all —
    /// asymmetric loss is normal on a WAN and is the one number here that is
    /// only knowable by the receiver.
    ///
    /// This variant exists because the same two numbers used to go out as an
    /// untagged 16-byte blob that the server matched on length and threw away,
    /// which also meant any future 16-byte packet was silently swallowed.
    LinkFeedback {
        rtt: std::time::Duration,
        lost_packets: u64,
    },
    /// One batch of client-measured durations in microseconds, for the metric
    /// named by `kind` (see [`SAMPLE_DECODE`]..[`SAMPLE_E2E`]). The client keeps
    /// the samples it takes between flushes and hands them over in bulk so the
    /// **server's** reservoir computes the percentiles: one distribution per
    /// session ends up in one record, the same way the encode tail does.
    ///
    /// Control-plane like the preset rows — never injected, survives view-only.
    /// Values are clamped to [`MAX_SAMPLE_MICROS`] on the way in; use
    /// [`clamp_sample_micros`] so the rule has one definition.
    ClientSamples { kind: u8, values: Vec<u32> },
    /// A pen on the phone's screen (E3): injected as a pen where the
    /// computer has one, else as the mouse.
    Pen(PenSample),
    /// Gamepad state: 6 analog axes + 16-bit button bitmask.
    Gamepad {
        /// Left stick X, Left stick Y, Right stick X, Right stick Y, L2, R2.
        axes: [i16; 6],
        /// Bitmask of digital buttons (A=0, B=1, X=2, Y=3, LB=4, RB=5,
        /// Select=6, Start=7, Home=8, LSB=9, RSB=10, DPadUp=11,
        /// DPadDown=12, DPadLeft=13, DPadRight=14).
        buttons: u16,
    },
}

impl InputPacket {
    /// Encode this packet into a `Vec<u8>` for sending over a QUIC stream.
    pub fn encode(&self) -> Vec<u8> {
        match self {
            InputPacket::MouseMove { dx, dy } => {
                let mut buf = vec![TAG_MOUSE_MOVE];
                buf.extend_from_slice(&dx.to_le_bytes());
                buf.extend_from_slice(&dy.to_le_bytes());
                buf
            }
            InputPacket::MouseMoveAbs { x_norm, y_norm } => {
                let mut buf = vec![TAG_MOUSE_MOVE_ABS];
                buf.extend_from_slice(&x_norm.to_le_bytes());
                buf.extend_from_slice(&y_norm.to_le_bytes());
                buf
            }
            InputPacket::MouseClick { button, pressed } => {
                let mut buf = vec![TAG_MOUSE_CLICK];
                buf.push(*button);
                buf.push(if *pressed { 1 } else { 0 });
                buf
            }
            InputPacket::MouseScroll { dx, dy } => {
                let mut buf = vec![TAG_MOUSE_SCROLL];
                buf.extend_from_slice(&dx.to_le_bytes());
                buf.extend_from_slice(&dy.to_le_bytes());
                buf
            }
            InputPacket::KeyEvent { key, pressed } => {
                let mut buf = vec![TAG_KEY_EVENT];
                buf.extend_from_slice(&key.to_le_bytes());
                buf.push(if *pressed { 1 } else { 0 });
                buf
            }
            InputPacket::Pen(pen) => {
                let mut buf = Vec::with_capacity(10);
                buf.push(TAG_PEN);
                buf.extend_from_slice(&pen.x_norm.to_le_bytes());
                buf.extend_from_slice(&pen.y_norm.to_le_bytes());
                buf.extend_from_slice(&pen.pressure.to_le_bytes());
                buf.extend_from_slice(&pen.tilt_x.to_le_bytes());
                buf.extend_from_slice(&pen.tilt_y.to_le_bytes());
                buf.push(pen.flags);
                buf
            }
            InputPacket::Gamepad { axes, buttons } => {
                let mut buf = Vec::with_capacity(1 + 12 + 2);
                buf.push(TAG_GAMEPAD);
                for &axis in axes {
                    buf.extend_from_slice(&axis.to_le_bytes());
                }
                buf.extend_from_slice(&buttons.to_le_bytes());
                buf
            }
            InputPacket::Text(text) => {
                let text_bytes = text.as_bytes();
                let mut buf = Vec::with_capacity(1 + 4 + text_bytes.len());
                buf.push(TAG_TEXT);
                buf.extend_from_slice(&(text_bytes.len() as u32).to_le_bytes());
                buf.extend_from_slice(text_bytes);
                buf
            }
            InputPacket::RequestKeyframe => vec![TAG_REQUEST_KEYFRAME],
            InputPacket::ViewOnly { enabled } => {
                vec![TAG_VIEW_ONLY, if *enabled { 1 } else { 0 }]
            }
            InputPacket::FullQuality { enabled } => {
                vec![TAG_FULL_QUALITY, if *enabled { 1 } else { 0 }]
            }
            InputPacket::Mic { enabled, opus } => {
                let mut buf = Vec::with_capacity(6 + opus.len());
                buf.push(TAG_MIC);
                buf.push(if *enabled { 1 } else { 0 });
                buf.extend_from_slice(&(opus.len() as u32).to_le_bytes());
                buf.extend_from_slice(opus);
                buf
            }
            InputPacket::QualityPreset { preset } => vec![TAG_QUALITY_PRESET, *preset],
            InputPacket::LinkFeedback { rtt, lost_packets } => {
                let mut buf = Vec::with_capacity(17);
                buf.push(TAG_LINK_FEEDBACK);
                buf.extend_from_slice(&(rtt.as_micros() as u64).to_le_bytes());
                buf.extend_from_slice(&lost_packets.to_le_bytes());
                buf
            }
            InputPacket::ClientSamples { kind, values } => {
                // A caller that overruns the 255-sample frame loses the
                // remainder rather than putting a lying count on the wire;
                // SampleBatch::take_chunk is the chunking path that cannot
                // overflow in the first place.
                let values = &values[..values.len().min(MAX_SAMPLES_PER_FRAME)];
                let mut buf = Vec::with_capacity(3 + values.len() * 4);
                buf.push(TAG_CLIENT_SAMPLES);
                buf.push(*kind);
                buf.push(values.len() as u8);
                for &value in values {
                    buf.extend_from_slice(&value.to_le_bytes());
                }
                buf
            }
            InputPacket::WindowCrop {
                x,
                y,
                width,
                height,
                window,
            } => {
                let mut buf = Vec::with_capacity(25);
                buf.push(TAG_WINDOW_CROP);
                for v in [x, y, width, height] {
                    buf.extend_from_slice(&v.unwrap_or(0).to_le_bytes());
                }
                buf.extend_from_slice(&window.to_le_bytes());
                buf
            }
        }
    }

    /// For a `WindowCrop` packet: the crop rectangle it establishes, or
    /// `None` when the packet clears the crop (no width/height set).
    /// Coordinates default to 0 when absent.
    pub fn crop_rect(&self) -> Option<(u32, u32, u32, u32)> {
        match self {
            InputPacket::WindowCrop {
                x,
                y,
                width,
                height,
                ..
            } => match (*width, *height) {
                (Some(w), Some(h)) if w > 0 && h > 0 => {
                    Some((x.unwrap_or(0), y.unwrap_or(0), w, h))
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// For a `WindowCrop` packet: the Hyprland window address to capture
    /// compositor-side (R4 B2), or `None` for plain geometry cropping.
    pub fn window_handle(&self) -> Option<u64> {
        match self {
            InputPacket::WindowCrop { window, .. } => (*window > 0).then_some(*window),
            _ => None,
        }
    }

    /// Decode a packet from a byte slice.
    pub fn decode(data: &[u8]) -> Result<Self> {
        if data.is_empty() {
            anyhow::bail!("Empty input packet data");
        }

        let tag = data[0];
        match tag {
            TAG_MOUSE_MOVE => {
                anyhow::ensure!(data.len() >= 5, "MouseMove packet too short");
                let dx = i16::from_le_bytes(data[1..3].try_into().unwrap());
                let dy = i16::from_le_bytes(data[3..5].try_into().unwrap());
                Ok(InputPacket::MouseMove { dx, dy })
            }
            TAG_MOUSE_MOVE_ABS => {
                anyhow::ensure!(data.len() >= 5, "MouseMoveAbs packet too short");
                let x_norm = u16::from_le_bytes(data[1..3].try_into().unwrap());
                let y_norm = u16::from_le_bytes(data[3..5].try_into().unwrap());
                Ok(InputPacket::MouseMoveAbs { x_norm, y_norm })
            }
            TAG_MOUSE_CLICK => {
                anyhow::ensure!(data.len() >= 3, "MouseClick packet too short");
                let button = data[1];
                let pressed = data[2] != 0;
                Ok(InputPacket::MouseClick { button, pressed })
            }
            TAG_MOUSE_SCROLL => {
                anyhow::ensure!(data.len() >= 5, "MouseScroll packet too short");
                let dx = i16::from_le_bytes(data[1..3].try_into().unwrap());
                let dy = i16::from_le_bytes(data[3..5].try_into().unwrap());
                Ok(InputPacket::MouseScroll { dx, dy })
            }
            TAG_KEY_EVENT => {
                anyhow::ensure!(data.len() >= 4, "KeyEvent packet too short");
                let key = u16::from_le_bytes(data[1..3].try_into().unwrap());
                let pressed = data[3] != 0;
                Ok(InputPacket::KeyEvent { key, pressed })
            }
            TAG_PEN => {
                anyhow::ensure!(data.len() >= 10, "Pen packet too short");
                let tilt = |b: u8| (b as i8).clamp(-PEN_MAX_TILT, PEN_MAX_TILT);
                Ok(InputPacket::Pen(PenSample {
                    x_norm: u16::from_le_bytes(data[1..3].try_into().unwrap()),
                    y_norm: u16::from_le_bytes(data[3..5].try_into().unwrap()),
                    pressure: u16::from_le_bytes(data[5..7].try_into().unwrap()),
                    tilt_x: tilt(data[7]),
                    tilt_y: tilt(data[8]),
                    flags: data[9],
                }))
            }
            TAG_GAMEPAD => {
                anyhow::ensure!(data.len() >= 15, "Gamepad packet too short");
                let mut axes = [0i16; 6];
                for (i, axis) in axes.iter_mut().enumerate() {
                    let offset = 1 + i * 2;
                    *axis = i16::from_le_bytes(data[offset..offset + 2].try_into().unwrap());
                }
                let buttons = u16::from_le_bytes(data[13..15].try_into().unwrap());
                Ok(InputPacket::Gamepad { axes, buttons })
            }
            TAG_TEXT => {
                anyhow::ensure!(data.len() >= 5, "Text packet too short");
                let len = u32::from_le_bytes(data[1..5].try_into().unwrap()) as usize;
                anyhow::ensure!(data.len() >= 5 + len, "Text packet payload truncated");
                let text = String::from_utf8(data[5..5 + len].to_vec())
                    .context("Invalid UTF-8 in Text packet")?;
                Ok(InputPacket::Text(text))
            }
            TAG_REQUEST_KEYFRAME => {
                anyhow::ensure!(data.len() == 1, "RequestKeyframe packet must be 1 byte");
                Ok(InputPacket::RequestKeyframe)
            }
            TAG_VIEW_ONLY => {
                anyhow::ensure!(data.len() == 2, "ViewOnly packet must be 2 bytes");
                Ok(InputPacket::ViewOnly {
                    enabled: data[1] != 0,
                })
            }
            TAG_FULL_QUALITY => {
                anyhow::ensure!(data.len() == 2, "FullQuality packet must be 2 bytes");
                Ok(InputPacket::FullQuality {
                    enabled: data[1] != 0,
                })
            }
            TAG_MIC => {
                anyhow::ensure!(data.len() >= 6, "Mic packet needs a 6-byte header");
                let len = u32::from_le_bytes(data[2..6].try_into().unwrap()) as usize;
                anyhow::ensure!(
                    data.len() == 6 + len,
                    "Mic packet payload must match its length prefix"
                );
                Ok(InputPacket::Mic {
                    enabled: data[1] != 0,
                    opus: data[6..].to_vec(),
                })
            }
            TAG_WINDOW_CROP => {
                anyhow::ensure!(
                    data.len() == 25,
                    "WindowCrop packet must be 25 bytes, got {}",
                    data.len()
                );
                let field = |i: usize| {
                    let v = u32::from_le_bytes(data[1 + i * 4..5 + i * 4].try_into().unwrap());
                    (v > 0).then_some(v)
                };
                let window = u64::from_le_bytes(data[17..25].try_into().unwrap());
                Ok(InputPacket::WindowCrop {
                    x: field(0),
                    y: field(1),
                    width: field(2),
                    height: field(3),
                    window,
                })
            }
            TAG_QUALITY_PRESET => {
                anyhow::ensure!(
                    data.len() == 2,
                    "QualityPreset packet must be 2 bytes, got {}",
                    data.len()
                );
                Ok(InputPacket::QualityPreset { preset: data[1] })
            }
            TAG_LINK_FEEDBACK => {
                anyhow::ensure!(
                    data.len() == 17,
                    "LinkFeedback packet must be 17 bytes, got {}",
                    data.len()
                );
                Ok(InputPacket::LinkFeedback {
                    rtt: std::time::Duration::from_micros(u64::from_le_bytes(
                        data[1..9].try_into().unwrap(),
                    )),
                    lost_packets: u64::from_le_bytes(data[9..17].try_into().unwrap()),
                })
            }
            TAG_CLIENT_SAMPLES => {
                anyhow::ensure!(
                    data.len() >= 3,
                    "ClientSamples packet needs a 3-byte header"
                );
                let count = data[2] as usize;
                anyhow::ensure!(
                    data.len() == 3 + count * 4,
                    "ClientSamples payload must match its count, got {} bytes for {count} samples",
                    data.len()
                );
                Ok(InputPacket::ClientSamples {
                    kind: data[1],
                    values: data[3..]
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|chunk| u32::from_le_bytes(*chunk))
                        .collect(),
                })
            }
            _ => {
                anyhow::bail!("Unknown input packet tag: {}", tag);
            }
        }
    }
}

impl From<InputPacket> for Vec<u8> {
    fn from(packet: InputPacket) -> Self {
        packet.encode()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_pen_sample_goes_both_ways_in_ten_bytes() {
        let pen = PenSample {
            x_norm: 12_345,
            y_norm: 65_535,
            pressure: 40_000,
            tilt_x: -30,
            tilt_y: 45,
            flags: PEN_CONTACT | PEN_IN_RANGE | PEN_BARREL,
        };
        let data = InputPacket::Pen(pen).encode();
        assert_eq!(data.len(), 10);
        assert_eq!(data[0], 15, "tags are never renumbered");
        match InputPacket::decode(&data).unwrap() {
            InputPacket::Pen(got) => {
                assert_eq!(got, pen);
                assert!(got.in_contact() && got.in_range() && got.barrel());
            }
            other => panic!("wrong variant: {other:?}"),
        }
        assert!(InputPacket::decode(&data[..9]).is_err(), "short");
    }

    #[test]
    fn a_pen_sample_s_tilt_is_held_to_ninety_degrees_and_contact_means_in_range() {
        let mut data = InputPacket::Pen(PenSample::default()).encode();
        data[7] = (-128i8) as u8;
        data[8] = 127;
        data[9] = PEN_CONTACT | 0xF0;
        let InputPacket::Pen(got) = InputPacket::decode(&data).unwrap() else {
            panic!("not a pen");
        };
        assert_eq!((got.tilt_x, got.tilt_y), (-90, 90));
        assert!(got.in_range(), "a pen in contact is in range");
        assert!(!got.barrel());
        let left = PenSample::default();
        assert!(!left.in_range() && !left.in_contact());
    }

    #[test]
    fn test_mouse_move_roundtrip() {
        let packet = InputPacket::MouseMove { dx: 127, dy: -128 };
        let data = packet.encode();
        let decoded = InputPacket::decode(&data).unwrap();
        match decoded {
            InputPacket::MouseMove { dx, dy } => {
                assert_eq!(dx, 127);
                assert_eq!(dy, -128);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_mouse_move_abs_roundtrip() {
        let packet = InputPacket::MouseMoveAbs {
            x_norm: 0,
            y_norm: 65535,
        };
        let data = packet.encode();
        assert_eq!(data.len(), 5);
        let decoded = InputPacket::decode(&data).unwrap();
        match decoded {
            InputPacket::MouseMoveAbs { x_norm, y_norm } => {
                assert_eq!(x_norm, 0);
                assert_eq!(y_norm, 65535);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_mouse_move_abs_truncated() {
        assert!(InputPacket::decode(&[6, 0, 1]).is_err());
    }

    #[test]
    fn test_quality_preset_roundtrip_and_framing() {
        for preset in [PRESET_AUTO, PRESET_QUALITY, PRESET_BALANCED, PRESET_ECONOMY] {
            let data = InputPacket::QualityPreset { preset }.encode();
            assert_eq!(data, vec![12, preset]);
            match InputPacket::decode(&data).unwrap() {
                InputPacket::QualityPreset { preset: got } => assert_eq!(got, preset),
                other => panic!("wrong variant: {other:?}"),
            }
        }
        // Strict 2-byte framing.
        assert!(InputPacket::decode(&[12]).is_err());
        assert!(InputPacket::decode(&[12, 2, 0]).is_err());
    }

    #[test]
    fn test_preset_bitrate_ceil_is_cap_relative_to_configured() {
        assert_eq!(preset_bitrate_ceil(PRESET_AUTO, 8_000_000), 8_000_000);
        assert_eq!(preset_bitrate_ceil(PRESET_QUALITY, 8_000_000), 8_000_000);
        assert_eq!(preset_bitrate_ceil(PRESET_BALANCED, 8_000_000), 5_000_000);
        assert_eq!(preset_bitrate_ceil(PRESET_ECONOMY, 8_000_000), 1_500_000);
        // Bands are ceilings: a low configured rate is never raised.
        assert_eq!(preset_bitrate_ceil(PRESET_BALANCED, 1_000_000), 1_000_000);
        assert_eq!(preset_bitrate_ceil(PRESET_ECONOMY, 900_000), 900_000);
    }

    #[test]
    fn test_mouse_click_roundtrip() {
        let packet = InputPacket::MouseClick {
            button: 1,
            pressed: true,
        };
        let data = packet.encode();
        let decoded = InputPacket::decode(&data).unwrap();
        match decoded {
            InputPacket::MouseClick { button, pressed } => {
                assert_eq!(button, 1);
                assert!(pressed);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_key_event_roundtrip() {
        let packet = InputPacket::KeyEvent {
            key: 42,
            pressed: false,
        };
        let data = packet.encode();
        let decoded = InputPacket::decode(&data).unwrap();
        match decoded {
            InputPacket::KeyEvent { key, pressed } => {
                assert_eq!(key, 42);
                assert!(!pressed);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_text_roundtrip() {
        let packet = InputPacket::Text("Hello 世界".to_string());
        let data = packet.encode();
        let decoded = InputPacket::decode(&data).unwrap();
        match decoded {
            InputPacket::Text(text) => {
                assert_eq!(text, "Hello 世界");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_empty_data() {
        assert!(InputPacket::decode(&[]).is_err());
    }

    /// Whatever arrives on a phone's input stream, decoding returns an error
    /// or a packet, and never panics (R3: malformed frames fail closed).
    #[test]
    fn decoding_garbage_never_panics() {
        // A small xorshift: deterministic, so a failure reproduces.
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for round in 0..20_000 {
            let len = (next() % 160) as usize;
            let mut bytes: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            // Most rounds start with a real tag, so each decoder sees garbage.
            if let Some(first) = bytes.first_mut()
                && round % 4 != 0
            {
                *first %= 16;
            }
            let _ = InputPacket::decode(&bytes);
        }
    }

    #[test]
    fn test_unknown_tag() {
        assert!(InputPacket::decode(&[0xFF]).is_err());
    }

    #[test]
    fn test_truncated_packet() {
        // MouseMove needs 5 bytes total, we give 2
        assert!(InputPacket::decode(&[0x00, 0x01]).is_err());
    }

    #[test]
    fn test_gamepad_roundtrip() {
        let packet = InputPacket::Gamepad {
            axes: [0, 32767, -32768, 100, -100, 0],
            buttons: 0b1010_0101,
        };
        let data = packet.encode();
        assert_eq!(data.len(), 15); // 1 tag + 12 axes + 2 buttons
        let decoded = InputPacket::decode(&data).unwrap();
        match decoded {
            InputPacket::Gamepad { axes, buttons } => {
                assert_eq!(axes[0], 0);
                assert_eq!(axes[1], 32767);
                assert_eq!(axes[2], -32768);
                assert_eq!(axes[3], 100);
                assert_eq!(axes[4], -100);
                assert_eq!(axes[5], 0);
                assert_eq!(buttons, 0b1010_0101);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_gamepad_truncated() {
        // Need 15 bytes, give 3
        assert!(InputPacket::decode(&[5, 0, 1]).is_err());
    }

    #[test]
    fn test_gamepad_all_buttons() {
        let packet = InputPacket::Gamepad {
            axes: [0; 6],
            buttons: 0xFFFF,
        };
        let data = packet.encode();
        let decoded = InputPacket::decode(&data).unwrap();
        match decoded {
            InputPacket::Gamepad { axes, buttons } => {
                assert_eq!(buttons, 0xFFFF);
                assert!(axes.iter().all(|&a| a == 0));
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_request_keyframe_roundtrip() {
        let packet = InputPacket::RequestKeyframe;
        let data = packet.encode();
        assert_eq!(data.len(), 1);
        let decoded = InputPacket::decode(&data).unwrap();
        assert!(matches!(decoded, InputPacket::RequestKeyframe));
        // Trailing garbage must be rejected so a stats/input framing bug
        // is caught loudly instead of silently mis-parsed.
        assert!(InputPacket::decode(&[7, 0]).is_err());
    }

    #[test]
    fn test_window_crop_roundtrip() {
        let packet = InputPacket::WindowCrop {
            x: Some(100),
            y: Some(60),
            width: Some(1280),
            height: Some(720),
            window: 0xdead_beef_1234,
        };
        let data = packet.encode();
        assert_eq!(data.len(), 25);
        let decoded = InputPacket::decode(&data).unwrap();
        assert_eq!(decoded.crop_rect(), Some((100, 60, 1280, 720)));
        assert_eq!(decoded.window_handle(), Some(0xdead_beef_1234));
    }

    #[test]
    fn test_window_crop_origin_and_clear() {
        // x/y of 0 are legitimate positions: crop_rect must default them to 0.
        let packet = InputPacket::WindowCrop {
            x: None,
            y: None,
            width: Some(800),
            height: Some(600),
            window: 0,
        };
        let decoded = InputPacket::decode(&packet.encode()).unwrap();
        assert_eq!(decoded.crop_rect(), Some((0, 0, 800, 600)));
        assert_eq!(decoded.window_handle(), None);

        // All-zero payload clears the crop.
        let clear = InputPacket::WindowCrop {
            x: None,
            y: None,
            width: None,
            height: None,
            window: 0,
        };
        let data = clear.encode();
        assert_eq!(&data[1..], &[0u8; 24]);
        assert_eq!(InputPacket::decode(&data).unwrap().crop_rect(), None);
    }

    #[test]
    fn test_window_crop_bad_length() {
        assert!(InputPacket::decode(&[8, 0, 1]).is_err());
        let mut full = InputPacket::WindowCrop {
            x: Some(1),
            y: Some(2),
            width: Some(3),
            height: Some(4),
            window: 7,
        }
        .encode();
        full.push(0);
        assert!(InputPacket::decode(&full).is_err());
        // A pre-B2 17-byte frame (no window field) is rejected too.
        assert!(InputPacket::decode(&full[..17]).is_err());
        assert_eq!(
            InputPacket::decode(&full[..25]).unwrap().crop_rect(),
            Some((1, 2, 3, 4))
        );
    }

    #[test]
    fn test_crop_rect_ignores_other_variants() {
        assert_eq!(InputPacket::RequestKeyframe.crop_rect(), None);
    }

    #[test]
    fn test_view_only_roundtrip() {
        for enabled in [true, false] {
            let data = InputPacket::ViewOnly { enabled }.encode();
            assert_eq!(data.len(), 2);
            let decoded = InputPacket::decode(&data).unwrap();
            assert!(
                matches!(decoded, InputPacket::ViewOnly { enabled: e } if e == enabled),
                "wrong variant for {enabled}"
            );
        }
        // Framing is strict like RequestKeyframe: payload size must be exact.
        assert!(InputPacket::decode(&[9]).is_err());
        assert!(InputPacket::decode(&[9, 1, 0]).is_err());
    }

    #[test]
    fn test_full_quality_roundtrip() {
        for enabled in [true, false] {
            let data = InputPacket::FullQuality { enabled }.encode();
            assert_eq!(data, vec![10, u8::from(enabled)]);
            let decoded = InputPacket::decode(&data).unwrap();
            assert!(matches!(decoded, InputPacket::FullQuality { enabled: e } if e == enabled));
        }
        assert!(InputPacket::decode(&[10]).is_err());
        assert!(InputPacket::decode(&[10, 1, 1]).is_err());
    }

    #[test]
    fn test_mic_roundtrip() {
        let packet = InputPacket::Mic {
            enabled: true,
            opus: vec![0x01, 0xFE, 0x20, 0x00, 0x10],
        };
        let data = packet.encode();
        assert_eq!(data.len(), 11); // 6-byte header + 5-byte payload
        let decoded = InputPacket::decode(&data).unwrap();
        match decoded {
            InputPacket::Mic { enabled, opus } => {
                assert!(enabled);
                assert_eq!(opus, vec![0x01, 0xFE, 0x20, 0x00, 0x10]);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_mic_start_and_stop_frames() {
        // Start / keep-alive: enabled with empty payload.
        let start = InputPacket::Mic {
            enabled: true,
            opus: Vec::new(),
        }
        .encode();
        assert_eq!(start, vec![11, 1, 0, 0, 0, 0]);
        // Stop: disabled, empty payload.
        let stop = InputPacket::Mic {
            enabled: false,
            opus: Vec::new(),
        }
        .encode();
        assert_eq!(stop, vec![11, 0, 0, 0, 0, 0]);
        assert!(matches!(
            InputPacket::decode(&stop).unwrap(),
            InputPacket::Mic {
                enabled: false,
                ref opus
            } if opus.is_empty()
        ));
    }

    #[test]
    fn test_mic_framing_strict() {
        // Truncated header, lying length prefix, and trailing garbage all fail.
        assert!(InputPacket::decode(&[11, 1, 0, 0]).is_err());
        assert!(InputPacket::decode(&[11, 1, 5, 0, 0, 0, 1, 2]).is_err());
        assert!(InputPacket::decode(&[11, 1, 0, 0, 0, 0, 9]).is_err());
    }

    #[test]
    fn test_link_feedback_roundtrip_and_framing() {
        let data = InputPacket::LinkFeedback {
            rtt: Duration::from_micros(28_400),
            lost_packets: 12,
        }
        .encode();
        assert_eq!(data.len(), 17);
        assert_eq!(data[0], 13);
        match InputPacket::decode(&data).unwrap() {
            InputPacket::LinkFeedback { rtt, lost_packets } => {
                assert_eq!(rtt, Duration::from_micros(28_400));
                assert_eq!(lost_packets, 12);
            }
            other => panic!("wrong variant: {other:?}"),
        }
        // Strict length: the old untagged 16-byte blob is no longer a shape
        // anything on this connection can send by accident.
        assert!(InputPacket::decode(&data[..16]).is_err());
        assert!(InputPacket::decode(&[&data[..], &[0u8]].concat()).is_err());
    }

    #[test]
    fn test_client_samples_roundtrip_and_framing() {
        for kind in [SAMPLE_DECODE, SAMPLE_RENDER, SAMPLE_E2E] {
            let values = vec![4_000u32, 8_200, 120_000];
            let data = InputPacket::ClientSamples {
                kind,
                values: values.clone(),
            }
            .encode();
            assert_eq!(data.len(), 3 + 12);
            assert_eq!(&data[..3], &[14, kind, 3]);
            match InputPacket::decode(&data).unwrap() {
                InputPacket::ClientSamples {
                    kind: got_kind,
                    values: got,
                } => {
                    assert_eq!(got_kind, kind);
                    assert_eq!(got, values);
                }
                other => panic!("wrong variant: {other:?}"),
            }
        }
        // An empty batch is legal on the wire and decodes as such; the recorder
        // is what decides it says nothing.
        let empty = InputPacket::ClientSamples {
            kind: SAMPLE_DECODE,
            values: Vec::new(),
        }
        .encode();
        assert_eq!(empty, vec![14, 0, 0]);
        assert!(matches!(
            InputPacket::decode(&empty).unwrap(),
            InputPacket::ClientSamples { ref values, .. } if values.is_empty()
        ));
    }

    #[test]
    fn test_client_samples_length_must_match_count() {
        assert!(InputPacket::decode(&[14, 0, 1]).is_err());
        assert!(InputPacket::decode(&[14, 0, 1, 0, 0, 0]).is_err());
        assert!(InputPacket::decode(&[14, 0, 1, 0, 0, 0, 0, 0]).is_err());
        let mut oversized = vec![14u8, 0, 255];
        oversized.extend((0..255 * 4).map(|i| i as u8));
        assert!(InputPacket::decode(&oversized).is_ok());
        oversized.push(0);
        assert!(InputPacket::decode(&oversized).is_err());
    }

    #[test]
    fn sample_batch_and_clamp_bound_what_the_wire_can_carry() {
        assert_eq!(
            3 + MAX_SAMPLES_PER_FRAME * 4,
            InputPacket::ClientSamples {
                kind: SAMPLE_RENDER,
                values: vec![0u32; MAX_SAMPLES_PER_FRAME],
            }
            .encode()
            .len()
        );
        // A caller that overruns gets a valid, shorter frame — never a count
        // that disagrees with the payload.
        let over = InputPacket::ClientSamples {
            kind: SAMPLE_RENDER,
            values: vec![7u32; MAX_SAMPLES_PER_FRAME + 10],
        }
        .encode();
        assert_eq!(over[2] as usize, MAX_SAMPLES_PER_FRAME);
        assert_eq!(over.len(), 3 + MAX_SAMPLES_PER_FRAME * 4);
        assert_eq!(clamp_sample_micros(4_000), 4_000);
        assert_eq!(
            clamp_sample_micros(u64::from(MAX_SAMPLE_MICROS)),
            MAX_SAMPLE_MICROS
        );
        assert_eq!(
            clamp_sample_micros(u64::from(MAX_SAMPLE_MICROS) + 1),
            MAX_SAMPLE_MICROS
        );
    }

    #[test]
    fn sample_batch_drains_in_order_and_bounds_itself() {
        let batch = SampleBatch::default();
        assert!(batch.is_empty());
        assert!(batch.take_chunk().is_none());
        for i in 0..(MAX_SAMPLES_PER_FRAME + 5) {
            assert!(batch.push(i as u64 * 1_000), "kept growing to {i}");
        }
        // First chunk is a full frame's worth, oldest first; the remainder
        // waits for the next flush.
        let first = batch.take_chunk().unwrap();
        assert_eq!(first.len(), MAX_SAMPLES_PER_FRAME);
        assert_eq!(first.first().copied(), Some(0));
        let second = batch.take_chunk().unwrap();
        assert_eq!(second.len(), 5);
        assert_eq!(second[0], 255 * 1_000);
        assert!(batch.take_chunk().is_none());

        // A starved flush loop cannot grow the batch without limit, and what it
        // drops is the oldest sample rather than the stall it just measured.
        let tight = SampleBatch::default();
        for i in 0..(SAMPLE_BATCH_CAPACITY + 100) {
            tight.push(i as u64);
        }
        assert_eq!(tight.len(), SAMPLE_BATCH_CAPACITY);
        let mut last = None;
        while let Some(chunk) = tight.take_chunk() {
            assert!(chunk.len() <= MAX_SAMPLES_PER_FRAME);
            last = chunk.last().copied();
        }
        assert_eq!(
            last,
            Some(SAMPLE_BATCH_CAPACITY as u32 + 99),
            "the newest sample must survive a saturated batch"
        );
        // Anything over the wire range arrives clamped, not wrapped.
        assert!(tight.push(u64::from(MAX_SAMPLE_MICROS) + 10_000));
        assert_eq!(
            tight.take_chunk().unwrap().last().copied(),
            Some(MAX_SAMPLE_MICROS)
        );

        // A batch is per-session: what it did not get to report is forgotten
        // rather than handed to the next one.
        tight.push(5_000);
        tight.clear();
        assert!(tight.is_empty());
        assert!(tight.take_chunk().is_none());
    }

    #[test]
    fn test_from_trait() {
        let packet = InputPacket::MouseMove { dx: 10, dy: -5 };
        let bytes: Vec<u8> = packet.into();
        assert_eq!(bytes.len(), 5);
    }
}
