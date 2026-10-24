//! The phone's gamepad as a game controller (T7): on Linux, with Settings
//! "The phone's gamepad as a game controller" on, its packets drive a
//! virtual pad laid out as an Xbox 360 controller's (`xpad`), which games
//! and SDL know, instead of keys and the mouse. Here the packet becomes the
//! pad's events, only those that changed; the device is the injector's.

/// `EV_KEY`.
pub const EV_KEY: u16 = 0x01;
/// `EV_ABS`.
pub const EV_ABS: u16 = 0x03;

/// The sticks, the triggers and the D-pad's hat, as `xpad` names them.
pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const ABS_Z: u16 = 0x02;
pub const ABS_RX: u16 = 0x03;
pub const ABS_RY: u16 = 0x04;
pub const ABS_RZ: u16 = 0x05;
pub const ABS_HAT0X: u16 = 0x10;
pub const ABS_HAT0Y: u16 = 0x11;

/// A trigger's range on the pad, as `xpad`'s.
pub const TRIGGER_MAX: i32 = 255;

/// The wire's button bits and the pad's buttons, as `xpad` reports an Xbox
/// pad's (T7 review: X is `BTN_X` 0x133 and Y `BTN_Y` 0x134, which SDL's
/// Xbox 360 mapping takes as x and y; by position the other way round).
/// The D-pad's bits are the hat's.
pub const BUTTONS: [(u16, u16); 11] = [
    (0x0001, 0x130), // A: BTN_SOUTH
    (0x0002, 0x131), // B: BTN_EAST
    (0x0004, 0x133), // X: BTN_X
    (0x0008, 0x134), // Y: BTN_Y
    (0x0010, 0x136), // LB: BTN_TL
    (0x0020, 0x137), // RB: BTN_TR
    (0x0040, 0x13a), // Select: BTN_SELECT
    (0x0080, 0x13b), // Start: BTN_START
    (0x0100, 0x13c), // Home: BTN_MODE
    (0x0200, 0x13d), // Left stick: BTN_THUMBL
    (0x0400, 0x13e), // Right stick: BTN_THUMBR
];

const DPAD_UP: u16 = 0x0800;
const DPAD_DOWN: u16 = 0x1000;
const DPAD_LEFT: u16 = 0x2000;
const DPAD_RIGHT: u16 = 0x4000;

/// One event for the pad: `EV_KEY` or `EV_ABS`, its code and value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PadEvent {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

/// The pad's state from one packet: its axes as the pad reports them, and
/// the buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PadState {
    abs: [(u16, i32); 8],
    buttons: u16,
}

/// A trigger as the wire sends it (0 up to 32767; less than 0 is none) on
/// the pad's range.
fn trigger(value: i16) -> i32 {
    i32::from(value.max(0)) * TRIGGER_MAX / i32::from(i16::MAX)
}

/// The hat from two opposite D-pad bits: -1, 0 or 1 (both held is none).
fn hat(buttons: u16, minus: u16, plus: u16) -> i32 {
    i32::from(buttons & plus != 0) - i32::from(buttons & minus != 0)
}

impl PadState {
    /// The wire's packet: left stick X and Y, right stick X and Y, L2, R2
    /// (pushed up is negative, as Linux has it), and the button bits.
    pub fn of(axes: &[i16; 6], buttons: u16) -> Self {
        Self {
            abs: [
                (ABS_X, i32::from(axes[0])),
                (ABS_Y, i32::from(axes[1])),
                (ABS_RX, i32::from(axes[2])),
                (ABS_RY, i32::from(axes[3])),
                (ABS_Z, trigger(axes[4])),
                (ABS_RZ, trigger(axes[5])),
                (ABS_HAT0X, hat(buttons, DPAD_LEFT, DPAD_RIGHT)),
                (ABS_HAT0Y, hat(buttons, DPAD_UP, DPAD_DOWN)),
            ],
            buttons,
        }
    }

    /// What changed from `previous` (all of it with none), less the
    /// closing sync, which the caller adds.
    pub fn events(&self, previous: Option<&PadState>) -> Vec<PadEvent> {
        let mut out = Vec::new();
        for (i, &(code, value)) in self.abs.iter().enumerate() {
            if previous.is_none_or(|p| p.abs[i].1 != value) {
                out.push(PadEvent {
                    kind: EV_ABS,
                    code,
                    value,
                });
            }
        }
        for &(bit, code) in &BUTTONS {
            let down = self.buttons & bit != 0;
            if previous.is_none_or(|p| (p.buttons & bit != 0) != down) {
                out.push(PadEvent {
                    kind: EV_KEY,
                    code,
                    value: i32::from(down),
                });
            }
        }
        out
    }

    /// The pad at rest: what is sent when a session ends, so no button or
    /// stick stays held in a game.
    pub fn rest() -> Self {
        Self::of(&[0; 6], 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: u16, value: i32) -> PadEvent {
        PadEvent {
            kind: EV_KEY,
            code,
            value,
        }
    }

    fn abs(code: u16, value: i32) -> PadEvent {
        PadEvent {
            kind: EV_ABS,
            code,
            value,
        }
    }

    #[test]
    fn the_first_packet_says_everything_and_later_ones_only_what_changed() {
        let first = PadState::of(&[0; 6], 0);
        assert_eq!(first.events(None).len(), 8 + BUTTONS.len());
        let a = PadState::of(&[0; 6], 0x0001);
        assert_eq!(a.events(Some(&first)), [key(0x130, 1)]);
        assert!(a.events(Some(&a)).is_empty(), "held: nothing new");
        assert_eq!(first.events(Some(&a)), [key(0x130, 0)], "released");
    }

    #[test]
    fn sticks_pass_through_and_triggers_take_the_pads_range() {
        let rest = PadState::rest();
        let moved = PadState::of(&[-32_768, 32_767, 100, -100, 32_767, -5], 0);
        assert_eq!(
            moved.events(Some(&rest)),
            [
                abs(ABS_X, -32_768),
                abs(ABS_Y, 32_767),
                abs(ABS_RX, 100),
                abs(ABS_RY, -100),
                abs(ABS_Z, TRIGGER_MAX),
            ],
            "a negative trigger is none"
        );
        assert_eq!(trigger(16_384), 127);
    }

    #[test]
    fn the_dpad_is_the_hat_and_the_face_buttons_are_linuxs() {
        let rest = PadState::rest();
        let up_left = PadState::of(&[0; 6], DPAD_UP | DPAD_LEFT);
        assert_eq!(
            up_left.events(Some(&rest)),
            [abs(ABS_HAT0X, -1), abs(ABS_HAT0Y, -1)]
        );
        let both = PadState::of(&[0; 6], DPAD_LEFT | DPAD_RIGHT);
        assert!(both.events(Some(&rest)).is_empty(), "both held is none");
        // X is BTN_X and Y BTN_Y, as xpad reports them and SDL maps them.
        let xy = PadState::of(&[0; 6], 0x0004 | 0x0008);
        assert_eq!(xy.events(Some(&rest)), [key(0x133, 1), key(0x134, 1)]);
        // Every bit but the D-pad's is a button.
        let all: u16 = BUTTONS.iter().map(|(bit, _)| bit).sum();
        assert_eq!(all | DPAD_UP | DPAD_DOWN | DPAD_LEFT | DPAD_RIGHT, 0x7FFF);
    }
}
