//! Whether this computer is paused (K2a): phones' notifications and texts
//! not shown, links not opened, a phone ringing this computer silent. Read at each one, so a
//! pause from the tray, the desktop app or the CLI applies to the next.

/// Whether a pause is in force now.
#[cfg(not(test))]
pub fn now() -> bool {
    crate::state::state_dir().is_ok_and(|dir| {
        pocketdesk_core::local::pause::current(&dir, pocketdesk_core::local::pin::now_unix())
            .is_some()
    })
}

/// In tests, the thread's own answer, never the owner's pause (K2a
/// review): a test that reaches a hold must not depend on whether the
/// computer running it is paused, nor on another test's pause.
#[cfg(test)]
pub fn now() -> bool {
    PAUSED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&std::thread::current().id())
}

/// The test threads paused now.
#[cfg(test)]
static PAUSED: std::sync::Mutex<Vec<std::thread::ThreadId>> = std::sync::Mutex::new(Vec::new());

/// Paused on this thread until the guard drops (tests).
#[cfg(test)]
pub fn paused_for_test() -> impl Drop {
    struct Resume(std::thread::ThreadId);
    impl Drop for Resume {
        fn drop(&mut self) {
            PAUSED
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|id| *id != self.0);
        }
    }
    let id = std::thread::current().id();
    PAUSED.lock().unwrap_or_else(|e| e.into_inner()).push(id);
    Resume(id)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_test_is_paused_only_by_its_guard() {
        assert!(!super::now());
        let guard = super::paused_for_test();
        assert!(super::now());
        drop(guard);
        assert!(!super::now());
    }
}
