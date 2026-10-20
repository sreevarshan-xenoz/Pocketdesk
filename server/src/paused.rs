//! Whether this computer is paused (K2a): phones' notifications and texts
//! not shown, links not opened, rings not sounded. Read at each one, so a
//! pause from the tray, the desktop app or the CLI applies to the next.

/// Whether a pause is in force now.
pub fn now() -> bool {
    crate::state::state_dir().is_ok_and(|dir| {
        pocketdesk_core::local::pause::current(&dir, pocketdesk_core::local::pin::now_unix())
            .is_some()
    })
}
