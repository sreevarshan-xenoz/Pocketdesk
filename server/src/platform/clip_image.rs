//! Clipboard images between PNG (what crosses the wire, roadmap N4) and the
//! device-independent bitmap Windows apps read and write (`CF_DIB`).
//!
//! Pure byte conversion, so it is tested on every OS. A DIB here is what
//! `CF_DIB` holds: a `BITMAPINFOHEADER` (or a V4/V5 header), the three
//! colour masks when the compression is `BI_BITFIELDS` and the header is the
//! 40-byte one, then the rows, each padded to 4 bytes, bottom-up when the
//! height is positive.

/// Images larger than this, either side, are refused (pixels, not bytes):
/// 16384 x 16384 is a 1 GiB RGBA buffer.
const MAX_SIDE: u32 = 16384;

/// The most pixels an image may have, checked before anything is decoded or
/// allocated: 40 megapixels (an 8K screenshot is 33). A few hundred bytes of
/// PNG can claim 16384 x 16384, which would be gigabytes of buffers.
const MAX_PIXELS: u64 = 40_000_000;

const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Whether `bytes` starts like a PNG file.
pub fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(&PNG_SIGNATURE)
}

/// An image as straight RGBA rows, top row first.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Rgba {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

fn le_u16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn le_u32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn le_i32(b: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn checked_side(side: u32) -> Result<u32, String> {
    if side == 0 || side > MAX_SIDE {
        return Err(format!("an image side of {side} pixels is not supported"));
    }
    Ok(side)
}

/// `width` x `height`, each side checked, refused over [`MAX_PIXELS`].
fn checked_size(width: u32, height: u32) -> Result<(u32, u32), String> {
    let (width, height) = (checked_side(width)?, checked_side(height)?);
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(format!("a {width} x {height} image is too large"));
    }
    Ok((width, height))
}

/// Read a 24- or 32-bit uncompressed DIB.
fn decode_dib(dib: &[u8]) -> Result<Rgba, String> {
    let short = || "the bitmap is cut short".to_string();
    let header = le_u32(dib, 0).ok_or_else(short)? as usize;
    if !matches!(header, 40 | 108 | 124) {
        return Err(format!("a {header}-byte bitmap header is not supported"));
    }
    let raw_width = le_i32(dib, 4).ok_or_else(short)?;
    let raw_height = le_i32(dib, 8).ok_or_else(short)?;
    let bits = le_u16(dib, 14).ok_or_else(short)?;
    let compression = le_u32(dib, 16).ok_or_else(short)?;
    if !matches!(bits, 24 | 32) {
        return Err(format!("{bits}-bit bitmaps are not supported"));
    }
    // Masks: inside a V4/V5 header, after a 40-byte one.
    let (masks_at, mut offset) = if header == 40 {
        (40, 40 + if compression == BI_BITFIELDS { 12 } else { 0 })
    } else {
        (40, header)
    };
    match compression {
        BI_RGB => {}
        BI_BITFIELDS if bits == 32 => {
            let masks = (
                le_u32(dib, masks_at).ok_or_else(short)?,
                le_u32(dib, masks_at + 4).ok_or_else(short)?,
                le_u32(dib, masks_at + 8).ok_or_else(short)?,
            );
            if masks != (0x00FF_0000, 0x0000_FF00, 0x0000_00FF) {
                return Err("only BGRA bitmaps are supported".to_string());
            }
        }
        other => return Err(format!("bitmap compression {other} is not supported")),
    }
    // A colour table is only meaningful at 8 bits or fewer; skip one if a
    // writer left it anyway.
    if header == 40 {
        offset += le_u32(dib, 32).ok_or_else(short)? as usize * 4;
    }
    // The width is never negative; only the height says which way up.
    let width = u32::try_from(raw_width).map_err(|_| format!("a width of {raw_width}"))?;
    let (width, height) = checked_size(width, raw_height.unsigned_abs())?;
    let bottom_up = raw_height > 0;
    let step = usize::from(bits / 8);
    let stride = (width as usize * usize::from(bits)).div_ceil(32) * 4;
    let needed = offset + stride * height as usize;
    if dib.len() < needed {
        return Err(short());
    }
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    let mut alpha_seen = false;
    for row in 0..height as usize {
        let source = if bottom_up {
            height as usize - 1 - row
        } else {
            row
        };
        let line = &dib[offset + source * stride..][..width as usize * step];
        for px in line.chunks(step) {
            let alpha = if step == 4 { px[3] } else { 255 };
            alpha_seen |= alpha != 0;
            pixels.extend_from_slice(&[px[2], px[1], px[0], alpha]);
        }
    }
    // 32-bit BI_RGB leaves the fourth byte undefined, and most writers put
    // zero there: an image with no alpha anywhere is opaque, not invisible.
    if bits == 32 && !alpha_seen {
        for px in pixels.chunks_mut(4) {
            px[3] = 255;
        }
    }
    Ok(Rgba {
        width,
        height,
        pixels,
    })
}

/// Write a bottom-up 32-bit `BI_RGB` DIB with the alpha in the fourth byte.
fn encode_dib(image: &Rgba) -> Vec<u8> {
    let (w, h) = (image.width as usize, image.height as usize);
    let mut dib = Vec::with_capacity(40 + w * h * 4);
    dib.extend_from_slice(&40u32.to_le_bytes());
    dib.extend_from_slice(&(image.width as i32).to_le_bytes());
    dib.extend_from_slice(&(image.height as i32).to_le_bytes());
    dib.extend_from_slice(&1u16.to_le_bytes());
    dib.extend_from_slice(&32u16.to_le_bytes());
    dib.extend_from_slice(&BI_RGB.to_le_bytes());
    dib.extend_from_slice(&((w * h * 4) as u32).to_le_bytes());
    // 96 DPI, no colour table.
    dib.extend_from_slice(&3780i32.to_le_bytes());
    dib.extend_from_slice(&3780i32.to_le_bytes());
    dib.extend_from_slice(&0u32.to_le_bytes());
    dib.extend_from_slice(&0u32.to_le_bytes());
    for row in (0..h).rev() {
        for px in image.pixels[row * w * 4..][..w * 4].chunks(4) {
            dib.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    dib
}

fn decode_png(png_bytes: &[u8]) -> Result<Rgba, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| format!("not a PNG: {e}"))?;
    let (width, height) = {
        let info = reader.info();
        checked_size(info.width, info.height)?
    };
    let mut buffer = vec![0; reader.output_buffer_size().ok_or("the PNG is too large")?];
    let frame = reader
        .next_frame(&mut buffer)
        .map_err(|e| format!("the PNG does not decode: {e}"))?;
    buffer.truncate(frame.buffer_size());
    let data = &buffer;
    let pixels = match frame.color_type {
        png::ColorType::Rgba => buffer,
        png::ColorType::Rgb => data
            .chunks(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => data
            .chunks(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => data.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("an indexed PNG was not expanded".to_string()),
    };
    Ok(Rgba {
        width,
        height,
        pixels,
    })
}

fn encode_png(image: &Rgba) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer
        .write_image_data(&image.pixels)
        .map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(out)
}

/// A `CF_DIB` clipboard bitmap as PNG.
pub fn png_from_dib(dib: &[u8]) -> Result<Vec<u8>, String> {
    encode_png(&decode_dib(dib)?)
}

/// A PNG as a `CF_DIB` clipboard bitmap.
pub fn dib_from_png(png_bytes: &[u8]) -> Result<Vec<u8>, String> {
    Ok(encode_dib(&decode_png(png_bytes)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 3x2 test picture: red, green, blue on top; white, black, half-clear
    /// grey below.
    fn picture() -> Rgba {
        Rgba {
            width: 3,
            height: 2,
            pixels: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, //
                255, 255, 255, 255, 0, 0, 0, 255, 128, 128, 128, 128,
            ],
        }
    }

    fn header(width: i32, height: i32, bits: u16, compression: u32) -> Vec<u8> {
        let mut h = Vec::new();
        h.extend_from_slice(&40u32.to_le_bytes());
        h.extend_from_slice(&width.to_le_bytes());
        h.extend_from_slice(&height.to_le_bytes());
        h.extend_from_slice(&1u16.to_le_bytes());
        h.extend_from_slice(&bits.to_le_bytes());
        h.extend_from_slice(&compression.to_le_bytes());
        h.extend_from_slice(&[0; 20]);
        h
    }

    #[test]
    fn a_png_survives_the_trip_through_a_dib() {
        let png_bytes = encode_png(&picture()).unwrap();
        assert!(is_png(&png_bytes));
        let dib = dib_from_png(&png_bytes).unwrap();
        assert_eq!(decode_dib(&dib).unwrap(), picture());
        assert_eq!(decode_png(&png_from_dib(&dib).unwrap()).unwrap(), picture());
    }

    #[test]
    fn a_bottom_up_24_bit_dib_reads_top_row_first() {
        // Rows of 3 pixels x 3 bytes = 9, padded to 12; bottom row first.
        let mut dib = header(3, 2, 24, BI_RGB);
        dib.extend_from_slice(&[255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        dib.extend_from_slice(&[0, 0, 255, 0, 255, 0, 255, 0, 0, 0, 0, 0]);
        let image = decode_dib(&dib).unwrap();
        assert_eq!(&image.pixels[..12], &picture().pixels[..12], "top row");
        assert_eq!(&image.pixels[12..20], &picture().pixels[12..20]);
        assert_eq!(image.pixels[23], 255, "24-bit is opaque");
    }

    #[test]
    fn a_top_down_32_bit_dib_with_no_alpha_is_opaque() {
        let mut dib = header(1, -2, 32, BI_RGB);
        dib.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]);
        let image = decode_dib(&dib).unwrap();
        assert_eq!(image.pixels, vec![255, 0, 0, 255, 0, 0, 255, 255]);
    }

    #[test]
    fn a_bitfields_dib_with_bgra_masks_reads() {
        let mut dib = header(1, 1, 32, BI_BITFIELDS);
        for mask in [0x00FF_0000u32, 0x0000_FF00, 0x0000_00FF] {
            dib.extend_from_slice(&mask.to_le_bytes());
        }
        dib.extend_from_slice(&[1, 2, 3, 4]);
        assert_eq!(decode_dib(&dib).unwrap().pixels, vec![3, 2, 1, 4]);
        let mut other = header(1, 1, 32, BI_BITFIELDS);
        for mask in [0x0000_00FFu32, 0x0000_FF00, 0x00FF_0000] {
            other.extend_from_slice(&mask.to_le_bytes());
        }
        other.extend_from_slice(&[1, 2, 3, 4]);
        assert!(decode_dib(&other).is_err(), "RGBA masks are not guessed");
    }

    #[test]
    fn an_image_claiming_too_many_pixels_is_refused_before_decoding() {
        assert_eq!(checked_size(7680, 4320), Ok((7680, 4320)), "8K");
        assert!(checked_size(16384, 16384).is_err(), "a decode bomb");
        assert!(checked_size(16384, 2441).is_ok());
        assert!(checked_size(16384, 2442).is_err());
        // A PNG whose header claims 16384 x 16384 with one row of data: the
        // header is all that is read before refusing it.
        let mut bomb = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bomb, 16384, 16384);
            encoder.set_color(png::ColorType::Grayscale);
            let mut writer = encoder.write_header().unwrap();
            let mut rows = writer.stream_writer().unwrap();
            std::io::Write::write_all(&mut rows, &[0; 16384]).unwrap();
            std::io::Write::flush(&mut rows).unwrap();
            // Dropped unfinished: the rest of the image never comes.
        }
        assert!(decode_png(&bomb).unwrap_err().contains("too large"));
        // A bitmap header claims the same.
        assert!(decode_dib(&header(16384, 16384, 24, BI_RGB)).is_err());
    }

    #[test]
    fn malformed_bitmaps_and_pngs_are_refused() {
        assert!(decode_dib(&[]).is_err());
        assert!(decode_dib(&header(3, 2, 24, BI_RGB)).is_err(), "no rows");
        assert!(decode_dib(&header(0, 2, 24, BI_RGB)).is_err(), "no width");
        assert!(
            decode_dib(&header(-3, 2, 24, BI_RGB)).is_err(),
            "negative width"
        );
        assert!(
            decode_dib(&header(70000, 1, 24, BI_RGB)).is_err(),
            "too wide"
        );
        assert!(decode_dib(&header(1, 1, 8, BI_RGB)).is_err(), "paletted");
        assert!(decode_dib(&header(1, 1, 24, 1)).is_err(), "RLE");
        assert!(dib_from_png(b"\x89PNG\r\n\x1a\nnot really").is_err());
        assert!(!is_png(b"GIF89a"));
    }
}
