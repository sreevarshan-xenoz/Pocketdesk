//! The other computer's pointer, drawn here (local cursor): what the
//! computer that is shown tells the one that shows it, so the pointer is
//! drawn where the viewer's own mouse is, with no wait for the picture.
//!
//! Today the shown computer's pointer is part of the video (or missing from
//! it), so a pointer that moves in the window is seen only after the whole
//! pipeline: capture, encode, the link, decode, present. When the shown
//! computer can take its pointer out of the picture, it sends the pointer's
//! shape on a stream of its own ([`crate::streaming::transport::STREAM_KIND_CURSOR`])
//! and its place and visibility as they change, and the window shows the
//! shape as its own system pointer.
//!
//! A message is one type byte and a body, little-endian, carried by one
//! uni-stream behind the stream's 18-byte header (whose sequence orders the
//! messages):
//!
//! | type | body |
//! |---|---|
//! | 0 mode | `[version u8][separate u8]`: whether the picture has no pointer in it |
//! | 1 shape | `[id u32][w u16][h u16][hot_x u16][hot_y u16][RGBA w*h*4]`, straight alpha; it is the pointer's shape from now on |
//! | 2 use | `[id u32]`: the shape sent before under that id, again |
//! | 3 visible | `[visible u8]` |
//! | 4 move | `[x u16][y u16]`: the hotspot's place over the picture, `0..=65535` as [`InputPacket::MouseMoveAbs`](crate::streaming::InputPacket::MouseMoveAbs) says it; sent only while the viewer asked for places |
//!
//! Compatibility: a computer sends none of it unless the viewer said it
//! draws the pointer ([`crate::streaming::client::CODEC_CAP_LOCAL_CURSOR`]),
//! and says in its first message ([`CursorMessage::Mode`]) whether the
//! picture has the pointer taken out; a computer that cannot (or an older
//! one that never answers) keeps the pointer in the picture, and a viewer
//! that has heard no `Mode` that says `separate` hides and draws nothing.
//! A message type a reader does not know is skipped, so more can be added.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// The version a `Mode` message names.
pub const VERSION: u8 = 1;

/// The largest side of a pointer image, in pixels. A larger one is
/// shrunk by a whole factor ([`CursorShape::fitted`]).
pub const MAX_SIDE: u32 = 256;

/// The most bytes one message holds: the largest image and its header.
pub const MAX_MESSAGE: usize = 1 + 4 + 8 + (MAX_SIDE * MAX_SIDE * 4) as usize;

/// How many shapes a reader keeps to answer a `Use`.
pub const CACHE_SHAPES: usize = 48;

const TYPE_MODE: u8 = 0;
const TYPE_SHAPE: u8 = 1;
const TYPE_USE: u8 = 2;
const TYPE_VISIBLE: u8 = 3;
const TYPE_MOVE: u8 = 4;

/// One pointer image with its hotspot, as straight-alpha RGBA.
#[derive(Clone, PartialEq, Eq)]
pub struct CursorShape {
    /// A hash of the image and hotspot: the shape's name on the wire.
    pub id: u32,
    pub width: u16,
    pub height: u16,
    /// The pixel of the image that is the pointer's place.
    pub hot_x: u16,
    pub hot_y: u16,
    pub rgba: Arc<[u8]>,
}

impl std::fmt::Debug for CursorShape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CursorShape")
            .field("id", &self.id)
            .field("size", &(self.width, self.height))
            .field("hot", &(self.hot_x, self.hot_y))
            .finish()
    }
}

impl CursorShape {
    /// A shape of `width` by `height` straight-alpha RGBA pixels, with its
    /// hotspot (moved inside the image). `None` when the size is zero or
    /// over [`MAX_SIDE`], or `rgba` is not that many pixels: see
    /// [`CursorShape::fitted`] for a larger one.
    pub fn new(width: u32, height: u32, hot_x: u32, hot_y: u32, rgba: Vec<u8>) -> Option<Self> {
        if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
            return None;
        }
        if rgba.len() != (width * height * 4) as usize {
            return None;
        }
        let hot_x = hot_x.min(width - 1);
        let hot_y = hot_y.min(height - 1);
        let id = shape_id(width, height, hot_x, hot_y, &rgba);
        Some(Self {
            id,
            width: width as u16,
            height: height as u16,
            hot_x: hot_x as u16,
            hot_y: hot_y as u16,
            rgba: rgba.into(),
        })
    }

    /// As [`CursorShape::new`], but a pointer larger than [`MAX_SIDE`] is
    /// shrunk by the smallest whole factor that fits it (every n-th pixel,
    /// the hotspot with it); such a pointer is an accessibility size and
    /// rare. `None` only for a size of zero or pixels that do not add up.
    pub fn fitted(width: u32, height: u32, hot_x: u32, hot_y: u32, rgba: Vec<u8>) -> Option<Self> {
        if width == 0 || height == 0 || rgba.len() != (width as usize) * (height as usize) * 4 {
            return None;
        }
        let factor = width.max(height).div_ceil(MAX_SIDE);
        if factor <= 1 {
            return Self::new(width, height, hot_x, hot_y, rgba);
        }
        let (w, h) = (width / factor, height / factor);
        if w == 0 || h == 0 {
            return None;
        }
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let at = (((y * factor) * width + x * factor) * 4) as usize;
                out.extend_from_slice(&rgba[at..at + 4]);
            }
        }
        Self::new(w, h, hot_x / factor, hot_y / factor, out)
    }

    /// The pixels' byte count (`w * h * 4`).
    pub fn bytes(&self) -> usize {
        self.rgba.len()
    }
}

/// FNV-1a over the size, hotspot and pixels, folded to 32 bits.
fn shape_id(width: u32, height: u32, hot_x: u32, hot_y: u32, rgba: &[u8]) -> u32 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for &b in bytes {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    eat(&width.to_le_bytes());
    eat(&height.to_le_bytes());
    eat(&hot_x.to_le_bytes());
    eat(&hot_y.to_le_bytes());
    eat(rgba);
    ((hash >> 32) ^ hash) as u32
}

/// Straight-alpha RGBA from `width` by `height` premultiplied pixels in
/// memory order B, G, R, A (Wayland's ARGB8888, `stride` bytes a row, and
/// X8R8G8B8 with its fourth byte ignored when `has_alpha` is false): the
/// form a pointer image comes in from a compositor.
pub fn rgba_from_premultiplied_bgra(
    src: &[u8],
    width: u32,
    height: u32,
    stride: u32,
    has_alpha: bool,
) -> Option<Vec<u8>> {
    let (w, h, stride) = (width as usize, height as usize, stride as usize);
    if stride < w * 4 || src.len() < stride.checked_mul(h)? {
        return None;
    }
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = &src[y * stride..y * stride + w * 4];
        for x in 0..w {
            let px = &row[x * 4..x * 4 + 4];
            let a = if has_alpha { px[3] } else { 255 };
            let un = |c: u8| -> u8 {
                if a == 0 || a == 255 {
                    c
                } else {
                    ((u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8
                }
            };
            out.extend_from_slice(&[un(px[2]), un(px[1]), un(px[0]), a]);
        }
    }
    Some(out)
}

/// What the computer that is shown says of its pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorMessage {
    /// Whether the picture has the pointer taken out of it. Sent first.
    Mode { separate: bool },
    /// The pointer's shape from now on.
    Shape(CursorShape),
    /// The shape sent before as `id`, again.
    Use { id: u32 },
    /// The pointer shows (or not).
    Visible(bool),
    /// Where the pointer's hotspot is over the picture.
    Move { x: u16, y: u16 },
}

/// Why a message was not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorError {
    Empty,
    Truncated,
    /// A shape of a size or with pixels that do not add up.
    BadShape,
}

impl std::fmt::Display for CursorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            CursorError::Empty => "an empty pointer message",
            CursorError::Truncated => "a pointer message cut short",
            CursorError::BadShape => "a pointer shape that does not add up",
        })
    }
}

impl std::error::Error for CursorError {}

impl CursorMessage {
    /// The message as it goes on the wire.
    pub fn encode(&self) -> Vec<u8> {
        match self {
            CursorMessage::Mode { separate } => vec![TYPE_MODE, VERSION, u8::from(*separate)],
            CursorMessage::Shape(shape) => {
                let mut out = Vec::with_capacity(13 + shape.bytes());
                out.push(TYPE_SHAPE);
                out.extend_from_slice(&shape.id.to_le_bytes());
                for v in [shape.width, shape.height, shape.hot_x, shape.hot_y] {
                    out.extend_from_slice(&v.to_le_bytes());
                }
                out.extend_from_slice(&shape.rgba);
                out
            }
            CursorMessage::Use { id } => {
                let mut out = vec![TYPE_USE];
                out.extend_from_slice(&id.to_le_bytes());
                out
            }
            CursorMessage::Visible(visible) => vec![TYPE_VISIBLE, u8::from(*visible)],
            CursorMessage::Move { x, y } => {
                let mut out = vec![TYPE_MOVE];
                out.extend_from_slice(&x.to_le_bytes());
                out.extend_from_slice(&y.to_le_bytes());
                out
            }
        }
    }

    /// Read one message. `Ok(None)` is a type this build does not know: it
    /// is skipped, as a newer computer may send more.
    pub fn decode(data: &[u8]) -> Result<Option<Self>, CursorError> {
        let (&kind, body) = data.split_first().ok_or(CursorError::Empty)?;
        let u16_at = |at: usize| -> Result<u16, CursorError> {
            body.get(at..at + 2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .ok_or(CursorError::Truncated)
        };
        Ok(Some(match kind {
            TYPE_MODE => {
                let &separate = body.get(1).ok_or(CursorError::Truncated)?;
                CursorMessage::Mode {
                    separate: separate != 0,
                }
            }
            TYPE_SHAPE => {
                let id = body
                    .get(0..4)
                    .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .ok_or(CursorError::Truncated)?;
                let (w, h) = (u32::from(u16_at(4)?), u32::from(u16_at(6)?));
                let (hx, hy) = (u32::from(u16_at(8)?), u32::from(u16_at(10)?));
                let pixels = body.get(12..).ok_or(CursorError::Truncated)?;
                let shape =
                    CursorShape::new(w, h, hx, hy, pixels.to_vec()).ok_or(CursorError::BadShape)?;
                // The sender's name for it is the one its `Use` will say.
                CursorMessage::Shape(CursorShape { id, ..shape })
            }
            TYPE_USE => CursorMessage::Use {
                id: body
                    .get(0..4)
                    .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .ok_or(CursorError::Truncated)?,
            },
            TYPE_VISIBLE => {
                CursorMessage::Visible(*body.first().ok_or(CursorError::Truncated)? != 0)
            }
            TYPE_MOVE => CursorMessage::Move {
                x: u16_at(0)?,
                y: u16_at(2)?,
            },
            _ => return Ok(None),
        }))
    }
}

/// A few shapes by id, the oldest forgotten first.
#[derive(Debug, Default)]
pub struct ShapeCache {
    shapes: HashMap<u32, Arc<CursorShape>>,
    order: VecDeque<u32>,
}

impl ShapeCache {
    pub fn get(&self, id: u32) -> Option<Arc<CursorShape>> {
        self.shapes.get(&id).cloned()
    }

    pub fn contains(&self, id: u32) -> bool {
        self.shapes.contains_key(&id)
    }

    /// Keep `shape`, forgetting the oldest beyond [`CACHE_SHAPES`].
    pub fn put(&mut self, shape: Arc<CursorShape>) {
        let id = shape.id;
        if self.shapes.insert(id, shape).is_none() {
            self.order.push_back(id);
            while self.order.len() > CACHE_SHAPES {
                if let Some(old) = self.order.pop_front() {
                    self.shapes.remove(&old);
                }
            }
        }
    }

    pub fn len(&self) -> usize {
        self.shapes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }
}

/// The other computer's pointer as a viewer holds it: fed the messages in
/// order, it says what to draw.
#[derive(Debug, Default)]
pub struct CursorState {
    /// The computer said the picture has no pointer in it, so the viewer
    /// draws it. False until it says so (and for good from one that does
    /// not).
    pub separate: bool,
    pub visible: bool,
    pub shape: Option<Arc<CursorShape>>,
    /// The hotspot's place over the picture, once told (`0..=65535`).
    pub place: Option<(u16, u16)>,
    cache: ShapeCache,
}

impl CursorState {
    /// Take one message in. Whether what is drawn changed.
    pub fn apply(&mut self, message: CursorMessage) -> bool {
        match message {
            CursorMessage::Mode { separate } => {
                let changed = self.separate != separate;
                self.separate = separate;
                if !separate {
                    *self = Self::default();
                } else if changed {
                    self.visible = true;
                }
                changed
            }
            // Nothing is drawn until the computer said the picture has no
            // pointer, whatever it sends.
            _ if !self.separate => false,
            // The host says the current shape whole again now and then (a
            // message may have been lost): the same one is no change, and
            // keeps the image the window already made its pointer of.
            CursorMessage::Shape(shape) => {
                if let Some(held) = self.shape.clone().filter(|s| s.id == shape.id) {
                    self.cache.put(held);
                    return false;
                }
                let shape = Arc::new(shape);
                self.cache.put(shape.clone());
                self.shape = Some(shape);
                true
            }
            CursorMessage::Use { id } => match self.cache.get(id) {
                Some(shape) => {
                    let changed = self.shape.as_ref().is_none_or(|s| s.id != id);
                    self.shape = Some(shape);
                    changed
                }
                None => false,
            },
            CursorMessage::Visible(visible) => {
                let changed = self.visible != visible;
                self.visible = visible;
                changed
            }
            CursorMessage::Move { x, y } => {
                let changed = self.place != Some((x, y));
                self.place = Some((x, y));
                changed
            }
        }
    }

    /// The messages that bring a viewer that joins now to this state.
    pub fn snapshot(&self) -> Vec<CursorMessage> {
        if !self.separate {
            return Vec::new();
        }
        let mut out = vec![CursorMessage::Mode { separate: true }];
        if let Some(shape) = &self.shape {
            out.push(CursorMessage::Shape((**shape).clone()));
        }
        out.push(CursorMessage::Visible(self.visible));
        if let Some((x, y)) = self.place {
            out.push(CursorMessage::Move { x, y });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(seed: u8, w: u32, h: u32) -> CursorShape {
        let rgba: Vec<u8> = (0..w * h * 4)
            .map(|i| (i as u8).wrapping_add(seed))
            .collect();
        CursorShape::new(w, h, 1, 2, rgba).unwrap()
    }

    #[test]
    fn every_message_reads_back_as_it_was_sent() {
        let shape = shape(7, 4, 3);
        for message in [
            CursorMessage::Mode { separate: true },
            CursorMessage::Mode { separate: false },
            CursorMessage::Shape(shape.clone()),
            CursorMessage::Use { id: 0xdead_beef },
            CursorMessage::Visible(true),
            CursorMessage::Visible(false),
            CursorMessage::Move { x: 0, y: 65535 },
        ] {
            let wire = message.encode();
            assert_eq!(CursorMessage::decode(&wire).unwrap(), Some(message));
        }
    }

    #[test]
    fn a_shape_is_named_by_its_pixels_and_hotspot() {
        let a = shape(1, 4, 4);
        assert_eq!(a.id, shape(1, 4, 4).id, "the same pointer, the same name");
        assert_ne!(a.id, shape(2, 4, 4).id);
        let moved = CursorShape::new(4, 4, 0, 0, a.rgba.to_vec()).unwrap();
        assert_ne!(a.id, moved.id, "the hotspot is part of it");
    }

    #[test]
    fn a_hotspot_outside_the_image_is_moved_inside() {
        let s = CursorShape::new(2, 2, 40, 50, vec![0; 16]).unwrap();
        assert_eq!((s.hot_x, s.hot_y), (1, 1));
    }

    #[test]
    fn a_message_a_reader_does_not_know_is_skipped_not_refused() {
        // A newer computer's type 9, and an older reader's view of it.
        assert_eq!(CursorMessage::decode(&[9, 1, 2, 3]).unwrap(), None);
    }

    #[test]
    fn a_cut_or_empty_message_is_an_error_not_a_panic() {
        assert_eq!(CursorMessage::decode(&[]), Err(CursorError::Empty));
        assert_eq!(
            CursorMessage::decode(&[TYPE_MODE]),
            Err(CursorError::Truncated)
        );
        assert_eq!(
            CursorMessage::decode(&[TYPE_USE, 1, 2]),
            Err(CursorError::Truncated)
        );
        assert_eq!(
            CursorMessage::decode(&[TYPE_MOVE, 1, 2, 3]),
            Err(CursorError::Truncated)
        );
        let mut wire = CursorMessage::Shape(shape(3, 4, 4)).encode();
        wire.truncate(wire.len() - 1);
        assert_eq!(CursorMessage::decode(&wire), Err(CursorError::BadShape));
        wire.truncate(9);
        assert_eq!(CursorMessage::decode(&wire), Err(CursorError::Truncated));
    }

    #[test]
    fn a_shape_over_the_cap_is_refused_on_the_wire() {
        let mut wire = vec![TYPE_SHAPE, 0, 0, 0, 0];
        for v in [300u16, 300, 0, 0] {
            wire.extend_from_slice(&v.to_le_bytes());
        }
        wire.extend(vec![0u8; 300 * 300 * 4]);
        assert_eq!(CursorMessage::decode(&wire), Err(CursorError::BadShape));
        assert!(MAX_MESSAGE >= 13 + (MAX_SIDE * MAX_SIDE * 4) as usize);
    }

    #[test]
    fn a_larger_pointer_is_shrunk_by_a_whole_factor() {
        let big = CursorShape::fitted(512, 300, 100, 40, vec![9; 512 * 300 * 4]).unwrap();
        assert_eq!((big.width, big.height), (256, 150));
        assert_eq!((big.hot_x, big.hot_y), (50, 20));
        let fits = CursorShape::fitted(256, 256, 0, 0, vec![9; 256 * 256 * 4]).unwrap();
        assert_eq!((fits.width, fits.height), (256, 256));
        assert!(CursorShape::fitted(0, 4, 0, 0, vec![]).is_none());
        assert!(CursorShape::fitted(4, 4, 0, 0, vec![0; 3]).is_none());
    }

    #[test]
    fn a_premultiplied_pixel_comes_out_straight_and_rgba() {
        // B, G, R, A in memory: half-transparent pure red, premultiplied.
        let px = [0u8, 0, 128, 128];
        let out = rgba_from_premultiplied_bgra(&px, 1, 1, 4, true).unwrap();
        assert_eq!(out, vec![255, 0, 0, 128]);
        // Opaque and fully clear pixels pass through; X8R8G8B8 is opaque.
        let out =
            rgba_from_premultiplied_bgra(&[10, 20, 30, 255, 1, 2, 3, 0], 2, 1, 8, true).unwrap();
        assert_eq!(out, vec![30, 20, 10, 255, 3, 2, 1, 0]);
        let out = rgba_from_premultiplied_bgra(&[10, 20, 30, 0], 1, 1, 4, false).unwrap();
        assert_eq!(out, vec![30, 20, 10, 255]);
        // Stride padding is skipped; a short buffer is refused.
        let out = rgba_from_premultiplied_bgra(
            &[1, 2, 3, 255, 9, 9, 9, 9, 4, 5, 6, 255, 9, 9, 9, 9],
            1,
            2,
            8,
            true,
        )
        .unwrap();
        assert_eq!(out, vec![3, 2, 1, 255, 6, 5, 4, 255]);
        assert!(rgba_from_premultiplied_bgra(&[0; 7], 2, 1, 8, true).is_none());
    }

    #[test]
    fn the_cache_forgets_the_oldest_shape_past_its_size() {
        let mut cache = ShapeCache::default();
        let first = Arc::new(shape(0, 2, 2));
        cache.put(first.clone());
        cache.put(first.clone());
        assert_eq!(cache.len(), 1, "the same id is kept once");
        for seed in 1..=CACHE_SHAPES as u8 {
            cache.put(Arc::new(shape(seed, 2, 2)));
        }
        assert_eq!(cache.len(), CACHE_SHAPES);
        assert!(!cache.contains(first.id), "the oldest went");
    }

    #[test]
    fn a_viewer_draws_nothing_until_the_computer_says_the_picture_has_no_pointer() {
        let mut state = CursorState::default();
        assert!(!state.apply(CursorMessage::Shape(shape(1, 2, 2))));
        assert!(state.shape.is_none());
        assert!(!state.apply(CursorMessage::Mode { separate: false }));
        assert!(!state.apply(CursorMessage::Visible(false)));
        assert!(!state.separate);
        assert!(state.apply(CursorMessage::Mode { separate: true }));
        assert!(state.separate && state.visible);
        assert!(state.apply(CursorMessage::Shape(shape(1, 2, 2))));
        assert!(state.shape.is_some());
    }

    #[test]
    fn a_shape_is_used_again_by_its_name_and_an_unknown_name_changes_nothing() {
        let mut state = CursorState::default();
        state.apply(CursorMessage::Mode { separate: true });
        let (a, b) = (shape(1, 2, 2), shape(2, 2, 2));
        state.apply(CursorMessage::Shape(a.clone()));
        state.apply(CursorMessage::Shape(b.clone()));
        assert_eq!(state.shape.as_ref().unwrap().id, b.id);
        assert!(state.apply(CursorMessage::Use { id: a.id }));
        assert_eq!(state.shape.as_ref().unwrap().id, a.id);
        assert!(!state.apply(CursorMessage::Use { id: a.id }), "already so");
        assert!(!state.apply(CursorMessage::Use { id: 12345 }), "never sent");
        assert_eq!(state.shape.as_ref().unwrap().id, a.id);
    }

    #[test]
    fn the_current_shape_said_again_is_no_change_and_keeps_the_image_held() {
        let mut state = CursorState::default();
        state.apply(CursorMessage::Mode { separate: true });
        let a = shape(1, 2, 2);
        assert!(state.apply(CursorMessage::Shape(a.clone())));
        let held = state.shape.clone().unwrap();
        assert!(
            !state.apply(CursorMessage::Shape(a.clone())),
            "the host's refresh"
        );
        assert!(
            Arc::ptr_eq(&held, state.shape.as_ref().unwrap()),
            "the window keeps the pointer it made of the image"
        );
        // A shape that was lost and came whole by a refresh is then usable
        // by its name.
        let b = shape(2, 2, 2);
        state.apply(CursorMessage::Shape(b.clone()));
        assert!(state.apply(CursorMessage::Use { id: a.id }));
    }

    #[test]
    fn a_mode_that_says_embedded_clears_everything_drawn() {
        let mut state = CursorState::default();
        state.apply(CursorMessage::Mode { separate: true });
        state.apply(CursorMessage::Shape(shape(1, 2, 2)));
        state.apply(CursorMessage::Move { x: 5, y: 6 });
        assert!(state.apply(CursorMessage::Mode { separate: false }));
        assert!(state.shape.is_none() && state.place.is_none() && !state.visible);
    }

    #[test]
    fn a_viewer_that_joins_late_is_brought_to_the_state_by_a_snapshot() {
        let mut state = CursorState::default();
        assert!(
            state.snapshot().is_empty(),
            "nothing separate, nothing to say"
        );
        state.apply(CursorMessage::Mode { separate: true });
        state.apply(CursorMessage::Shape(shape(4, 3, 3)));
        state.apply(CursorMessage::Visible(false));
        state.apply(CursorMessage::Move { x: 1, y: 2 });
        let mut late = CursorState::default();
        for message in state.snapshot() {
            late.apply(message);
        }
        assert!(late.separate && !late.visible);
        assert_eq!(
            late.shape.as_ref().map(|s| s.id),
            state.shape.as_ref().map(|s| s.id)
        );
        assert_eq!(late.place, Some((1, 2)));
    }
}
