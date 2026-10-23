//! Lock the computer when the phone leaves (D9). A phone with "Lock this
//! computer when it leaves" (`AwayLock`) on counts as present while it has
//! a control link. Once every such phone that was present has been gone
//! for `away_lock_minutes`, and nobody has touched the keyboard or mouse
//! for that long, the computer locks: once per departure. Where the
//! computer cannot say how long it has been idle (a wlroots desktop, a
//! session without GNOME, KDE on X11 or `xprintidle`), it never locks: a phone
//! whose network sleeps must not lock the owner out mid-work. On Linux the
//! idle time comes from GNOME, KDE or X11 (R3).
//! With `POCKETDESK_LOCK_FILE` set to an absolute path, the lock is a line
//! in that file instead (tests, and checks on a computer that must not
//! lock).

use std::collections::HashMap;
use std::time::Duration;

use pocketdesk_core::protocol::kdeconnect::TrustStore;
use pocketdesk_core::protocol::permissions::Capability;

/// How often the links are looked at.
const TICK: Duration = Duration::from_secs(5);

/// After a lock that failed, how long before it is tried again (D9 review).
const RETRY_SECS: u64 = 60;

/// The departure the computer locks on, as the watcher sees it.
#[derive(Debug, Default)]
pub struct AwayLock {
    /// A phone with the switch on was present since the last lock (or the
    /// server's start): the next departure may lock.
    armed: bool,
    /// When each phone with the switch on was last seen present (seconds on
    /// the watcher's clock).
    seen: HashMap<String, u64>,
    /// After a failed lock, when to try again.
    retry_at: Option<u64>,
}

impl AwayLock {
    /// Whether [`tick`](Self::tick) at `now` could lock, so the idle time
    /// is worth asking (R3 review: on Linux that is other programs, and
    /// every 5 s with nobody using the switch undid R6's idle cuts).
    pub fn wants_idle(
        &self,
        now: u64,
        switched: &[String],
        present: &[String],
        after_secs: u64,
    ) -> bool {
        if !self.armed || present.iter().any(|p| switched.contains(p)) {
            return false;
        }
        let last = self
            .seen
            .iter()
            .filter(|(phone, _)| switched.contains(phone))
            .map(|(_, at)| *at)
            .max();
        last.is_some_and(|last| now.saturating_sub(last) >= after_secs)
            && self.retry_at.is_none_or(|at| now >= at)
    }

    /// One look at `now` (seconds): `switched` phones have the switch on,
    /// `present` those with a link now; `idle_secs` how long the keyboard
    /// and mouse were quiet (`None` when unknown, which never locks). The
    /// phone to say the computer locked for, when it should lock now.
    pub fn tick(
        &mut self,
        now: u64,
        switched: &[String],
        present: &[String],
        idle_secs: Option<u64>,
        after_secs: u64,
    ) -> Option<String> {
        // A phone whose switch went off counts no more, present or gone
        // (D9 review): it neither arms nor locks.
        self.seen.retain(|phone, _| switched.contains(phone));
        let here: Vec<&String> = present.iter().filter(|p| switched.contains(p)).collect();
        if !here.is_empty() {
            for phone in here {
                self.seen.insert(phone.clone(), now);
            }
            self.armed = true;
            self.retry_at = None;
            return None;
        }
        // The last to leave, of those seen.
        let Some((phone, &last)) = self.seen.iter().max_by_key(|(_, at)| **at) else {
            self.armed = false;
            return None;
        };
        let gone = now.saturating_sub(last);
        // Someone at the keyboard is the owner; and with no way to tell,
        // nothing locks (D9 review).
        let quiet = idle_secs.is_some_and(|idle| idle >= after_secs);
        let due = self.retry_at.is_none_or(|at| now >= at);
        if self.armed && gone >= after_secs && quiet && due {
            self.armed = false;
            return Some(phone.clone());
        }
        None
    }

    /// The lock asked for at `now` failed: tried again a while later, while
    /// the phones stay away (D9 review).
    pub fn failed(&mut self, now: u64) {
        self.armed = true;
        self.retry_at = Some(now + RETRY_SECS);
    }
}

/// How long the keyboard and mouse have been quiet, when the system says.
#[cfg(windows)]
pub(crate) fn idle_secs() -> Option<u64> {
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
    // SAFETY: no arguments.
    let now = unsafe { GetTickCount() };
    Some(quiet_ms(now, info.dwTime) / 1000)
}

/// Milliseconds since the last input, from two wrapping tick counts: an
/// input stamped a moment after the tick was read is no time, not 49 days
/// (D9 review).
#[cfg_attr(not(windows), allow(dead_code))]
fn quiet_ms(now: u32, last_input: u32) -> u64 {
    u64::try_from((now.wrapping_sub(last_input) as i32).max(0)).unwrap_or(0)
}

/// R3: on Linux, from GNOME's idle monitor, KDE's screen saver or X11's
/// `xprintidle`, each given a second; none of them, none.
#[cfg(target_os = "linux")]
pub(crate) fn idle_secs() -> Option<u64> {
    const ASKS: [(&str, &[&str]); 2] = [
        (
            "gdbus",
            &[
                "call",
                "--session",
                "--dest",
                "org.gnome.Mutter.IdleMonitor",
                "--object-path",
                "/org/gnome/Mutter/IdleMonitor/Core",
                "--method",
                "org.gnome.Mutter.IdleMonitor.GetIdletime",
            ],
        ),
        (
            "gdbus",
            &[
                "call",
                "--session",
                "--dest",
                "org.freedesktop.ScreenSaver",
                "--object-path",
                "/org/freedesktop/ScreenSaver",
                "--method",
                "org.freedesktop.ScreenSaver.GetSessionIdleTime",
            ],
        ),
    ];
    for (program, args) in ASKS {
        if let Some(ms) = output_within(program, args).and_then(|out| first_number(&out)) {
            return Some(ms / 1000);
        }
    }
    // Never under Wayland, where XWayland sees only X clients' input (R3
    // review: a service that kept DISPLAY alone would read the owner idle).
    let x11 = std::env::var_os("DISPLAY").is_some_and(|d| !d.is_empty())
        && std::env::var_os("WAYLAND_DISPLAY").is_none_or(|d| d.is_empty())
        && std::env::var("XDG_SESSION_TYPE").map_or(true, |t| t != "wayland");
    if x11 {
        return output_within("xprintidle", &[])
            .and_then(|out| first_number(&out))
            .map(|ms| ms / 1000);
    }
    None
}

/// The first whole number in a tool's answer: GNOME's `(uint64 12345,)`,
/// KDE's `(uint32 12345,)`, `xprintidle`'s `12345`. All are read as
/// milliseconds; where a screen saver answers in seconds, the idle time
/// reads a thousand times shorter, so it never locks rather than too soon.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn first_number(out: &str) -> Option<u64> {
    let start = out.find(|c: char| c.is_ascii_digit())?;
    let digits: String = out[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    // Past the type's own digits: `uint64 12345` reads 12345, not 64.
    let rest = out[start + digits.len()..].trim_start();
    if out[..start].ends_with("int") && rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return first_number(rest);
    }
    digits.parse().ok()
}

/// A command's output, or none when it fails or takes over a second.
#[cfg(target_os = "linux")]
fn output_within(program: &str, args: &[&str]) -> Option<String> {
    use std::io::Read;
    use std::time::{Duration, Instant};
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        out
    });
    let end = Instant::now() + Duration::from_secs(1);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return reader.join().ok(),
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if Instant::now() >= end => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
pub(crate) fn idle_secs() -> Option<u64> {
    None
}

/// Lock the computer, or, with `POCKETDESK_LOCK_FILE` set, note the lock
/// in that file. Never the real lock in tests.
fn lock(phone: &str) -> Result<&'static str, String> {
    if cfg!(test) {
        return Err("never locked in tests".to_string());
    }
    lock_to(std::env::var_os("POCKETDESK_LOCK_FILE"), phone)
}

/// [`lock`], with the seam's file given: a test names one, and so can never
/// reach the real lock. A seam set to a relative path locks nothing: it was
/// meant to stand in.
fn lock_to(seam: Option<std::ffi::OsString>, phone: &str) -> Result<&'static str, String> {
    if let Some(file) = seam {
        if !std::path::Path::new(&file).is_absolute() {
            return Err("POCKETDESK_LOCK_FILE is not an absolute path".to_string());
        }
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

/// The devices with the switch on; `None` when the trust store does not
/// read now (written meanwhile), so the last answer stands (D9 review).
fn switched() -> Option<Vec<String>> {
    let store = TrustStore::read(crate::state::trust_store_path().ok()?).ok()?;
    Some(
        store
            .paired_devices()
            .into_iter()
            .filter(|id| store.permissions(id).allows(Capability::AwayLock))
            .collect(),
    )
}

/// The minutes away before a lock, `None` when the config does not read now.
fn minutes() -> Option<u32> {
    pocketdesk_core::local::config::Config::load()
        .ok()
        .map(|c| c.away_lock_minutes.clamp(2, 60))
}

/// Watch the phones for the server's life, and lock as they leave.
pub async fn run() {
    let started = std::time::Instant::now();
    let mut away = AwayLock::default();
    let mut last_switched = Vec::new();
    let mut last_minutes = 5;
    // Locks that failed, from the task that tried them.
    let (failed_tx, mut failed_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let now = started.elapsed().as_secs();
        while let Ok(phone) = failed_rx.try_recv() {
            away.failed(now);
            crate::events::emit(
                "away_locked",
                Some(&phone),
                serde_json::json!({ "failed": true }),
                serde_json::json!({}),
            );
        }
        // Blocking reads, off the runtime; each keeps its last good answer.
        if let Ok((switched, minutes)) =
            tokio::task::spawn_blocking(|| (switched(), minutes())).await
        {
            if let Some(switched) = switched {
                last_switched = switched;
            }
            if let Some(minutes) = minutes {
                last_minutes = minutes;
            }
        }
        let present: Vec<String> = crate::state::clone_clients()
            .await
            .iter()
            .map(|c| c.device_id().to_string())
            .collect();
        // Off the runtime, and only when a lock could follow: on Linux it
        // asks other programs (R3, R3 review).
        let idle = if away.wants_idle(now, &last_switched, &present, u64::from(last_minutes) * 60) {
            tokio::task::spawn_blocking(idle_secs).await.ok().flatten()
        } else {
            None
        };
        let Some(phone) = away.tick(
            now,
            &last_switched,
            &present,
            idle,
            u64::from(last_minutes) * 60,
        ) else {
            continue;
        };
        // Not waited for: a locker that blocks until the owner unlocks must
        // not stop the watcher (D9 review).
        let failed = failed_tx.clone();
        tokio::spawn(async move {
            let locked = tokio::task::spawn_blocking({
                let phone = phone.clone();
                move || lock(&phone)
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
            match locked {
                Ok(how) => {
                    tracing::info!("{phone} left: locked this computer ({how})");
                    crate::events::emit(
                        "away_locked",
                        Some(&phone),
                        serde_json::json!({}),
                        serde_json::json!({}),
                    );
                }
                Err(e) => {
                    tracing::warn!("{phone} left, but the lock failed: {e}");
                    let _ = failed.send(phone);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R3 review: the idle time is asked only once a lock could follow.
    #[test]
    fn the_idle_time_is_asked_only_when_a_lock_could_follow() {
        let phones = vec!["p".to_string()];
        let mut away = AwayLock::default();
        assert!(
            !away.wants_idle(0, &phones, &[], 300),
            "never seen: nothing to lock for"
        );
        away.tick(0, &phones, &phones, None, 300);
        assert!(!away.wants_idle(10, &phones, &phones, 300), "present");
        assert!(
            !away.wants_idle(100, &phones, &[], 300),
            "not gone long enough"
        );
        assert!(away.wants_idle(300, &phones, &[], 300), "gone its time");
        assert!(!away.wants_idle(300, &[], &[], 300), "its switch off");
    }

    /// R3: GNOME's, KDE's and xprintidle's answers read as milliseconds.
    #[test]
    fn the_idle_answers_are_read() {
        assert_eq!(first_number("(uint64 12345,)\n"), Some(12345));
        assert_eq!(first_number("(uint32 900,)"), Some(900));
        assert_eq!(first_number("(int32 5,)"), Some(5));
        assert_eq!(first_number("67890\n"), Some(67890));
        assert_eq!(
            first_number("Error: GDBus.Error:org.freedesktop.DBus.Error.ServiceUnknown"),
            None
        );
    }

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    const AFTER: u64 = 180;
    /// The keyboard quiet as long as the phone has been gone.
    const QUIET: Option<u64> = Some(100_000);

    #[test]
    fn it_locks_once_when_a_present_phone_has_been_gone_long_enough() {
        let on = ids(&["a"]);
        let mut away = AwayLock::default();
        // Never present since the start: nothing.
        assert_eq!(away.tick(0, &on, &[], QUIET, AFTER), None);
        assert_eq!(away.tick(1000, &on, &[], QUIET, AFTER), None);
        assert_eq!(away.tick(1005, &on, &on, QUIET, AFTER), None);
        assert_eq!(
            away.tick(1100, &on, &[], QUIET, AFTER),
            None,
            "gone too briefly"
        );
        assert_eq!(
            away.tick(1005 + AFTER, &on, &[], QUIET, AFTER),
            Some("a".to_string())
        );
        assert_eq!(
            away.tick(5000, &on, &[], QUIET, AFTER),
            None,
            "once per departure"
        );
        // Back, and gone again: it locks again.
        assert_eq!(away.tick(6000, &on, &on, QUIET, AFTER), None);
        assert_eq!(
            away.tick(6000 + AFTER, &on, &[], QUIET, AFTER),
            Some("a".to_string())
        );
    }

    #[test]
    fn someone_at_the_keyboard_or_no_way_to_tell_keeps_it_unlocked() {
        let on = ids(&["a"]);
        let mut away = AwayLock::default();
        away.tick(0, &on, &on, Some(0), AFTER);
        assert_eq!(away.tick(AFTER, &on, &[], Some(10), AFTER), None, "typing");
        assert_eq!(
            away.tick(AFTER * 9, &on, &[], None, AFTER),
            None,
            "idle unknown"
        );
        assert_eq!(
            away.tick(AFTER * 10, &on, &[], Some(AFTER), AFTER),
            Some("a".to_string()),
            "quiet as long as the phone is gone"
        );
    }

    #[test]
    fn several_phones_lock_only_when_the_last_has_left() {
        let on = ids(&["a", "b"]);
        let mut away = AwayLock::default();
        away.tick(0, &on, &on, QUIET, AFTER);
        // One leaves, the other stays.
        assert_eq!(away.tick(AFTER * 2, &on, &ids(&["b"]), QUIET, AFTER), None);
        assert_eq!(away.tick(AFTER * 3, &on, &ids(&["b"]), QUIET, AFTER), None);
        // The last leaves.
        assert_eq!(away.tick(AFTER * 3 + 10, &on, &[], QUIET, AFTER), None);
        assert_eq!(
            away.tick(AFTER * 4, &on, &[], QUIET, AFTER),
            Some("b".to_string())
        );
        // A phone without the switch counts for nothing.
        let mut other = AwayLock::default();
        assert_eq!(
            other.tick(0, &ids(&["a"]), &ids(&["z"]), QUIET, AFTER),
            None
        );
        assert_eq!(other.tick(AFTER * 2, &ids(&["a"]), &[], QUIET, AFTER), None);
    }

    #[test]
    fn turning_the_switch_off_while_away_locks_nothing() {
        let on = ids(&["a"]);
        let mut away = AwayLock::default();
        away.tick(0, &on, &on, QUIET, AFTER);
        assert_eq!(away.tick(60, &[], &[], QUIET, AFTER), None);
        assert_eq!(
            away.tick(AFTER * 2, &on, &[], QUIET, AFTER),
            None,
            "disarmed"
        );
        // D9 review: one phone's switch off, another's on that never came.
        let mut two = AwayLock::default();
        two.tick(0, &ids(&["a", "b"]), &ids(&["a"]), QUIET, AFTER);
        assert_eq!(
            two.tick(AFTER * 3, &ids(&["b"]), &[], QUIET, AFTER),
            None,
            "no phone with the switch on ever left"
        );
    }

    #[test]
    fn a_failed_lock_is_tried_again_a_while_later() {
        let on = ids(&["a"]);
        let mut away = AwayLock::default();
        away.tick(0, &on, &on, QUIET, AFTER);
        assert_eq!(
            away.tick(AFTER, &on, &[], QUIET, AFTER),
            Some("a".to_string())
        );
        away.failed(AFTER);
        assert_eq!(
            away.tick(AFTER + 5, &on, &[], QUIET, AFTER),
            None,
            "not at once"
        );
        assert_eq!(
            away.tick(AFTER + RETRY_SECS, &on, &[], QUIET, AFTER),
            Some("a".to_string())
        );
    }

    #[test]
    fn an_input_stamped_after_the_tick_was_read_is_no_time() {
        assert_eq!(quiet_ms(10_000, 4_000), 6_000);
        assert_eq!(quiet_ms(10_000, 10_003), 0, "a moment ahead");
        assert_eq!(quiet_ms(5, u32::MAX - 4), 10, "across the wrap");
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
        assert!(lock_to(Some("relative.txt".into()), "phone-a").is_err());
        assert!(lock("phone-a").is_err(), "never the real lock in tests");
    }
}
