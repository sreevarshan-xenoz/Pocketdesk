//! A computer's screen window here holds the pointer (PC7): while it does,
//! the shared keyboard and mouse leave the screens' edges alone, so the
//! window's relative mouse and an edge crossing never both drive (the plan's
//! gap 12).
//!
//! The view relay takes a [`PointerHold`] when its window says its pointer
//! is locked and drops it when the window frees it, closes, or its
//! connection ends; the driver's loop reads [`pointer_held`] each pass, as
//! it reads [`super::receive::driven_by`], and [`super::drive::Driver`]
//! stands down meanwhile. A count, not a flag, so a window replaced by a
//! newer one never clears the newer one's hold as its reader winds down.

use std::sync::atomic::{AtomicUsize, Ordering};

static HOLDS: AtomicUsize = AtomicUsize::new(0);

/// A window's hold on the pointer, given back as it drops.
#[derive(Debug)]
pub struct PointerHold(());

impl PointerHold {
    pub fn take() -> PointerHold {
        HOLDS.fetch_add(1, Ordering::SeqCst);
        PointerHold(())
    }
}

impl Drop for PointerHold {
    fn drop(&mut self) {
        HOLDS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Tests that count holds take turns (they share the one count).
#[cfg(test)]
pub static TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Whether a window here holds the pointer now.
pub fn pointer_held() -> bool {
    HOLDS.load(Ordering::SeqCst) > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Holds overlap as a newer window takes the pointer before an older
    /// one's reader has wound down: held until the last is given back.
    #[test]
    fn the_pointer_is_held_until_the_last_hold_goes() {
        let _alone = TESTS.blocking_lock();
        let before = HOLDS.load(Ordering::SeqCst);
        let older = PointerHold::take();
        let newer = PointerHold::take();
        assert!(pointer_held());
        drop(older);
        assert!(pointer_held(), "the newer window still holds it");
        assert_eq!(HOLDS.load(Ordering::SeqCst), before + 1);
        drop(newer);
        assert_eq!(HOLDS.load(Ordering::SeqCst), before);
    }
}
