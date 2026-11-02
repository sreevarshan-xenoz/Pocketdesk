//! The pointer in a PipeWire screen stream's cursor metadata
//! (`SPA_META_Cursor`), for the portal and mutter captures.
//!
//! A screencast asked for the pointer as metadata (the portal's
//! `cursor_mode` Metadata, mutter's `cursor-mode` 2) leaves it out of the
//! picture and attaches to each buffer a small record: an id (0 when there
//! is no pointer), its place, its hotspot and, only when the image changed,
//! a bitmap. [`PwCursor`] reads that record and reports it to the session's
//! [`CursorFeed`]: a shape when a bitmap came, whether the pointer shows,
//! and its place. A compositor sends a bitmap on a change only, but not
//! all do, so a bitmap is compared with the last one before anything is
//! done with it: a pointer that does not change costs a comparison of a few
//! kilobytes a frame, and nothing is sent.

use anyhow::Result;
use libspa::sys as spa;

use super::cursor_feed::{CursorFeed, normalized};
use crate::protocol::cursor::{CursorShape, MAX_SIDE, rgba_from_premultiplied_bgra};

/// `sizeof(struct spa_meta_cursor)`: id, flags, position, hotspot, bitmap offset.
const CURSOR_HEADER: usize = 28;
/// `sizeof(struct spa_meta_bitmap)`: format, size, stride, offset.
const BITMAP_HEADER: usize = 20;

/// The pointer's record in a buffer, as the compositor wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaCursor {
    /// 0: there is no pointer in this buffer.
    pub id: u32,
    pub x: i32,
    pub y: i32,
    pub hot_x: i32,
    pub hot_y: i32,
    pub bitmap: Option<MetaBitmap>,
}

/// A pointer image in a record, premultiplied, in its own format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaBitmap {
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixels: Vec<u8>,
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    data.get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn i32_at(data: &[u8], at: usize) -> Option<i32> {
    u32_at(data, at).map(|v| v as i32)
}

/// Read a `spa_meta_cursor` record from the `data` of its meta (every field
/// bounds-checked: the buffer is another process's). `None` when it is too
/// short to hold the header. A bitmap that does not fit the record is left
/// out, not guessed at.
pub fn parse(data: &[u8]) -> Option<MetaCursor> {
    if data.len() < CURSOR_HEADER {
        return None;
    }
    let bitmap_offset = u32_at(data, 24)? as usize;
    let mut cursor = MetaCursor {
        id: u32_at(data, 0)?,
        x: i32_at(data, 8)?,
        y: i32_at(data, 12)?,
        hot_x: i32_at(data, 16)?,
        hot_y: i32_at(data, 20)?,
        bitmap: None,
    };
    // An offset of 0 is no image data; one inside the header is not a bitmap.
    if bitmap_offset >= CURSOR_HEADER
        && let Some(header) = data.get(bitmap_offset..bitmap_offset + BITMAP_HEADER)
    {
        let format = u32_at(header, 0)?;
        let (width, height) = (u32_at(header, 4)?, u32_at(header, 8)?);
        let stride = i32_at(header, 12)?;
        let pixels_at = u32_at(header, 16)? as usize;
        // Format 0 is "no new bitmap"; a bottom-up or empty one is not read.
        if format != 0 && width > 0 && height > 0 && stride > 0 {
            let stride = stride as usize;
            let start = bitmap_offset.checked_add(pixels_at)?;
            let end = start.checked_add(stride.checked_mul(height as usize)?)?;
            if stride >= (width as usize).checked_mul(4)?
                && let Some(pixels) = data.get(start..end)
            {
                cursor.bitmap = Some(MetaBitmap {
                    format,
                    width,
                    height,
                    stride: stride as u32,
                    pixels: pixels.to_vec(),
                });
            }
        }
    }
    Some(cursor)
}

/// Straight RGBA from a record's bitmap, when it is in a format read here
/// (BGRA, BGRx, RGBA, RGBx; every compositor's pointer is one of them).
fn rgba_of(bitmap: &MetaBitmap) -> Option<Vec<u8>> {
    use libspa::param::video::VideoFormat as F;
    let f = bitmap.format;
    let (swap, alpha) = if f == F::BGRA.as_raw() {
        (false, true)
    } else if f == F::BGRx.as_raw() {
        (false, false)
    } else if f == F::RGBA.as_raw() {
        (true, true)
    } else if f == F::RGBx.as_raw() {
        (true, false)
    } else {
        return None;
    };
    if !swap {
        return rgba_from_premultiplied_bgra(
            &bitmap.pixels,
            bitmap.width,
            bitmap.height,
            bitmap.stride,
            alpha,
        );
    }
    // R, G, B, A in memory: swap R and B, then read it as B, G, R, A.
    let mut swapped = bitmap.pixels.clone();
    for row in swapped.chunks_mut(bitmap.stride as usize) {
        for x in 0..bitmap.width as usize {
            row.swap(x * 4, x * 4 + 2);
        }
    }
    rgba_from_premultiplied_bgra(&swapped, bitmap.width, bitmap.height, bitmap.stride, alpha)
}

/// The `Meta` parameter that asks the stream for the pointer's record:
/// `SPA_META_Cursor`, room for an image of up to [`MAX_SIDE`] square (64
/// by default).
pub fn meta_param_pod() -> Result<Vec<u8>> {
    use libspa::pod::{ChoiceValue, Object, Property, Value};
    use libspa::utils::{Choice, ChoiceEnum, ChoiceFlags, Id, SpaTypes};
    let size = |side: u32| (CURSOR_HEADER + BITMAP_HEADER + (side * side * 4) as usize) as i32;
    let object = Object {
        type_: SpaTypes::ObjectParamMeta.as_raw(),
        id: libspa::param::ParamType::Meta.as_raw(),
        properties: vec![
            Property::new(
                spa::SPA_PARAM_META_type,
                Value::Id(Id(spa::SPA_META_Cursor)),
            ),
            Property::new(
                spa::SPA_PARAM_META_size,
                Value::Choice(ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Range {
                        default: size(64),
                        min: size(1),
                        max: size(MAX_SIDE),
                    },
                ))),
            ),
        ],
    };
    let (cursor, _) = libspa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &Value::Object(object),
    )
    .map_err(|e| anyhow::anyhow!("The pointer's meta param did not serialize: {e:?}"))?;
    Ok(cursor.into_inner())
}

/// The pointer's record in the buffer `buffer`, as bytes: the meta of type
/// cursor, when it has one.
///
/// # Safety
/// `buffer` is null or a valid `spa_buffer` that stays valid and unmodified
/// for the returned slice's life.
pub unsafe fn cursor_meta<'a>(buffer: *const spa::spa_buffer) -> Option<&'a [u8]> {
    if buffer.is_null() {
        return None;
    }
    // SAFETY: the caller's guarantee; `metas` holds `n_metas` entries.
    unsafe {
        let b = &*buffer;
        if b.metas.is_null() {
            return None;
        }
        for i in 0..b.n_metas as usize {
            let meta = &*b.metas.add(i);
            if meta.type_ == spa::SPA_META_Cursor && !meta.data.is_null() && meta.size > 0 {
                return Some(std::slice::from_raw_parts(
                    meta.data as *const u8,
                    meta.size as usize,
                ));
            }
        }
    }
    None
}

/// Reads a stream's pointer records and tells the feed what changed.
pub struct PwCursor {
    feed: CursorFeed,
    /// The last bitmap's pixels, to tell a repeat from a new image.
    last: Option<MetaBitmap>,
}

impl PwCursor {
    /// `feed` is the session's. The capture that asked for the metadata
    /// said so to it already ([`CursorFeed::set_separate`]), before its
    /// session started, so the streamer knows whether the picture has the
    /// pointer in it.
    pub fn new(feed: CursorFeed) -> Self {
        Self { feed, last: None }
    }

    /// One buffer's record, over a picture of `width` by `height`.
    pub fn update(&mut self, record: &[u8], width: u32, height: u32) {
        let Some(cursor) = parse(record) else { return };
        if let Some(bitmap) = cursor.bitmap
            && self.last.as_ref() != Some(&bitmap)
        {
            if let Some(rgba) = rgba_of(&bitmap)
                && let Some(shape) = CursorShape::fitted(
                    bitmap.width,
                    bitmap.height,
                    cursor.hot_x.max(0) as u32,
                    cursor.hot_y.max(0) as u32,
                    rgba,
                )
            {
                self.feed.set_shape(shape);
            }
            self.last = Some(bitmap);
        }
        self.feed.set_visible(cursor.id != 0);
        if cursor.id != 0 {
            let (x, y) = normalized(cursor.x, cursor.y, width, height);
            self.feed.set_place(x, y);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record as a compositor writes it: header, bitmap header, pixels.
    fn record(
        id: u32,
        pos: (i32, i32),
        hot: (i32, i32),
        bitmap: Option<(u32, u32, u32, &[u8])>,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        for v in [
            id,
            0,
            pos.0 as u32,
            pos.1 as u32,
            hot.0 as u32,
            hot.1 as u32,
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        let offset = if bitmap.is_some() {
            CURSOR_HEADER as u32
        } else {
            0
        };
        out.extend_from_slice(&offset.to_le_bytes());
        if let Some((format, w, h, pixels)) = bitmap {
            for v in [format, w, h, w * 4, BITMAP_HEADER as u32] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend_from_slice(pixels);
        }
        out
    }

    fn bgra() -> u32 {
        libspa::param::video::VideoFormat::BGRA.as_raw()
    }

    #[test]
    fn a_record_with_an_image_is_read_whole() {
        let pixels = [10u8, 20, 30, 255, 1, 2, 3, 255];
        let wire = record(7, (100, 50), (1, 2), Some((bgra(), 2, 1, &pixels)));
        let c = parse(&wire).unwrap();
        assert_eq!((c.id, c.x, c.y, c.hot_x, c.hot_y), (7, 100, 50, 1, 2));
        let b = c.bitmap.unwrap();
        assert_eq!((b.width, b.height, b.stride), (2, 1, 8));
        assert_eq!(b.pixels, pixels);
    }

    #[test]
    fn a_record_without_an_image_or_with_a_bad_one_has_no_bitmap_and_never_panics() {
        assert_eq!(
            parse(&record(1, (0, 0), (0, 0), None)).unwrap().bitmap,
            None
        );
        assert!(parse(&[0; 27]).is_none(), "shorter than the header");
        // A bitmap that claims more pixels than the record holds is left out.
        let mut wire = record(1, (0, 0), (0, 0), Some((bgra(), 2, 2, &[0; 16])));
        wire.truncate(wire.len() - 4);
        assert_eq!(parse(&wire).unwrap().bitmap, None);
        // Format 0 is "no new bitmap"; a huge stride or offset cannot overflow.
        let wire = record(1, (0, 0), (0, 0), Some((0, 2, 2, &[0; 16])));
        assert_eq!(parse(&wire).unwrap().bitmap, None);
        let mut wire = record(1, (0, 0), (0, 0), Some((bgra(), 1, 1, &[0; 4])));
        wire[CURSOR_HEADER + 12..CURSOR_HEADER + 16].copy_from_slice(&i32::MAX.to_le_bytes());
        assert_eq!(parse(&wire).unwrap().bitmap, None);
        let mut wire = record(1, (0, 0), (0, 0), Some((bgra(), 1, 1, &[0; 4])));
        wire[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(parse(&wire).unwrap().bitmap, None);
    }

    #[test]
    fn the_pointer_is_reported_as_shape_visibility_and_place() {
        let feed = CursorFeed::new();
        let mut pw = PwCursor::new(feed.clone());
        feed.set_tracking(true);
        let mut marks = super::super::cursor_feed::Marks::default();
        feed.take(&mut marks);
        let pixels = [0u8, 0, 255, 255];
        pw.update(
            &record(3, (960, 540), (0, 0), Some((bgra(), 1, 1, &pixels))),
            1920,
            1080,
        );
        let got = feed.take(&mut marks);
        assert_eq!(
            got.shape.as_ref().unwrap().rgba.as_ref(),
            &[255, 0, 0, 255],
            "red, as RGBA"
        );
        assert_eq!(got.place, Some(normalized(960, 540, 1920, 1080)));
        // The same bitmap again, and a move: no new shape, a new place.
        pw.update(
            &record(3, (961, 540), (0, 0), Some((bgra(), 1, 1, &pixels))),
            1920,
            1080,
        );
        let got = feed.take(&mut marks);
        assert!(got.shape.is_none(), "a repeated image is nothing");
        assert!(got.place.is_some());
        // No pointer in the buffer: hidden.
        pw.update(&record(0, (0, 0), (0, 0), None), 1920, 1080);
        assert_eq!(feed.take(&mut marks).visible, Some(false));
    }

    #[test]
    fn rgba_pointers_are_swapped_and_an_unknown_format_gives_no_shape() {
        use libspa::param::video::VideoFormat as F;
        let feed = CursorFeed::new();
        let mut pw = PwCursor::new(feed.clone());
        let mut marks = super::super::cursor_feed::Marks::default();
        feed.take(&mut marks);
        pw.update(
            &record(
                1,
                (0, 0),
                (0, 0),
                Some((F::RGBA.as_raw(), 1, 1, &[255, 0, 0, 255])),
            ),
            10,
            10,
        );
        assert_eq!(
            feed.take(&mut marks).shape.unwrap().rgba.as_ref(),
            &[255, 0, 0, 255]
        );
        pw.update(
            &record(
                1,
                (0, 0),
                (0, 0),
                Some((F::I420.as_raw(), 1, 1, &[1, 2, 3, 4])),
            ),
            10,
            10,
        );
        assert!(feed.take(&mut marks).shape.is_none());
    }

    #[test]
    fn the_stream_is_asked_for_a_cursor_record_up_to_the_cap() {
        use libspa::pod::{ChoiceValue, Value};
        use libspa::utils::{Choice, ChoiceEnum, Id};
        let bytes = meta_param_pod().unwrap();
        let (_, value) =
            libspa::pod::deserialize::PodDeserializer::deserialize_any_from(&bytes).unwrap();
        let Value::Object(object) = value else {
            panic!("not an object")
        };
        assert_eq!(object.id, libspa::param::ParamType::Meta.as_raw());
        let find = |key: u32| {
            object
                .properties
                .iter()
                .find(|p| p.key == key)
                .unwrap()
                .value
                .clone()
        };
        assert_eq!(
            find(spa::SPA_PARAM_META_type),
            Value::Id(Id(spa::SPA_META_Cursor))
        );
        let Value::Choice(ChoiceValue::Int(Choice(_, ChoiceEnum::Range { default, min, max }))) =
            find(spa::SPA_PARAM_META_size)
        else {
            panic!("the size is not a range")
        };
        let room = |side: i32| 28 + 20 + side * side * 4;
        assert_eq!(default, room(64));
        assert_eq!(min, room(1));
        assert_eq!(max, room(256));
    }
}
