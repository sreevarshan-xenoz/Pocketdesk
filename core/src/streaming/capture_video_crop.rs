//! The part of a PipeWire frame that is the picture (`SPA_META_VideoCrop`),
//! for a lent window.
//!
//! Mutter's stream of one window (`RecordWindow`) is negotiated at the size of
//! the screen it could fill, and each buffer is that big, with the window
//! drawn in a corner of it and the rest empty (measured on GNOME Shell 51: a
//! 640x480 window came as 1280x720 frames with the window at the top left).
//! What is the picture is said in a record on each buffer, a `spa_meta_region`
//! (`SPA_META_VideoCrop`) that the stream sends only to a consumer that asks
//! for it. A lent window asks, reads the record of each buffer, and cuts the
//! frame to it, so the viewer shows the window and nothing else.
//!
//! The record is another process's: each field is read with its bounds
//! checked, and a rectangle that is not inside the frame is not used (the
//! frame is then sent whole, as before this).

use anyhow::Result;
use libspa::sys as spa;

/// `sizeof(struct spa_meta_region)`: a position (two `int32`) and a size (two
/// `uint32`).
const REGION_SIZE: usize = 16;

/// A rectangle of a frame, in its pixels: left, top, width, height.
pub type Region = (u32, u32, u32, u32);

fn i32_at(data: &[u8], at: usize) -> Option<i32> {
    data.get(at..at + 4)
        .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// The rectangle a `spa_meta_region` record holds: `None` when it is too
/// short, or its size is empty or not a size (a record of zeroes is the
/// stream saying nothing).
pub fn parse(data: &[u8]) -> Option<(i32, i32, i32, i32)> {
    if data.len() < REGION_SIZE {
        return None;
    }
    let (x, y) = (i32_at(data, 0)?, i32_at(data, 4)?);
    let (w, h) = (i32_at(data, 8)?, i32_at(data, 12)?);
    (w > 0 && h > 0).then_some((x, y, w, h))
}

/// The part of a `frame_width`×`frame_height` frame a record names, when it
/// is a part of it: inside the frame on every side, and not the whole frame
/// (which needs no cutting). `None` otherwise, and the frame is used whole.
pub fn region_in(
    frame_width: u32,
    frame_height: u32,
    record: (i32, i32, i32, i32),
) -> Option<Region> {
    let (x, y, w, h) = record;
    let (x, y) = (u32::try_from(x).ok()?, u32::try_from(y).ok()?);
    let (w, h) = (u32::try_from(w).ok()?, u32::try_from(h).ok()?);
    if w == 0 || h == 0 || x.checked_add(w)? > frame_width || y.checked_add(h)? > frame_height {
        return None;
    }
    ((x, y, w, h) != (0, 0, frame_width, frame_height)).then_some((x, y, w, h))
}

/// What to do with a frame of a lent window's stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// The frame is the window (its record names the whole frame).
    Whole,
    /// Cut the frame to this rectangle.
    Cut(Region),
    /// Send nothing: the frame cannot be shown to be only the window.
    Drop,
}

/// The part of a `frame_width` by `frame_height` frame of a lent window to
/// send, by the rule that when in doubt nothing is sent: a stream that is as
/// big as a screen with the window in a corner is only the window once it is
/// cut to the window. A record inside the frame cuts it; one naming the whole
/// frame leaves it; one that is not inside the frame drops it; a buffer with
/// no record (`None`) is cut to the last rectangle that was good (`last`) if
/// that still fits, and dropped if there is none or it no longer fits.
pub fn part(
    frame_width: u32,
    frame_height: u32,
    record: Option<(i32, i32, i32, i32)>,
    last: Option<Region>,
) -> Part {
    let whole = (
        i32::try_from(frame_width).ok(),
        i32::try_from(frame_height).ok(),
    );
    let fits = |record: (i32, i32, i32, i32)| {
        if Some(record.2) == whole.0 && Some(record.3) == whole.1 && (record.0, record.1) == (0, 0)
        {
            return Some(Part::Whole);
        }
        region_in(frame_width, frame_height, record).map(Part::Cut)
    };
    match record {
        Some(record) => fits(record).unwrap_or(Part::Drop),
        None => last
            .and_then(|(x, y, w, h)| {
                Some((
                    i32::try_from(x).ok()?,
                    i32::try_from(y).ok()?,
                    i32::try_from(w).ok()?,
                    i32::try_from(h).ok()?,
                ))
            })
            .and_then(|r| match fits(r) {
                Some(Part::Cut(c)) => Some(Part::Cut(c)),
                _ => None,
            })
            .unwrap_or(Part::Drop),
    }
}

/// The `Meta` parameter that asks the stream for the picture's rectangle:
/// `SPA_META_VideoCrop`, one `spa_meta_region`.
pub fn meta_param_pod() -> Result<Vec<u8>> {
    use libspa::pod::{Object, Property, Value};
    use libspa::utils::{Id, SpaTypes};
    let object = Object {
        type_: SpaTypes::ObjectParamMeta.as_raw(),
        id: libspa::param::ParamType::Meta.as_raw(),
        properties: vec![
            Property::new(
                spa::SPA_PARAM_META_type,
                Value::Id(Id(spa::SPA_META_VideoCrop)),
            ),
            Property::new(spa::SPA_PARAM_META_size, Value::Int(REGION_SIZE as i32)),
        ],
    };
    let (cursor, _) = libspa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &Value::Object(object),
    )
    .map_err(|e| anyhow::anyhow!("The crop's meta param did not serialize: {e:?}"))?;
    Ok(cursor.into_inner())
}

/// The picture's record in the buffer `buffer`, as bytes: the meta of type
/// video crop, when it has one.
///
/// # Safety
/// `buffer` is null or a valid `spa_buffer` that stays valid and unmodified
/// for the returned slice's life.
pub unsafe fn crop_meta<'a>(buffer: *const spa::spa_buffer) -> Option<&'a [u8]> {
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
            if meta.type_ == spa::SPA_META_VideoCrop
                && !meta.data.is_null()
                && meta.size as usize >= REGION_SIZE
            {
                return Some(std::slice::from_raw_parts(
                    meta.data as *const u8,
                    meta.size as usize,
                ));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(x: i32, y: i32, w: i32, h: i32) -> Vec<u8> {
        [x, y, w, h].iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    #[test]
    fn a_record_is_read_field_by_field_and_a_short_or_empty_one_says_nothing() {
        assert_eq!(parse(&record(10, 20, 640, 480)), Some((10, 20, 640, 480)));
        assert_eq!(parse(&record(-5, -6, 1, 1)), Some((-5, -6, 1, 1)));
        assert_eq!(parse(&record(0, 0, 0, 0)), None, "zeroes say nothing");
        assert_eq!(parse(&record(0, 0, 640, 0)), None);
        assert_eq!(parse(&record(0, 0, -640, 480)), None);
        assert_eq!(parse(&record(0, 0, 640, 480)[..15]), None, "too short");
        assert_eq!(parse(&[]), None);
        // More than the record is fine: only its first sixteen bytes count.
        let mut longer = record(1, 2, 3, 4);
        longer.extend([0xff; 8]);
        assert_eq!(parse(&longer), Some((1, 2, 3, 4)));
    }

    #[test]
    fn only_a_part_inside_the_frame_is_cut_out_of_it() {
        let frame = (1280, 720);
        assert_eq!(
            region_in(frame.0, frame.1, (0, 0, 640, 480)),
            Some((0, 0, 640, 480)),
            "the measured case: a window in the corner"
        );
        assert_eq!(
            region_in(frame.0, frame.1, (100, 50, 300, 200)),
            Some((100, 50, 300, 200))
        );
        assert_eq!(
            region_in(frame.0, frame.1, (640, 240, 640, 480)),
            Some((640, 240, 640, 480)),
            "touching the far edges is inside"
        );
        // The whole frame needs no cutting.
        assert_eq!(region_in(frame.0, frame.1, (0, 0, 1280, 720)), None);
        // Anything that is not inside is not used: the frame goes whole.
        for outside in [
            (-1, 0, 640, 480),
            (0, -1, 640, 480),
            (641, 0, 640, 480),
            (0, 241, 640, 480),
            (0, 0, 1281, 720),
            (0, 0, 1280, 721),
            (0, 0, 0, 480),
            (0, 0, 640, 0),
            (i32::MAX, 0, 1, 1),
            (1, 1, i32::MAX, i32::MAX),
        ] {
            assert_eq!(region_in(frame.0, frame.1, outside), None, "{outside:?}");
        }
    }

    #[test]
    fn a_frame_of_a_lent_window_is_sent_only_when_it_is_shown_to_be_the_window() {
        let (w, h) = (1280, 720);
        // A record inside the frame cuts it; the whole frame is the window.
        assert_eq!(
            part(w, h, Some((0, 0, 640, 480)), None),
            Part::Cut((0, 0, 640, 480))
        );
        assert_eq!(part(w, h, Some((0, 0, 1280, 720)), None), Part::Whole);
        // A record outside, or empty: nothing is sent.
        assert_eq!(part(w, h, Some((700, 0, 640, 480)), None), Part::Drop);
        assert_eq!(
            part(w, h, Some((0, 0, 0, 0)), Some((0, 0, 640, 480))),
            Part::Drop
        );
        // No record: the last good rectangle while it fits, else nothing.
        assert_eq!(part(w, h, None, None), Part::Drop);
        assert_eq!(
            part(w, h, None, Some((0, 0, 640, 480))),
            Part::Cut((0, 0, 640, 480))
        );
        assert_eq!(part(w, h, None, Some((0, 0, 1280, 720))), Part::Drop);
        assert_eq!(
            part(640, 480, None, Some((0, 0, 800, 600))),
            Part::Drop,
            "no longer fits"
        );
    }

    #[test]
    fn the_stream_is_asked_for_one_crop_region() {
        use libspa::pod::Value;
        use libspa::utils::Id;
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
            Value::Id(Id(spa::SPA_META_VideoCrop))
        );
        assert_eq!(find(spa::SPA_PARAM_META_size), Value::Int(16));
    }
}
