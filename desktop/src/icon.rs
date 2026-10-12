//! The app icon, drawn rather than shipped: a monitor on the brand pine
//! plate, readable at 16 px. Used for the window and, where there is one,
//! the tray.

pub fn icon_rgba(size: u32) -> Vec<u8> {
    let pine = [0x1f, 0x4e, 0x4a, 0xff];
    let screen = [0xf2, 0xf5, 0xf4, 0xff];
    let amber = [0xf2, 0xa9, 0x3b, 0xff];
    let s = size as f32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 / s, y as f32 / s);
            // Rounded plate: drop the four corner pixels' worth of each corner.
            let corner = (fx < 0.12 || fx > 0.88) && (fy < 0.12 || fy > 0.88);
            let in_screen = (0.18..0.82).contains(&fx) && (0.22..0.62).contains(&fy);
            let in_stand = (0.44..0.56).contains(&fx) && (0.62..0.74).contains(&fy);
            let in_base = (0.32..0.68).contains(&fx) && (0.74..0.80).contains(&fy);
            let pixel = if corner {
                [0, 0, 0, 0]
            } else if in_screen && (0.62..0.74).contains(&fx) && (0.46..0.56).contains(&fy) {
                amber
            } else if in_screen || in_stand || in_base {
                screen
            } else {
                pine
            };
            rgba.extend_from_slice(&pixel);
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_is_square_rgba_with_transparent_corners() {
        let icon = icon_rgba(32);
        assert_eq!(icon.len(), 32 * 32 * 4);
        assert_eq!(icon[3], 0, "the corner is transparent");
        let centre = ((16 * 32 + 16) * 4) as usize;
        assert_eq!(icon[centre + 3], 0xff);
    }
}
