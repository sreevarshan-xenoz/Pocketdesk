//! A glance at the computer's screen with no session (BF2): the phone asks
//! for one still, and the computer sends it as a JPEG through the transfer
//! path, as a clipboard image is fetched. Under the Peek permission, off
//! for every phone until its owner turns it on, and the computer's own
//! "Peek at this screen" setting.

/// Phone → computer: `{key}` asked, the still sent to the transfer key; or
/// `{key, refused: true, reason}` answered on the same connection.
pub const PEEK: &str = "kdeconnect.pocketdesk.peek";

/// The most bytes a still may be.
pub const MAX_PEEK: u64 = 8 * 1024 * 1024;

/// A still's longest side, at most: enough to read a window's title or a
/// progress bar on a phone, a fraction of a 4K screen's bytes.
pub const LONGEST_SIDE: u32 = 1600;

/// The size a `width` x `height` screen is sent at: shrunk, its shape kept,
/// so its longest side is at most `longest`; never grown, never 0.
pub fn fit(width: u32, height: u32, longest: u32) -> (u32, u32) {
    let side = width.max(height);
    if side <= longest || side == 0 {
        return (width.max(1), height.max(1));
    }
    let scale = |n: u32| {
        ((u64::from(n) * u64::from(longest) + u64::from(side) / 2) / u64::from(side)).max(1) as u32
    };
    (scale(width), scale(height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_large_screen_is_shrunk_to_its_longest_side_and_keeps_its_shape() {
        assert_eq!(fit(3840, 2160, 1600), (1600, 900));
        assert_eq!(fit(1080, 2400, 1600), (720, 1600));
        assert_eq!(fit(5120, 1440, 1600), (1600, 450));
    }

    #[test]
    fn a_small_screen_is_never_grown_and_nothing_is_zero() {
        assert_eq!(fit(1280, 720, 1600), (1280, 720));
        assert_eq!(fit(1600, 1600, 1600), (1600, 1600));
        assert_eq!(fit(0, 0, 1600), (1, 1));
        assert_eq!(fit(100_000, 1, 1600), (1600, 1));
    }
}
