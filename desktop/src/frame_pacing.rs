//! The app's windows, and what their swap waits for.
//!
//! A swap that waits for the display's refresh (vsync) never tears, and on
//! Wayland it waits for the compositor's frame callback, which a window the
//! compositor does not show never gets: a computer's screen on a workspace
//! that is not shown, or under another window, held its swap for ever, and
//! as one thread draws every window of the app, every window stopped with
//! it. So on Wayland the swap never waits (a compositor shows whole frames
//! whatever the swap does), and elsewhere it waits unless Settings say
//! "show pictures without waiting for the display".
//!
//! A swap that does not wait leaves nothing to slow a window that asks for a
//! frame the moment the last is done (a spinner, any animation): measured,
//! 14,500 frames a second and a whole core. [`install`] puts that back: no
//! window starts frames closer than half the display's refresh period
//! (twice the refresh at most), waiting out what is left of that time
//! before a frame builds. A picture that comes after the gap draws at once,
//! so the latency the swap's wait cost (9.5 ms at the median) stays gone.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use eframe::egui;

/// What a display with no known refresh rate is taken to run at.
const DEFAULT_MILLIHERTZ: u32 = 60_000;

/// Whether a window's swap waits for the display's refresh: not on Wayland
/// (`on_wayland`: a window the compositor does not show would never get
/// its frame callback, and would stop every window of the app), and not
/// when Settings say not to (`no_vsync`); otherwise it does, which never
/// tears.
pub fn swap_waits(no_vsync: bool, on_wayland: bool) -> bool {
    !no_vsync && !on_wayland
}

/// [`swap_waits`] for this session: whether winit takes the Wayland
/// backend is [`crate::view_pointer::winit_on_wayland`]'s rule.
pub fn swap_waits_here(no_vsync: bool) -> bool {
    let display = std::env::var_os("WAYLAND_DISPLAY");
    let socket = std::env::var_os("WAYLAND_SOCKET");
    swap_waits(
        no_vsync,
        crate::view_pointer::winit_on_wayland(display.as_deref(), socket.as_deref()),
    )
}

/// The least time between the starts of two frames of one window: half the
/// refresh period of a display that runs at `millihertz` (60 Hz when not
/// known), so a window draws twice a refresh at most.
pub fn min_gap(millihertz: Option<u32>) -> Duration {
    let millihertz = millihertz
        .filter(|m| *m >= 1_000)
        .unwrap_or(DEFAULT_MILLIHERTZ);
    Duration::from_secs_f64(500.0 / f64::from(millihertz))
}

/// How much longer a frame asked for at `now` waits, for a window whose last
/// frame began at `last` (`None`: it has had none): what is left of `gap`.
pub fn wait_left(last: Option<Instant>, now: Instant, gap: Duration) -> Duration {
    last.map_or(Duration::ZERO, |last| {
        gap.saturating_sub(now.saturating_duration_since(last))
    })
}

/// Each window's last frame start, and the gap they keep.
pub struct Pacer {
    gap_micros: AtomicU64,
    last: Mutex<HashMap<egui::ViewportId, Instant>>,
}

impl Pacer {
    pub fn new(gap: Duration) -> Self {
        Self {
            gap_micros: AtomicU64::new(gap.as_micros() as u64),
            last: Mutex::new(HashMap::new()),
        }
    }

    /// Keep `gap` from now on.
    pub fn set_gap(&self, gap: Duration) {
        self.gap_micros
            .store(gap.as_micros() as u64, Ordering::Relaxed);
    }

    /// The gap kept.
    pub fn gap(&self) -> Duration {
        Duration::from_micros(self.gap_micros.load(Ordering::Relaxed))
    }

    /// How long window `id`, asking for a frame at `now`, waits; its frame
    /// is taken to begin when the wait is over.
    pub fn next_wait(&self, id: egui::ViewportId, now: Instant) -> Duration {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        let wait = wait_left(last.get(&id).copied(), now, self.gap());
        last.insert(id, now + wait);
        wait
    }

    /// Wait, if the window of this pass has to, before its frame builds.
    fn pace(&self, ctx: &egui::Context) {
        // A frame's second pass (a widget asked for a redo) follows the
        // first at once, and is no new frame.
        if ctx.current_pass_index() != 0 {
            return;
        }
        let wait = self.next_wait(ctx.viewport_id(), Instant::now());
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
    }
}

static PACER: OnceLock<Arc<Pacer>> = OnceLock::new();

/// Pace every window of `ctx` (the first call only): called where the swap
/// does not wait.
pub fn install(ctx: &egui::Context) {
    let pacer = PACER
        .get_or_init(|| Arc::new(Pacer::new(min_gap(None))))
        .clone();
    ctx.on_begin_pass(
        "pocketdesk-frame-pacing",
        Arc::new(move |ui: &mut egui::Ui| pacer.pace(ui.ctx())),
    );
}

/// Keep the gap to the refresh of the display the window `frame` is
/// drawn on, if the windows are paced. Cheap enough for every frame of the
/// main window.
pub fn watch_refresh(frame: &eframe::Frame) {
    let Some(pacer) = PACER.get() else {
        return;
    };
    let millihertz = frame
        .winit_window()
        .and_then(|window| window.current_monitor())
        .and_then(|monitor| monitor.refresh_rate_millihertz());
    let gap = min_gap(millihertz);
    if pacer.gap() != gap {
        pacer.set_gap(gap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_swap_waits_unless_wayland_or_settings_say_not_to() {
        assert!(swap_waits(false, false), "X11 and Windows: no tearing");
        assert!(!swap_waits(true, false), "the setting");
        assert!(
            !swap_waits(false, true),
            "Wayland: a window not shown gets no frame callback"
        );
        assert!(!swap_waits(true, true));
    }

    #[test]
    fn the_gap_is_half_a_refresh_period() {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        assert!((ms(min_gap(Some(60_000))) - 8.333).abs() < 0.01);
        assert!((ms(min_gap(Some(144_000))) - 3.472).abs() < 0.01);
        assert!((ms(min_gap(Some(59_940))) - 8.342).abs() < 0.01);
        // Not known, or nonsense: 60 Hz.
        assert_eq!(min_gap(None), min_gap(Some(60_000)));
        assert_eq!(min_gap(Some(0)), min_gap(Some(60_000)));
        assert_eq!(min_gap(Some(7)), min_gap(Some(60_000)));
    }

    #[test]
    fn a_frame_waits_for_what_is_left_of_the_gap() {
        let t0 = Instant::now();
        let gap = Duration::from_millis(8);
        assert_eq!(wait_left(None, t0, gap), Duration::ZERO, "its first");
        assert_eq!(wait_left(Some(t0), t0, gap), gap, "at once after the last");
        assert_eq!(
            wait_left(Some(t0), t0 + Duration::from_millis(3), gap),
            Duration::from_millis(5)
        );
        assert_eq!(
            wait_left(Some(t0), t0 + gap, gap),
            Duration::ZERO,
            "exactly a gap after"
        );
        assert_eq!(
            wait_left(Some(t0), t0 + Duration::from_secs(1), gap),
            Duration::ZERO,
            "long after"
        );
        // A clock that reads earlier than the last start: the whole gap, never more.
        assert_eq!(wait_left(Some(t0 + gap), t0, gap), gap);
    }

    #[test]
    fn each_window_keeps_its_own_gap_and_a_frame_counts_from_its_wait() {
        let pacer = Pacer::new(Duration::from_millis(8));
        let (a, b) = (
            egui::ViewportId::from_hash_of("a"),
            egui::ViewportId::from_hash_of("b"),
        );
        let t0 = Instant::now();
        assert_eq!(pacer.next_wait(a, t0), Duration::ZERO);
        // B has had none, whatever A did.
        assert_eq!(pacer.next_wait(b, t0), Duration::ZERO);
        // A asks again after 2 ms: 6 ms; the frame then begins at 8 ms, so
        // an ask at 10 ms waits 6 ms more, not 0.
        let at = |ms| t0 + Duration::from_millis(ms);
        assert_eq!(pacer.next_wait(a, at(2)), Duration::from_millis(6));
        assert_eq!(pacer.next_wait(a, at(10)), Duration::from_millis(6));
        // A window that asked long ago does not wait.
        assert_eq!(pacer.next_wait(a, at(1000)), Duration::ZERO);
        // The gap follows the display.
        pacer.set_gap(Duration::from_millis(4));
        assert_eq!(pacer.gap(), Duration::from_millis(4));
        assert_eq!(pacer.next_wait(b, at(1001)), Duration::ZERO);
        assert_eq!(pacer.next_wait(b, at(1002)), Duration::from_millis(3));
    }

    /// The hook is in egui's passes: an animating window (a spinner)
    /// draws no more often than the gap lets it, and a window that waits
    /// does not wait on the frame's second pass.
    #[test]
    fn an_animating_window_is_held_to_the_gap() {
        let ctx = egui::Context::default();
        let pacer = Arc::new(Pacer::new(Duration::from_millis(20)));
        let hook = pacer.clone();
        ctx.on_begin_pass(
            "test-pacing",
            Arc::new(move |ui: &mut egui::Ui| hook.pace(ui.ctx())),
        );
        let begin = Instant::now();
        for _ in 0..4 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.spinner();
            });
            output.textures_delta.clear();
        }
        // Four frames from the first: three gaps at least.
        assert!(
            begin.elapsed() >= Duration::from_millis(58),
            "{:?}",
            begin.elapsed()
        );
    }
}
