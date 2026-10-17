//! The phone's Cancel for the file transfer the bridge is running (C1).
//!
//! The phone runs one transfer at a time, each a blocking call from its
//! transfer service; [`cancellable`] lets [`cancel_current`], called from
//! another thread, end the one running with [`CANCELLED`]. What a download
//! had committed stays for a later attempt, as after any break.

use std::future::Future;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio_util::sync::CancellationToken;

/// What a cancelled transfer returns; the phone's retry rule never retries it.
pub(crate) const CANCELLED: &str = "cancelled on the phone";

static NEXT: AtomicU64 = AtomicU64::new(1);
/// The running transfer's stop, by its number.
static CURRENT: Mutex<Option<(u64, CancellationToken)>> = Mutex::new(None);

fn current() -> std::sync::MutexGuard<'static, Option<(u64, CancellationToken)>> {
    CURRENT.lock().unwrap_or_else(|e| e.into_inner())
}

/// Run `work` as the transfer [`cancel_current`] stops.
pub(crate) async fn cancellable<T>(
    work: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
    let number = NEXT.fetch_add(1, Ordering::Relaxed);
    let token = CancellationToken::new();
    *current() = Some((number, token.clone()));
    let out = tokio::select! {
        out = work => out,
        () = token.cancelled() => Err(CANCELLED.to_string()),
    };
    let mut slot = current();
    // Only this transfer's own stop: a later one may have taken the slot.
    if slot.as_ref().is_some_and(|(n, _)| *n == number) {
        *slot = None;
    }
    out
}

/// Stop the transfer running now, if any.
pub(crate) fn cancel_current() {
    if let Some((_, token)) = current().take() {
        token.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One test, so no other test's transfer shares the slot.
    #[tokio::test]
    async fn a_cancel_stops_the_transfer_running_and_nothing_else() {
        // Nothing running: nothing to stop, and nothing left to stop the next.
        cancel_current();
        assert_eq!(cancellable(async { Ok::<_, String>(7) }).await, Ok(7));

        let running = tokio::spawn(cancellable(std::future::pending::<Result<u8, String>>()));
        while current().is_none() {
            tokio::task::yield_now().await;
        }
        cancel_current();
        assert_eq!(running.await.unwrap(), Err(CANCELLED.to_string()));
        assert!(current().is_none(), "its stop is gone with it");

        // A transfer that ended leaves no stop behind for a later Cancel.
        assert_eq!(
            cancellable(async { Err::<u8, _>("broke".to_string()) }).await,
            Err("broke".to_string())
        );
        assert!(current().is_none());
    }
}
