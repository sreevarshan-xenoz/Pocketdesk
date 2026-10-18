//! The folders the desktop app and the local API leave requests in, watched
//! instead of polled (reliability pass, R6c). Each consumer used to wake on
//! its own timer, 250 ms to 1 s, about twenty times a second in all, which
//! doubled the idle server's CPU on Windows (R6b). Now one file-system
//! watcher wakes the consumer whose folder changed, and each still looks
//! every [`SWEEP`] in case an event was lost (Windows drops them when its
//! buffer overflows). Where no watcher can be started, each waits its old
//! interval instead, so nothing is slower than it was.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::Duration;

use pocketdesk_core::local::paths::{
    CLIPBOARD_OUT_DIR, COMPUTERS_DIR, CONTROL_DIR, KDECONNECT_OUT_DIR, KICKS_DIR, LINKS_DIR,
    NOTIFICATION_OUT_DIR, OUTBOX_DIR, PHONE_FILES_DIR, PHONE_MEDIA_DIR, PHONE_PHOTOS_OUT_DIR,
    QUIET_DIR, RINGS_DIR, TRANSFER_CANCELS_DIR,
};
use tokio::sync::Notify;

/// How often a watched folder is looked at anyway.
pub const SWEEP: Duration = Duration::from_secs(5);

/// The folders watched, each a consumer's inbox.
pub const FOLDERS: &[&str] = &[
    OUTBOX_DIR,
    LINKS_DIR,
    RINGS_DIR,
    QUIET_DIR,
    CLIPBOARD_OUT_DIR,
    PHONE_MEDIA_DIR,
    PHONE_FILES_DIR,
    PHONE_PHOTOS_OUT_DIR,
    NOTIFICATION_OUT_DIR,
    KICKS_DIR,
    CONTROL_DIR,
    COMPUTERS_DIR,
    KDECONNECT_OUT_DIR,
    TRANSFER_CANCELS_DIR,
];

/// One wake-up per folder; a change while its consumer is busy is kept for
/// its next wait.
static WAKES: LazyLock<Mutex<HashMap<&'static str, Arc<Notify>>>> = LazyLock::new(Default::default);

/// Whether each folder changed since its consumer last looked (for loops
/// that keep their own tick, as the sessions mirror does).
static CHANGED: LazyLock<Mutex<HashMap<&'static str, bool>>> = LazyLock::new(Default::default);

/// The watcher, kept alive for the server's life; `None` when it could not
/// start.
static WATCHER: OnceLock<Option<notify::RecommendedWatcher>> = OnceLock::new();

fn wake_for(folder: &'static str) -> Arc<Notify> {
    WAKES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(folder)
        .or_insert_with(|| Arc::new(Notify::new()))
        .clone()
}

/// The inbox a changed `path` belongs to: the folder it is in, if watched.
pub fn folder_of(path: &Path) -> Option<&'static str> {
    let parent = path.parent()?.file_name()?.to_str()?;
    FOLDERS.iter().copied().find(|f| *f == parent)
}

/// Make the folders and watch them. Called once at start; without a watcher
/// (an error, or a file system that sends no events) each consumer waits its
/// own interval.
pub fn start(state_dir: &Path) {
    WATCHER.get_or_init(|| {
        use notify::Watcher;
        let mut watcher = notify::recommended_watcher(|event: notify::Result<notify::Event>| {
            // An error, or events lost (a full buffer on Windows): every
            // consumer looks now rather than at its next sweep.
            let event = match event {
                Ok(event) if !event.need_rescan() && !event.paths.is_empty() => event,
                Ok(_) => return nudge_all(),
                Err(e) => {
                    tracing::debug!("Folder watcher: {e}");
                    return nudge_all();
                }
            };
            for path in &event.paths {
                if let Some(folder) = folder_of(path) {
                    nudge(folder);
                }
            }
        })
        .map_err(|e| tracing::info!("No folder watcher ({e}); the inboxes are polled"))
        .ok()?;
        for folder in FOLDERS {
            let dir = state_dir.join(folder);
            if let Err(e) = std::fs::create_dir_all(&dir)
                .map_err(notify::Error::io)
                .and_then(|()| watcher.watch(&dir, notify::RecursiveMode::NonRecursive))
            {
                tracing::info!(
                    "Not watching {}: {e}; the inboxes are polled",
                    dir.display()
                );
                return None;
            }
        }
        tracing::debug!("Watching {} inbox folders", FOLDERS.len());
        Some(watcher)
    });
    // Each consumer's first look is now, not a sweep away: requests left
    // while the server was stopped, and files that start empty.
    static FIRST: std::sync::Once = std::sync::Once::new();
    FIRST.call_once(nudge_all);
}

fn nudge_all() {
    for folder in FOLDERS {
        nudge(folder);
    }
}

/// Wake `folder`'s consumer as if a file had appeared in it: for something
/// it must act on that is not a file, such as a phone's report to write out.
pub fn nudge(folder: &'static str) {
    CHANGED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(folder, true);
    wake_for(folder).notify_one();
}

/// Whether `folder` may hold something new since this was last asked: a
/// change seen, or no watcher to see one (so the caller looks every time).
pub fn changed(folder: &'static str) -> bool {
    if !watching() {
        return true;
    }
    CHANGED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(folder, false)
        .unwrap_or(false)
}

/// Whether the folders are watched.
pub fn watching() -> bool {
    WATCHER.get().is_some_and(Option::is_some)
}

/// Wait until `folder` may hold something new: a change in it, or
/// [`SWEEP`]; or, with no watcher, `unwatched` (the consumer's old poll).
pub async fn wait(folder: &'static str, unwatched: Duration) {
    wait_at_most(folder, SWEEP, unwatched).await;
}

/// [`wait`], but for no longer than `most`: for a consumer that also has a
/// deadline of its own.
pub async fn wait_at_most(folder: &'static str, most: Duration, unwatched: Duration) {
    if watching() {
        let wake = wake_for(folder);
        let _ = tokio::time::timeout(most, wake.notified()).await;
    } else {
        tokio::time::sleep(unwatched.min(most)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_belongs_to_the_folder_it_is_in() {
        let state = Path::new("/state");
        assert_eq!(
            folder_of(&state.join(LINKS_DIR).join("a.json")),
            Some(LINKS_DIR)
        );
        assert_eq!(
            folder_of(&state.join(OUTBOX_DIR).join("b.json.tmp")),
            Some(OUTBOX_DIR)
        );
        assert_eq!(
            folder_of(&state.join("live_sessions.json")),
            None,
            "not an inbox"
        );
        assert_eq!(folder_of(&state.join("events.jsonl")), None);
    }

    #[test]
    fn a_nudge_is_seen_once() {
        // A name no consumer has, so no other test's nudge reaches it.
        const FOLDER: &str = "test_seen_once";
        nudge(FOLDER);
        if watching() {
            assert!(changed(FOLDER));
            assert!(!changed(FOLDER), "seen, so not again");
        } else {
            assert!(changed(FOLDER), "no watcher: always look");
        }
    }

    #[tokio::test]
    async fn a_wait_with_a_deadline_ends_at_it_or_at_a_change() {
        // A name no consumer has, so no other test's nudge ends the wait.
        const FOLDER: &str = "test_deadline";
        let start = tokio::time::Instant::now();
        let most = Duration::from_millis(200);
        wait_at_most(FOLDER, most, most).await;
        let took = start.elapsed();
        assert!(
            took >= Duration::from_millis(190) && took < Duration::from_secs(2),
            "{took:?}"
        );
        nudge(FOLDER);
        let start = tokio::time::Instant::now();
        wait_at_most(FOLDER, Duration::from_secs(3), Duration::from_secs(3)).await;
        if watching() {
            assert!(
                start.elapsed() < Duration::from_millis(500),
                "the change ends it"
            );
        }
    }

    #[tokio::test]
    async fn a_file_written_wakes_its_folder_at_once() {
        let dir = std::env::temp_dir().join(format!("pd-inbox-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        start(&dir);
        assert!(watching(), "the watcher starts on this system");
        // A wait that started before the file is written.
        let waiting = tokio::spawn(async {
            let start = tokio::time::Instant::now();
            wait(LINKS_DIR, Duration::from_secs(30)).await;
            start.elapsed()
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        std::fs::write(dir.join(LINKS_DIR).join("x.json"), b"{}").unwrap();
        let took = waiting.await.unwrap();
        assert!(
            took < Duration::from_secs(2),
            "woken by the change, not the sweep: {took:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
