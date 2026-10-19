//! Lock the computer when the phone leaves (D9). A phone with "Lock this
//! computer when it leaves" (`AwayLock`) on counts as present while it has
//! a control link. Once every such phone that was present has been gone
//! for `away_lock_minutes`, and (on Windows) nobody has touched the
//! keyboard or mouse for that long, the computer locks: once per
//! departure. With `POCKETDESK_LOCK_FILE` set, the lock is a line in that
//! file instead (tests, and checks on a computer that must not lock).

use std::time::Duration;

use pocketdesk_core::protocol::kdeconnect::TrustStore;
use pocketdesk_core::protocol::permissions::Capability;

/// How often the links are looked at.
const TICK: Duration = Duration::from_secs(5);

/// The departure the computer locks on, as the watcher sees it.
#[derive(Debug, Default)]
pub struct AwayLock {
    /// A phone with the switch on was present since the last lock (or the
    /// server's start): the next departure may lock.
    armed: bool,
    /// When one was last seen present (seconds on the watcher's clock), and
    /// which.
    last_present: Option<(u64, String)>,
}

impl AwayLock {
    /// One look at `now` (seconds): `switched` phones have the switch on,
    /// `present` those of them with a link now; `idle_secs` how long the
    /// keyboard and mouse were quiet (`None` when unknown). The phone to
    /// say the computer locked for, when it should lock now.
    pub fn tick(
        &mut self,
        now: u64,
        switched: &[String],
        present: &[String],
        idle_secs: Option<u64>,
        after_secs: u64,
    ) -> Option<String> {
        // Nothing switched on (any more): nothing may lock, even a
        // departure already under way.
        if switched.is_empty() {
            *self = Self::default();
            return None;
        }
        if let Some(phone) = present.iter().find(|p| switched.contains(p)) {
            self.armed = true;
            self.last_present = Some((now, phone.clone()));
            return None;
        }
        let (since, phone) = self.last_present.clone()?;
        let gone = now.saturating_sub(since);
        // Someone at the keyboard is the owner: a phone whose network
        // sleeps while they work locks nothing.
        let quiet = idle_secs.is_none_or(|idle| idle >= after_secs);
        if self.armed && gone >= after_secs && quiet {
            self.armed = false;
            return Some(phone);
        }
        None
    }
}

/// How long the keyboard and mouse have been quiet, when the system says.
#[cfg(windows)]
fn idle_secs() -> Option<u64> {
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: a valid, sized LASTINPUTINFO the call fills in.
    if !unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        return None;
    }
    // SAFETY: no arguments; the tick count wraps every 49.7 days, as
    // dwTime does, so their wrapping difference is right.
    let now = unsafe { GetTickCount() };
    Some(u64::from(now.wrapping_sub(info.dwTime)) / 1000)
}

#[cfg(not(windows))]
fn idle_secs() -> Option<u64> {
    None
}

/// Lock the computer, or, with `POCKETDESK_LOCK_FILE` set, note the lock
/// in that file.
fn lock(phone: &str) -> Result<&'static str, String> {
    lock_to(std::env::var_os("POCKETDESK_LOCK_FILE"), phone)
}

/// [`lock`], with the seam's file given: a test names one, and so can never
/// reach the real lock.
fn lock_to(seam: Option<std::ffi::OsString>, phone: &str) -> Result<&'static str, String> {
    if let Some(file) = seam {
        use std::io::Write;
        let line = format!("{} {phone}\n", pocketdesk_core::local::pin::now_unix());
        return std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
            .and_then(|mut f| f.write_all(line.as_bytes()))
            .map(|()| "the lock file")
            .map_err(|e| format!("the lock file: {e}"));
    }
    crate::privacy::lock_screen()
}

/// The devices with the switch on.
fn switched() -> Vec<String> {
    let Some(store) = crate::state::trust_store_path()
        .ok()
        .and_then(|path| TrustStore::load_or_create(path).ok())
    else {
        return Vec::new();
    };
    store
        .paired_devices()
        .into_iter()
        .filter(|id| store.permissions(id).allows(Capability::AwayLock))
        .collect()
}

/// Watch the phones for the server's life, and lock as they leave.
pub async fn run() {
    let started = std::time::Instant::now();
    let mut away = AwayLock::default();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let switched = tokio::task::spawn_blocking(switched)
            .await
            .unwrap_or_default();
        let present: Vec<String> = crate::state::clone_clients()
            .await
            .iter()
            .map(|c| c.device_id().to_string())
            .collect();
        let after = pocketdesk_core::local::config::Config::load()
            .map(|c| c.away_lock_minutes)
            .unwrap_or(3)
            .clamp(1, 60);
        let now = started.elapsed().as_secs();
        let Some(phone) = away.tick(now, &switched, &present, idle_secs(), u64::from(after) * 60)
        else {
            continue;
        };
        match tokio::task::spawn_blocking(move || (lock(&phone), phone)).await {
            Ok((Ok(how), phone)) => {
                tracing::info!("{phone} left: locked this computer ({how})");
                crate::events::emit(
                    "away_locked",
                    Some(&phone),
                    serde_json::json!({}),
                    serde_json::json!({}),
                );
            }
            Ok((Err(e), phone)) => tracing::warn!("{phone} left, but the lock failed: {e}"),
            Err(e) => tracing::warn!("The away lock did not run: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    const AFTER: u64 = 180;

    #[test]
    fn it_locks_once_when_a_present_phone_has_been_gone_long_enough() {
        let on = ids(&["a"]);
        let mut away = AwayLock::default();
        // Never present since the start: nothing.
        assert_eq!(away.tick(0, &on, &[], None, AFTER), None);
        assert_eq!(away.tick(1000, &on, &[], None, AFTER), None);
        assert_eq!(away.tick(1005, &on, &on, None, AFTER), None);
        assert_eq!(
            away.tick(1100, &on, &[], None, AFTER),
            None,
            "gone too briefly"
        );
        assert_eq!(
            away.tick(1005 + AFTER, &on, &[], None, AFTER),
            Some("a".to_string())
        );
        assert_eq!(
            away.tick(5000, &on, &[], None, AFTER),
            None,
            "once per departure"
        );
        // Back, and gone again: it locks again.
        assert_eq!(away.tick(6000, &on, &on, None, AFTER), None);
        assert_eq!(
            away.tick(6000 + AFTER, &on, &[], None, AFTER),
            Some("a".to_string())
        );
    }

    #[test]
    fn someone_at_the_keyboard_keeps_it_unlocked() {
        let on = ids(&["a"]);
        let mut away = AwayLock::default();
        away.tick(0, &on, &on, Some(0), AFTER);
        assert_eq!(away.tick(AFTER, &on, &[], Some(10), AFTER), None, "typing");
        assert_eq!(
            away.tick(AFTER + 400, &on, &[], Some(AFTER), AFTER),
            Some("a".to_string()),
            "quiet as long as the phone is gone"
        );
    }

    #[test]
    fn several_phones_lock_only_when_the_last_has_left() {
        let on = ids(&["a", "b"]);
        let mut away = AwayLock::default();
        away.tick(0, &on, &on, None, AFTER);
        // One leaves, the other stays.
        assert_eq!(away.tick(AFTER * 2, &on, &ids(&["b"]), None, AFTER), None);
        assert_eq!(away.tick(AFTER * 3, &on, &ids(&["b"]), None, AFTER), None);
        // The last leaves.
        assert_eq!(away.tick(AFTER * 3 + 10, &on, &[], None, AFTER), None);
        assert_eq!(
            away.tick(AFTER * 4, &on, &[], None, AFTER),
            Some("b".to_string())
        );
        // A phone without the switch counts for nothing.
        let mut other = AwayLock::default();
        assert_eq!(other.tick(0, &ids(&["a"]), &ids(&["z"]), None, AFTER), None);
        assert_eq!(other.tick(AFTER * 2, &ids(&["a"]), &[], None, AFTER), None);
    }

    #[test]
    fn turning_the_switch_off_while_away_locks_nothing() {
        let on = ids(&["a"]);
        let mut away = AwayLock::default();
        away.tick(0, &on, &on, None, AFTER);
        assert_eq!(away.tick(60, &[], &[], None, AFTER), None);
        assert_eq!(
            away.tick(AFTER * 2, &on, &[], None, AFTER),
            None,
            "disarmed"
        );
    }

    #[test]
    fn the_seam_writes_a_line_instead_of_locking() {
        let file = std::env::temp_dir().join(format!("pd-lock-{}", std::process::id()));
        let _ = std::fs::remove_file(&file);
        assert_eq!(
            lock_to(Some(file.clone().into_os_string()), "phone-a"),
            Ok("the lock file")
        );
        let written = std::fs::read_to_string(&file).unwrap();
        assert!(written.trim_end().ends_with(" phone-a"), "{written}");
        let _ = std::fs::remove_file(&file);
    }
}
