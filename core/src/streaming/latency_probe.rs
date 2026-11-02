//! The marker of the input-to-picture latency probe (phase 0 of the window
//! handoff work). With `POCKETDESK_TEST_PATTERN=probe` the test pattern
//! (`super::test_pattern`) stands in for the screen, draws a bright square
//! where the last absolute pointer move of a session said, and nothing that
//! session sends is injected anywhere: a stand-in viewer sends moves, finds
//! the square in the pictures it decodes, and times it from the move to the
//! first picture that shows the square in its new place.
//!
//! Here, with no capture or encoder behind it so the viewer's tests can use
//! it too: the marker's place, the marker found in a luma plane, the pointer
//! the capture reads and the rule that keeps the packets out of the
//! injector.

use std::sync::atomic::{AtomicU64, Ordering};

use super::input_packet::InputPacket;

/// The marker's side, in pixels.
pub const SIZE: u32 = 32;
/// The reserved band's height, the picture's last rows. The marker moves
/// along it; the pattern's bar and counting strip never reach it, so the
/// marker cannot be taken for them.
pub const BAND: u32 = 48;
/// Luma the band is made at (limited range black), and the marker (white).
pub const DARK: u8 = 16;
/// The marker's luma; a decoded one is told from the band by anything
/// above [`FOUND_ABOVE`], halfway between the two.
pub const LIGHT: u8 = 235;
pub const FOUND_ABOVE: u8 = 125;

/// The band's first row in a picture `height` rows tall.
pub fn band_top(height: u32) -> u32 {
    height.saturating_sub(BAND)
}

/// The marker's first row: centred in the band.
pub fn marker_top(height: u32) -> u32 {
    band_top(height) + (BAND - SIZE) / 2
}

/// The marker's first column for a pointer at `x_norm` (0..=65535 over the
/// picture, as [`InputPacket::MouseMoveAbs`] says it) in a picture `width`
/// wide: the marker stays whole in the picture.
pub fn marker_left(width: u32, x_norm: u16) -> u32 {
    let span = u64::from(width.saturating_sub(SIZE));
    ((u64::from(x_norm) * span + 32767) / 65535) as u32
}

/// The first column of the marker in `luma` (a plane `width` by `height`,
/// one byte a pixel, rows without padding), or `None` when it is not in the
/// picture. A decoded marker is blurred a little at its edges, so it is the
/// run of bright pixels at least `SIZE - 8` long, found alike (within
/// 3 columns) in three rows through the marker, that counts.
pub fn find(luma: &[u8], width: usize, height: usize) -> Option<u32> {
    if width == 0 || height < BAND as usize || luma.len() < width * height {
        return None;
    }
    let top = marker_top(height as u32) as usize;
    let size = SIZE as usize;
    let rows = [top + 6, top + size / 2, top + size - 7];
    let mut first: Option<usize> = None;
    for row in rows {
        let line = &luma[row * width..(row + 1) * width];
        let start = bright_run(line, size - 8)?;
        match first {
            None => first = Some(start),
            Some(f) if start.abs_diff(f) <= 3 => {}
            Some(_) => return None,
        }
    }
    first.map(|x| x as u32)
}

/// Where the first run of at least `min` bright pixels in `line` starts.
fn bright_run(line: &[u8], min: usize) -> Option<usize> {
    let mut start = None;
    for (x, v) in line.iter().enumerate() {
        match (*v > FOUND_ABOVE, start) {
            (true, None) => start = Some(x),
            (false, Some(s)) => {
                if x - s >= min {
                    return Some(s);
                }
                start = None;
            }
            _ => {}
        }
    }
    start.filter(|s| line.len() - s >= min)
}

/// The test pattern's variable (`test_pattern::VAR` is this, where capture
/// is built).
pub const PATTERN_VAR: &str = "POCKETDESK_TEST_PATTERN";

/// The variable's value that asks for the probe (the pattern's own variable
/// at any other value but off is the pattern alone, as before).
pub const MODE: &str = "probe";

/// Whether `value` of `POCKETDESK_TEST_PATTERN` asks for the probe.
pub fn mode_by(value: Option<std::ffi::OsString>) -> bool {
    value.is_some_and(|v| v.to_string_lossy().trim().eq_ignore_ascii_case(MODE))
}

/// Whether this process is in probe mode: the pattern is the screen, the
/// marker is drawn, and no session's input is injected.
pub fn mode() -> bool {
    mode_by(std::env::var_os(PATTERN_VAR))
}

/// The last absolute pointer a session sent, shared between whoever
/// receives the input and the pattern's thread. Packed `x << 16 | y`, with
/// a bit above for "set", so every pointer value is one.
pub struct Pointer(AtomicU64);

const SET: u64 = 1 << 32;

impl Pointer {
    pub const fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    /// The last absolute move noted, as `(x_norm, y_norm)`.
    pub fn get(&self) -> Option<(u16, u16)> {
        let v = self.0.load(Ordering::Relaxed);
        (v & SET != 0).then_some(((v >> 16) as u16, v as u16))
    }

    fn set(&self, x_norm: u16, y_norm: u16) {
        self.0.store(
            SET | (u64::from(x_norm) << 16) | u64::from(y_norm),
            Ordering::Relaxed,
        );
    }

    /// Note `packet` when it is an absolute move.
    pub fn note(&self, packet: &InputPacket) {
        if let InputPacket::MouseMoveAbs { x_norm, y_norm } = packet {
            self.set(*x_norm, *y_norm);
        }
    }
}

impl Default for Pointer {
    fn default() -> Self {
        Self::new()
    }
}

/// The process's own pointer: what its sessions sent, and what its pattern
/// draws.
pub static POINTER: Pointer = Pointer::new();

/// The injection task's one question of each packet: with `probe` on, the
/// packet is kept from the injector (`true`: nothing of it goes to the
/// desktop, whatever it is) and its absolute move, if it is one, is noted
/// for the marker; otherwise `false` and nothing is touched.
pub fn divert(probe: bool, pointer: &Pointer, packet: &InputPacket) -> bool {
    if probe {
        pointer.note(packet);
    }
    probe
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::input_packet::PenSample;

    /// A luma plane of `width` x `height` at `DARK` with the marker drawn
    /// as the pattern does, left edge at `left`.
    fn plane(width: u32, height: u32, left: Option<u32>) -> Vec<u8> {
        let mut luma = vec![DARK; (width * height) as usize];
        if let Some(left) = left {
            for y in marker_top(height)..marker_top(height) + SIZE {
                for x in left..left + SIZE {
                    luma[(y * width + x) as usize] = LIGHT;
                }
            }
        }
        luma
    }

    #[test]
    fn the_marker_stays_whole_in_the_picture_and_in_its_band() {
        assert_eq!(marker_left(1280, 0), 0);
        assert_eq!(marker_left(1280, u16::MAX), 1280 - SIZE);
        assert_eq!(marker_left(1280, 32768), 624, "the middle, to the pixel");
        // A picture narrower than the marker clamps to its first column.
        assert_eq!(marker_left(16, u16::MAX), 0);
        assert_eq!(band_top(720), 672);
        assert_eq!(marker_top(720), 672 + 8);
        assert!(marker_top(720) + SIZE <= 720, "the marker's last row is in");
        // The smallest pattern (64 rows) still has rows above its band for
        // the counting strip (16).
        assert!(band_top(64) >= 16);
    }

    #[test]
    fn it_finds_the_marker_wherever_it_is() {
        for left in [0, 1, 100, 640, 1280 - SIZE] {
            let luma = plane(1280, 720, Some(left));
            assert_eq!(find(&luma, 1280, 720), Some(left), "left {left}");
        }
        assert_eq!(find(&plane(1280, 720, None), 1280, 720), None, "no marker");
        // A pattern's x_norm and the found column agree.
        let x_norm = 20000;
        let left = marker_left(1280, x_norm);
        assert_eq!(find(&plane(1280, 720, Some(left)), 1280, 720), Some(left));
    }

    #[test]
    fn it_ignores_what_is_not_the_marker() {
        // The pattern's bar is white and 64 wide, full height, but the band
        // is made dark over it: nothing above the band counts.
        let mut luma = plane(1280, 720, None);
        for y in 0..band_top(720) {
            for x in 300..364 {
                luma[(y * 1280 + x) as usize] = LIGHT;
            }
        }
        assert_eq!(find(&luma, 1280, 720), None);
        // A speck (one bright pixel) in the band is not a marker.
        let at = (marker_top(720) + 16) * 1280 + 500;
        luma[at as usize] = LIGHT;
        assert_eq!(find(&luma, 1280, 720), None);
        // Too short a run in all three rows: half a marker.
        let mut half = plane(1280, 720, None);
        for y in marker_top(720)..marker_top(720) + SIZE {
            for x in 500..500 + SIZE / 2 {
                half[(y * 1280 + x) as usize] = LIGHT;
            }
        }
        assert_eq!(find(&half, 1280, 720), None);
        // Rows that disagree (a bright block in only one of the three).
        let mut slanted = plane(1280, 720, None);
        for x in 100..100 + SIZE {
            slanted[((marker_top(720) + 16) * 1280 + x) as usize] = LIGHT;
        }
        assert_eq!(find(&slanted, 1280, 720), None);
    }

    #[test]
    fn it_finds_a_blurred_marker_and_takes_bad_input_calmly() {
        // Soft edges: two columns each side at half way, below the line.
        let mut luma = plane(640, 360, Some(200));
        for y in marker_top(360)..marker_top(360) + SIZE {
            for x in [198u32, 199, 232, 233] {
                luma[(y * 640 + x) as usize] = 70;
            }
        }
        assert_eq!(find(&luma, 640, 360), Some(200));
        assert_eq!(find(&[], 0, 0), None);
        assert_eq!(find(&[0; 10], 640, 360), None, "a plane too short");
        assert_eq!(find(&plane(64, 40, None), 64, 40), None, "too low a picture");
        // A marker at the right edge runs to the row's end.
        let edge = plane(640, 360, Some(640 - SIZE));
        assert_eq!(find(&edge, 640, 360), Some(640 - SIZE));
    }

    #[test]
    fn the_pointer_keeps_the_last_absolute_move_and_nothing_else() {
        let pointer = Pointer::new();
        assert_eq!(pointer.get(), None);
        pointer.note(&InputPacket::MouseMove { dx: 5, dy: 5 });
        assert_eq!(pointer.get(), None, "a relative move says no place");
        pointer.note(&InputPacket::MouseMoveAbs {
            x_norm: 1,
            y_norm: 2,
        });
        assert_eq!(pointer.get(), Some((1, 2)));
        pointer.note(&InputPacket::MouseClick {
            button: 0,
            pressed: true,
        });
        assert_eq!(pointer.get(), Some((1, 2)));
        pointer.note(&InputPacket::MouseMoveAbs {
            x_norm: u16::MAX,
            y_norm: 0,
        });
        assert_eq!(pointer.get(), Some((u16::MAX, 0)), "every value is a place");
        pointer.note(&InputPacket::MouseMoveAbs {
            x_norm: 0,
            y_norm: 0,
        });
        assert_eq!(pointer.get(), Some((0, 0)), "the corner is a place too");
    }

    /// One of every kind of packet there is.
    fn every_packet() -> Vec<InputPacket> {
        vec![
            InputPacket::MouseMove { dx: 1, dy: 1 },
            InputPacket::MouseMoveAbs {
                x_norm: 9,
                y_norm: 9,
            },
            InputPacket::MouseClick {
                button: 0,
                pressed: true,
            },
            InputPacket::MouseScroll {
                dx: 0,
                dy: 1,
                fine: Some((0, 120)),
            },
            InputPacket::KeyEvent {
                key: 30,
                pressed: true,
            },
            InputPacket::Text("x".into()),
            InputPacket::RequestKeyframe,
            InputPacket::WindowCrop {
                x: None,
                y: None,
                width: None,
                height: None,
                window: 0,
            },
            InputPacket::ViewOnly { enabled: false },
            InputPacket::FullQuality { enabled: true },
            InputPacket::Mic {
                enabled: true,
                opus: vec![1],
            },
            InputPacket::QualityPreset { preset: 0 },
            InputPacket::LinkFeedback {
                rtt: std::time::Duration::from_millis(1),
                lost_packets: 0,
            },
            InputPacket::ClientSamples {
                kind: 0,
                values: vec![1],
            },
            InputPacket::Pen(PenSample::default()),
            InputPacket::Laser {
                dx: 1,
                dy: 1,
                show: true,
            },
            InputPacket::Gamepad {
                axes: [0; 6],
                buttons: 1,
            },
        ]
    }

    #[test]
    fn in_probe_mode_no_packet_of_any_kind_reaches_the_injector() {
        let pointer = Pointer::new();
        for packet in every_packet() {
            assert!(
                divert(true, &pointer, &packet),
                "{packet:?} must be kept from the injector"
            );
        }
        // The last absolute move of the lot is the marker's place.
        assert_eq!(pointer.get(), Some((9, 9)));
    }

    #[test]
    fn out_of_probe_mode_every_packet_goes_on_and_nothing_is_noted() {
        let pointer = Pointer::new();
        for packet in every_packet() {
            assert!(!divert(false, &pointer, &packet), "{packet:?} goes on");
        }
        assert_eq!(pointer.get(), None);
    }

    #[test]
    fn probe_mode_is_one_value_of_the_patterns_variable() {
        assert!(mode_by(Some("probe".into())));
        assert!(mode_by(Some(" Probe ".into())));
        assert!(!mode_by(Some("1".into())), "the pattern alone, as before");
        assert!(!mode_by(Some("".into())));
        assert!(!mode_by(None));
    }
}
