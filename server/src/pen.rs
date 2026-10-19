//! The phone's pen (E3), as each computer's input takes it: what one sample
//! means after the last (it came into range, touched, moved, lifted, left),
//! where it lands in pixels, and how hard it presses on Windows' scale.
//! Pure, so every platform's tests hold it; the devices are in
//! `input_pen_windows` and the uinput tablet in `input_injector`.

use pocketdesk_core::streaming::PenSample;

/// Where the pen is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PenPhase {
    /// Out of range: no pen.
    #[default]
    Away,
    /// Above the screen.
    Hovering,
    /// On it.
    Touching,
}

impl PenPhase {
    pub fn of(sample: &PenSample) -> Self {
        if sample.in_contact() {
            PenPhase::Touching
        } else if sample.in_range() {
            PenPhase::Hovering
        } else {
            PenPhase::Away
        }
    }
}

/// One thing a pen does, as a pointer device says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PenStep {
    /// It came into range.
    Enter,
    /// It moved while hovering.
    Hover,
    /// It touched.
    Down,
    /// It moved while touching.
    Move,
    /// It lifted, still in range.
    Up,
    /// It left range.
    Leave,
}

/// What going from `before` to where `sample` is takes. A pen never goes
/// down from out of range or away while touching without the step between:
/// Windows refuses a pointer that does.
pub fn steps(before: PenPhase, sample: &PenSample) -> Vec<PenStep> {
    use PenPhase::*;
    use PenStep::*;
    match (before, PenPhase::of(sample)) {
        (Away, Away) => vec![],
        (Away, Hovering) => vec![Enter],
        (Away, Touching) => vec![Enter, Down],
        (Hovering, Away) => vec![Leave],
        (Hovering, Hovering) => vec![Hover],
        (Hovering, Touching) => vec![Down],
        (Touching, Away) => vec![Up, Leave],
        (Touching, Hovering) => vec![Up],
        (Touching, Touching) => vec![Move],
    }
}

/// The virtual screen: the rectangle all monitors make, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Screen {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

/// Where a sample's 0..=65535 position lands in `screen`'s pixels: the
/// same place a mouse moved over the virtual desktop lands.
pub fn to_pixels(x_norm: u16, y_norm: u16, screen: Screen) -> (i32, i32) {
    let along = |norm: u16, start: i32, size: i32| {
        let span = i64::from((size - 1).max(0));
        start + ((i64::from(norm) * span + 32_767) / 65_535) as i32
    };
    (
        along(x_norm, screen.left, screen.width),
        along(y_norm, screen.top, screen.height),
    )
}

/// The highest pressure Windows' pen takes.
pub const WINDOWS_MAX_PRESSURE: u32 = 1024;

/// A sample's pressure on Windows' 0..=1024; a pen touching presses at
/// least 1, as Windows reads 0 as no pressure reported.
pub fn windows_pressure(sample: &PenSample) -> u32 {
    if !sample.in_contact() {
        return 0;
    }
    let scaled = (u32::from(sample.pressure) * WINDOWS_MAX_PRESSURE + 32_767) / 65_535;
    scaled.max(1)
}

/// The pen left for good (control moved, the session ended): a sample out
/// of range where it last was.
pub fn lifted(last: &PenSample) -> PenSample {
    PenSample {
        pressure: 0,
        flags: 0,
        ..*last
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::streaming::{PEN_CONTACT, PEN_IN_RANGE};

    fn sample(flags: u8) -> PenSample {
        PenSample {
            flags,
            ..PenSample::default()
        }
    }

    #[test]
    fn a_stroke_from_out_of_range_comes_into_range_before_it_touches() {
        let touching = sample(PEN_CONTACT | PEN_IN_RANGE);
        assert_eq!(
            steps(PenPhase::Away, &touching),
            [PenStep::Enter, PenStep::Down]
        );
        assert_eq!(steps(PenPhase::Touching, &touching), [PenStep::Move]);
        assert_eq!(
            steps(PenPhase::Touching, &sample(0)),
            [PenStep::Up, PenStep::Leave],
            "a finger lifting is a pen lifting and leaving"
        );
        assert_eq!(steps(PenPhase::Away, &sample(0)), []);
    }

    #[test]
    fn a_stylus_hovers_touches_and_lifts_back_to_hovering() {
        let hover = sample(PEN_IN_RANGE);
        assert_eq!(steps(PenPhase::Away, &hover), [PenStep::Enter]);
        assert_eq!(steps(PenPhase::Hovering, &hover), [PenStep::Hover]);
        assert_eq!(
            steps(PenPhase::Hovering, &sample(PEN_CONTACT)),
            [PenStep::Down],
            "contact alone counts as in range"
        );
        assert_eq!(steps(PenPhase::Touching, &hover), [PenStep::Up]);
        assert_eq!(steps(PenPhase::Hovering, &sample(0)), [PenStep::Leave]);
    }

    #[test]
    fn a_position_lands_on_the_virtual_screen_s_pixels() {
        // Two monitors, the second left of the first.
        let screen = Screen {
            left: -1920,
            top: 0,
            width: 3840,
            height: 1080,
        };
        assert_eq!(to_pixels(0, 0, screen), (-1920, 0));
        assert_eq!(to_pixels(65_535, 65_535, screen), (1919, 1079));
        assert_eq!(to_pixels(32_768, 32_768, screen), (0, 540));
    }

    #[test]
    fn pressure_is_windows_scale_and_a_touch_presses_at_least_one() {
        let mut pen = sample(PEN_CONTACT);
        pen.pressure = 65_535;
        assert_eq!(windows_pressure(&pen), WINDOWS_MAX_PRESSURE);
        pen.pressure = 32_768;
        assert_eq!(windows_pressure(&pen), 512);
        pen.pressure = 0;
        assert_eq!(windows_pressure(&pen), 1);
        pen.flags = PEN_IN_RANGE;
        pen.pressure = 65_535;
        assert_eq!(windows_pressure(&pen), 0, "a hovering pen presses nothing");
    }

    #[test]
    fn a_pen_lifted_for_good_stays_where_it_was() {
        let last = PenSample {
            x_norm: 7,
            y_norm: 9,
            pressure: 100,
            tilt_x: 5,
            tilt_y: -5,
            flags: PEN_CONTACT | PEN_IN_RANGE,
        };
        let gone = lifted(&last);
        assert_eq!((gone.x_norm, gone.y_norm), (7, 9));
        assert_eq!(PenPhase::of(&gone), PenPhase::Away);
    }
}
