//! The phone's pen as Windows' own pen (E3): a synthetic pointer device of
//! type pen (`CreateSyntheticPointerDevice(PT_PEN)`, Windows 10 1809+, no
//! driver), fed one `POINTER_PEN_INFO` per step the sample takes
//! ([`crate::pen::steps`]). Apps read it as a pen: pressure, tilt and the
//! barrel button, the same as a tablet's.
//!
//! Position is in the virtual screen's pixels (the server is per-monitor
//! DPI aware, so physical ones), unlike `SendInput`'s 0..=65535.

use anyhow::{Context, Result};
use pocketdesk_core::streaming::{PEN_BARREL, PEN_ERASER, PenSample};
use tracing::warn;
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Controls::{
    CreateSyntheticPointerDevice, DestroySyntheticPointerDevice, HSYNTHETICPOINTERDEVICE,
    POINTER_FEEDBACK_DEFAULT, POINTER_TYPE_INFO, POINTER_TYPE_INFO_0,
};
use windows::Win32::UI::Input::Pointer::{
    InjectSyntheticPointerInput, POINTER_FLAG_DOWN, POINTER_FLAG_INCONTACT, POINTER_FLAG_INRANGE,
    POINTER_FLAG_PRIMARY, POINTER_FLAG_UP, POINTER_FLAG_UPDATE, POINTER_FLAGS, POINTER_INFO,
    POINTER_PEN_INFO,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, PEN_FLAG_BARREL, PEN_FLAG_ERASER, PEN_MASK_PRESSURE, PEN_MASK_TILT_X,
    PEN_MASK_TILT_Y, PT_PEN, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN,
};

use crate::pen::{PenPhase, PenStep, Screen};

/// The pointer flags for one step: in range for all but leaving, in contact
/// while touching, and the transition's own flag.
pub(crate) fn step_flags(step: PenStep) -> POINTER_FLAGS {
    let flags = match step {
        PenStep::Enter | PenStep::Hover => POINTER_FLAG_INRANGE | POINTER_FLAG_UPDATE,
        PenStep::Down => POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT | POINTER_FLAG_DOWN,
        PenStep::Move => POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT | POINTER_FLAG_UPDATE,
        PenStep::Up => POINTER_FLAG_INRANGE | POINTER_FLAG_UP,
        PenStep::Leave => POINTER_FLAG_UPDATE,
    };
    flags | POINTER_FLAG_PRIMARY
}

/// The pen info for one step of `sample`, landing at `at`.
pub(crate) fn pen_info(sample: &PenSample, step: PenStep, at: (i32, i32)) -> POINTER_TYPE_INFO {
    let touching = matches!(step, PenStep::Down | PenStep::Move);
    let mut pen_flags = 0;
    if sample.flags & PEN_BARREL != 0 {
        pen_flags |= PEN_FLAG_BARREL;
    }
    if sample.flags & PEN_ERASER != 0 {
        pen_flags |= PEN_FLAG_ERASER;
    }
    let pen = POINTER_PEN_INFO {
        pointerInfo: POINTER_INFO {
            pointerType: PT_PEN,
            pointerFlags: step_flags(step),
            ptPixelLocation: POINT { x: at.0, y: at.1 },
            ..Default::default()
        },
        penFlags: pen_flags,
        penMask: PEN_MASK_PRESSURE | PEN_MASK_TILT_X | PEN_MASK_TILT_Y,
        pressure: if touching {
            crate::pen::windows_pressure(sample)
        } else {
            0
        },
        tiltX: i32::from(sample.tilt_x),
        tiltY: i32::from(sample.tilt_y),
        ..Default::default()
    };
    POINTER_TYPE_INFO {
        r#type: PT_PEN,
        Anonymous: POINTER_TYPE_INFO_0 { penInfo: pen },
    }
}

/// The virtual screen now (a monitor may have come or gone).
fn virtual_screen() -> Screen {
    // SAFETY: no arguments beyond the metric's index.
    unsafe {
        Screen {
            left: GetSystemMetrics(SM_XVIRTUALSCREEN),
            top: GetSystemMetrics(SM_YVIRTUALSCREEN),
            width: GetSystemMetrics(SM_CXVIRTUALSCREEN),
            height: GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

/// A synthetic pen, destroyed when dropped.
#[derive(Debug)]
pub(crate) struct WindowsPen {
    device: HSYNTHETICPOINTERDEVICE,
    phase: PenPhase,
    /// A refused injection was said; said again only once one works.
    refused_said: bool,
    /// Windows took a step from this pen once: refusals since are the
    /// window's (an elevated one, the secure desktop), not the pen's.
    worked: bool,
    /// Samples refused in a row before anything worked.
    refused_samples: u32,
}

/// Samples refused in a row, with none ever taken, before the pen is given
/// up for the mouse (E3 review): a pen this Windows makes but never takes.
pub(crate) const GIVE_UP_AFTER: u32 = 8;

// SAFETY: the handle is a plain device handle, used from one thread at a
// time behind the injector's `&mut self`.
unsafe impl Send for WindowsPen {}

impl WindowsPen {
    /// Make the device; an error on Windows before 10 1809.
    pub(crate) fn new() -> Result<Self> {
        // SAFETY: plain arguments; the handle is destroyed in `Drop`.
        let device = unsafe { CreateSyntheticPointerDevice(PT_PEN, 1, POINTER_FEEDBACK_DEFAULT) }
            .context("CreateSyntheticPointerDevice(PT_PEN)")?;
        Ok(Self {
            device,
            phase: PenPhase::Away,
            refused_said: false,
            worked: false,
            refused_samples: 0,
        })
    }

    /// Inject `sample`: each step it takes from where the pen was. A step
    /// Windows refuses (the secure desktop, an elevated window) is said
    /// once, and the pen moves on as if it had gone. An error once
    /// [`GIVE_UP_AFTER`] samples in a row were refused and none was ever
    /// taken: this pen does not work here, and the mouse should stand in.
    pub(crate) fn inject(&mut self, sample: &PenSample) -> Result<()> {
        let at = crate::pen::to_pixels(sample.x_norm, sample.y_norm, virtual_screen());
        let steps = crate::pen::steps(self.phase, sample);
        let mut taken = steps.is_empty();
        for step in steps {
            let info = [pen_info(sample, step, at)];
            // SAFETY: a device this struct made, and one filled pen info.
            match unsafe { InjectSyntheticPointerInput(self.device, &info) } {
                Ok(()) => {
                    self.refused_said = false;
                    taken = true;
                }
                Err(e) if !self.refused_said => {
                    self.refused_said = true;
                    warn!(error = %e, ?step, "Windows refused the pen's input");
                }
                Err(_) => {}
            }
        }
        self.phase = PenPhase::of(sample);
        if taken {
            self.worked = true;
            self.refused_samples = 0;
        } else {
            self.refused_samples += 1;
        }
        if give_up(self.worked, self.refused_samples) {
            anyhow::bail!("Windows took none of {} pen samples", self.refused_samples);
        }
        Ok(())
    }
}

/// Whether a pen that has [`worked`](WindowsPen) or not, with `refused`
/// samples in a row, is given up for the mouse.
pub(crate) fn give_up(worked: bool, refused: u32) -> bool {
    !worked && refused >= GIVE_UP_AFTER
}

impl Drop for WindowsPen {
    fn drop(&mut self) {
        // SAFETY: the device this struct made, destroyed once.
        unsafe { DestroySyntheticPointerDevice(self.device) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::streaming::{PEN_CONTACT, PEN_IN_RANGE};

    #[test]
    fn each_step_has_windows_flags_and_only_a_touch_presses() {
        assert_eq!(
            step_flags(PenStep::Down),
            POINTER_FLAG_INRANGE
                | POINTER_FLAG_INCONTACT
                | POINTER_FLAG_DOWN
                | POINTER_FLAG_PRIMARY
        );
        assert_eq!(
            step_flags(PenStep::Leave),
            POINTER_FLAG_UPDATE | POINTER_FLAG_PRIMARY,
            "leaving range is an update without in range"
        );
        let sample = PenSample {
            x_norm: 0,
            y_norm: 0,
            pressure: 65_535,
            tilt_x: 20,
            tilt_y: -10,
            flags: PEN_CONTACT | PEN_IN_RANGE | PEN_BARREL,
        };
        let down = pen_info(&sample, PenStep::Down, (10, 20));
        let up = pen_info(&sample, PenStep::Up, (10, 20));
        // SAFETY: both were filled as pen info.
        let (down, up) = unsafe { (down.Anonymous.penInfo, up.Anonymous.penInfo) };
        assert_eq!(down.pressure, crate::pen::WINDOWS_MAX_PRESSURE);
        assert_eq!(up.pressure, 0, "a lift presses nothing");
        assert_eq!((down.tiltX, down.tiltY), (20, -10));
        assert_eq!(down.penFlags, PEN_FLAG_BARREL);
        assert_eq!(down.pointerInfo.ptPixelLocation, POINT { x: 10, y: 20 });
        assert_eq!(down.pointerInfo.pointerType, PT_PEN);
    }

    #[test]
    fn a_pen_is_given_up_only_if_windows_never_took_it() {
        assert!(!give_up(false, GIVE_UP_AFTER - 1));
        assert!(give_up(false, GIVE_UP_AFTER));
        assert!(
            !give_up(true, 1000),
            "refusals after it worked are the window's, not the pen's"
        );
    }

    /// Makes a real synthetic pen and draws a stroke with it: run by hand
    /// (`--ignored`), with the pointer over a window that takes a pen.
    #[test]
    #[ignore = "injects a pen stroke into this computer"]
    fn a_synthetic_pen_draws_a_stroke() {
        let mut pen = WindowsPen::new().expect("a synthetic pen (Windows 10 1809+)");
        for i in 0..=20u16 {
            pen.inject(&PenSample {
                x_norm: 30_000 + i * 200,
                y_norm: 30_000,
                pressure: 20_000 + i * 2000,
                tilt_x: 0,
                tilt_y: 0,
                flags: PEN_CONTACT | PEN_IN_RANGE,
            })
            .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        pen.inject(&PenSample::default()).unwrap();
    }
}
