//! Where the server and its companion app keep their files. Both resolve the
//! same way, so whatever one writes the other reads.

use std::io;
use std::path::PathBuf;

pub const CONFIG_FILE: &str = "config.toml";
pub const PID_FILE: &str = "server.pid";
pub const PIN_FILE: &str = "pairing.pin";
pub const TRUST_FILE: &str = "trusted_devices.json";
pub const LIVE_FILE: &str = "live_sessions.json";
pub const KICK_FILE: &str = "kick";
/// Requests from the desktop app to hand input control to a session (N7).
pub const CONTROL_DIR: &str = "control";
pub const ENDPOINT_FILE: &str = "endpoint.json";
pub const LOG_DIR: &str = "logs";
pub const CERT_FILE: &str = "certs/identity.der";
pub const DEVICE_ID_FILE: &str = "device_id";
pub const PENDING_APPROVALS_FILE: &str = "pending_approvals.json";
pub const APPROVAL_DIR: &str = "approvals";
pub const RECEIVED_FILE: &str = "received.json";
pub const OUTBOX_DIR: &str = "outbox";
pub const SENT_FILE: &str = "sent.json";
pub const DEVICE_KEYS_FILE: &str = "device_keys.json";
/// What the server measured about its internet path (N1): yes/no facts,
/// never an address.
pub const WAN_STATUS_FILE: &str = "wan_status.json";
/// One JSON record per streaming session, newest last.
pub const SESSIONS_FILE: &str = "streaming_sessions.jsonl";

fn missing(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("no {what} directory on this system"),
    )
}

/// `<config dir>/pocketdesk` (`%APPDATA%\pocketdesk`, `~/.config/pocketdesk`).
pub fn config_dir() -> io::Result<PathBuf> {
    Ok(app_dir(
        &dirs::config_dir().ok_or_else(|| missing("config"))?,
    ))
}

/// `<state dir>/pocketdesk`, falling back to local data (`%LOCALAPPDATA%` on
/// Windows, which has no state dir).
pub fn state_dir() -> io::Result<PathBuf> {
    Ok(app_dir(
        &dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .ok_or_else(|| missing("state"))?,
    ))
}

/// The folder name under the config and state directories.
pub const DIR_NAME: &str = "pocketdesk";

/// The name before the rename, moved to [`DIR_NAME`] on first use.
pub const OLD_DIR_NAME: &str = "linux-link";

/// `<base>/pocketdesk`. A folder from before the rename (`<base>/linux-link`)
/// is moved there the first time, so pairings, keys and settings carry
/// over; an existing new folder is never touched. If the move fails (a file
/// held open), the old folder keeps being used rather than starting empty.
fn app_dir(base: &std::path::Path) -> PathBuf {
    let new = base.join(DIR_NAME);
    let old = base.join(OLD_DIR_NAME);
    if new.exists() || !old.is_dir() {
        return new;
    }
    match std::fs::rename(&old, &new) {
        Ok(()) => new,
        Err(_) => old,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-paths-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_folder_from_before_the_rename_moves_to_the_new_name_once() {
        let base = base("migrate");
        std::fs::create_dir_all(base.join(OLD_DIR_NAME)).unwrap();
        std::fs::write(base.join(OLD_DIR_NAME).join(TRUST_FILE), "{}").unwrap();
        let dir = app_dir(&base);
        assert_eq!(dir, base.join(DIR_NAME));
        assert!(dir.join(TRUST_FILE).is_file(), "pairings carried over");
        assert!(!base.join(OLD_DIR_NAME).exists());
        assert_eq!(app_dir(&base), base.join(DIR_NAME), "and stays put");
    }

    #[test]
    fn an_existing_new_folder_is_never_overwritten() {
        let base = base("both");
        std::fs::create_dir_all(base.join(OLD_DIR_NAME)).unwrap();
        std::fs::create_dir_all(base.join(DIR_NAME)).unwrap();
        std::fs::write(base.join(DIR_NAME).join(TRUST_FILE), "new").unwrap();
        assert_eq!(app_dir(&base), base.join(DIR_NAME));
        assert_eq!(
            std::fs::read_to_string(base.join(DIR_NAME).join(TRUST_FILE)).unwrap(),
            "new"
        );
    }

    #[test]
    fn a_fresh_install_uses_the_new_name() {
        let base = base("fresh");
        assert_eq!(app_dir(&base), base.join(DIR_NAME));
    }
}
