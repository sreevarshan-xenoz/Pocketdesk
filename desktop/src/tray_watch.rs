//! What only the window can show (E2 review): a held session's Allow/Deny,
//! an ask-first task's, and a phone's screen. With the window closed the
//! tray program watches for them and starts the window, once for each new
//! one, as the hidden window used to raise itself.

use std::path::Path;

/// What waits now, each by a key of its own.
pub fn waiting(state_dir: &Path, approvals: &[String], now_unix: u64) -> Vec<String> {
    let mut keys: Vec<String> = approvals
        .iter()
        .map(|id| format!("approval:{id}"))
        .collect();
    keys.extend(
        pocketdesk_core::local::tasks::read_requests(state_dir, now_unix)
            .into_iter()
            .map(|r| format!("task:{}", r.id)),
    );
    if let Some(cast) = pocketdesk_core::local::cast::read(state_dir) {
        keys.push(format!("cast:{}", cast.session_id));
    }
    keys
}

/// What was already seen, so the window is started once for each.
#[derive(Debug, Default)]
pub struct Seen(Vec<String>);

impl Seen {
    /// Whether `now` holds anything not seen before; `now` is seen after.
    /// Once gone, a key may come back as new.
    pub fn any_new(&mut self, now: Vec<String>) -> bool {
        let new = now.iter().any(|key| !self.0.contains(key));
        self.0 = now;
        new
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(list: &[&str]) -> Vec<String> {
        list.iter().map(|k| k.to_string()).collect()
    }

    #[test]
    fn the_window_is_started_once_for_each_new_thing_waiting() {
        let mut seen = Seen::default();
        assert!(!seen.any_new(keys(&[])));
        assert!(seen.any_new(keys(&["approval:a"])));
        assert!(
            !seen.any_new(keys(&["approval:a"])),
            "already started for it"
        );
        assert!(seen.any_new(keys(&["approval:a", "cast:7"])));
        assert!(!seen.any_new(keys(&["cast:7"])), "one went: nothing new");
        assert!(!seen.any_new(keys(&[])));
        assert!(seen.any_new(keys(&["cast:7"])), "a cast again");
    }

    #[test]
    fn what_waits_is_read_from_the_state_folder() {
        let dir = std::env::temp_dir().join(format!("pd-tray-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(waiting(&dir, &[], 100), Vec::<String>::new());
        pocketdesk_core::local::cast::write(
            &dir,
            &pocketdesk_core::local::cast::CastFile {
                session_id: 7,
                device_id: "phone-a".into(),
                port: 1,
                token: String::new(),
            },
        )
        .unwrap();
        assert_eq!(
            waiting(&dir, &["r1".to_string()], 100),
            keys(&["approval:r1", "cast:7"])
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
