//! The phone's camera as this computer's camera (F6 part 2): the computer
//! asks a phone with its Webcam permission to start a camera cast, naming
//! the lens, the picture's height and its frame rate. The phone, with "Let
//! the computer use my camera" on, offers its owner the camera in a
//! notification (Android lets no app in the background open it), and the
//! cast that follows says it is for the camera
//! ([`crate::streaming::client::CAST_WEBCAM_FLAG`]). Both ends hold the
//! fields to the rules here.

use serde_json::{Value, json};

/// Computer → phone: `{lens, height, fps}`, be this computer's camera.
pub const START: &str = "kdeconnect.pocketdesk.phone_camera.start";

/// The heights a camera cast may ask for: 720p or 1080p.
pub const HEIGHTS: [u16; 2] = [720, 1080];

/// The frame rates a camera cast may ask for.
pub const FRAME_RATES: [u8; 3] = [15, 24, 30];

/// Which of the phone's cameras.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lens {
    #[default]
    Back,
    Front,
}

impl Lens {
    /// Its name on the wire and in the computer's settings.
    pub fn name(self) -> &'static str {
        match self {
            Lens::Back => "back",
            Lens::Front => "front",
        }
    }

    /// The lens `name` names, in any case; the back one for anything else.
    pub fn from_name(name: &str) -> Self {
        if name.trim().eq_ignore_ascii_case("front") {
            Lens::Front
        } else {
            Lens::Back
        }
    }
}

/// What the computer asks the phone's camera for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CameraAsk {
    pub lens: Lens,
    /// The picture's height: one of [`HEIGHTS`].
    pub height: u16,
    /// Frames a second: one of [`FRAME_RATES`].
    pub fps: u8,
}

impl Default for CameraAsk {
    fn default() -> Self {
        Self {
            lens: Lens::Back,
            height: 720,
            fps: 30,
        }
    }
}

/// The one of `allowed` nearest `wanted`, the lower on a tie.
fn nearest<T: Copy + Into<i64>>(allowed: &[T], wanted: i64) -> T {
    let mut best = allowed[0];
    for &value in allowed {
        if (value.into() - wanted).abs() < (best.into() - wanted).abs() {
            best = value;
        }
    }
    best
}

impl CameraAsk {
    /// An ask from the computer's settings, each held to what may be
    /// asked: an unknown lens is the back one, a height or rate the
    /// nearest allowed.
    pub fn new(lens: &str, height: u32, fps: u32) -> Self {
        Self {
            lens: Lens::from_name(lens),
            height: nearest(&HEIGHTS, i64::from(height)),
            fps: nearest(&FRAME_RATES, i64::from(fps)),
        }
    }

    /// The packet's body.
    pub fn body(&self) -> Value {
        json!({ "lens": self.lens.name(), "height": self.height, "fps": self.fps })
    }

    /// An ask as a computer sent it, held to the rules: fields missing or
    /// out of range take the default's or the nearest allowed.
    pub fn from_body(body: &Value) -> Self {
        let default = Self::default();
        let number = |key: &str, fallback: u32| {
            body.get(key)
                .and_then(Value::as_u64)
                .map_or(fallback, |n| u32::try_from(n).unwrap_or(u32::MAX))
        };
        Self::new(
            body.get("lens")
                .and_then(Value::as_str)
                .unwrap_or(default.lens.name()),
            number("height", u32::from(default.height)),
            number("fps", u32::from(default.fps)),
        )
    }

    /// The picture's width at its height, 16:9.
    pub fn width(&self) -> u16 {
        // 720 → 1280, 1080 → 1920: both whole.
        self.height / 9 * 16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ask_goes_and_comes_back_the_same() {
        for lens in ["back", "front"] {
            for height in HEIGHTS {
                for fps in FRAME_RATES {
                    let ask = CameraAsk::new(lens, u32::from(height), u32::from(fps));
                    assert_eq!(CameraAsk::from_body(&ask.body()), ask);
                }
            }
        }
        assert_eq!(
            CameraAsk::new("front", 1080, 24).body(),
            json!({"lens": "front", "height": 1080, "fps": 24})
        );
    }

    #[test]
    fn an_ask_is_held_to_what_may_be_asked() {
        let odd = CameraAsk::from_body(&json!({"lens": "side", "height": 4320, "fps": 240}));
        assert_eq!(odd, CameraAsk::new("back", 1080, 30));
        let low = CameraAsk::from_body(&json!({"height": 1, "fps": 1}));
        assert_eq!((low.height, low.fps), (720, 15));
        assert_eq!(CameraAsk::from_body(&json!({})), CameraAsk::default());
        assert_eq!(
            CameraAsk::from_body(&json!({"lens": "FRONT", "height": "1080", "fps": -5})),
            CameraAsk {
                lens: Lens::Front,
                ..CameraAsk::default()
            }
        );
        assert_eq!(
            CameraAsk::new("back", 900, 20).height,
            720,
            "a tie goes low"
        );
        assert_eq!(CameraAsk::new("back", 901, 28).fps, 30);
    }

    #[test]
    fn its_width_is_sixteen_by_nine() {
        assert_eq!(CameraAsk::new("back", 720, 30).width(), 1280);
        assert_eq!(CameraAsk::new("back", 1080, 30).width(), 1920);
    }
}
