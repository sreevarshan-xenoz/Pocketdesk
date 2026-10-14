//! The app icon: the Pocketdesk mark (`assets/brand/pocketdesk-mark.svg`),
//! rendered ahead of time to raw RGBA at the two sizes the app uses, so it
//! needs no image decoder. Used for the window and, where there is one, the
//! tray.

const ICON_32: &[u8] = include_bytes!("icon-32.rgba");
const ICON_64: &[u8] = include_bytes!("icon-64.rgba");

/// The icon as `size` x `size` RGBA: 32 for the tray, 64 for the window.
/// Any other size gets the nearer of the two, scaled by nearest pixel.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let (source, from) = if size <= 32 {
        (ICON_32, 32)
    } else {
        (ICON_64, 64)
    };
    if size == from {
        return source.to_vec();
    }
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (sx, sy) = (x * from / size, y * from / size);
            let at = ((sy * from + sx) * 4) as usize;
            rgba.extend_from_slice(&source[at..at + 4]);
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_is_square_rgba_with_transparent_corners() {
        for size in [16, 32, 48, 64] {
            let icon = icon_rgba(size);
            assert_eq!(icon.len(), (size * size * 4) as usize);
            assert_eq!(icon[3], 0, "the corner is transparent");
            let centre = (((size / 2) * size + size / 2) * 4) as usize;
            assert_eq!(icon[centre + 3], 0xff, "the mark fills the middle");
        }
    }
}
