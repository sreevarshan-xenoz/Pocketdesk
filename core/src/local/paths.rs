//! Where the server and its companion app keep their files. Both resolve the
//! same way, so whatever one writes the other reads.

use std::io;
use std::path::PathBuf;

pub const CONFIG_FILE: &str = "config.toml";
pub const PID_FILE: &str = "server.pid";
pub const PIN_FILE: &str = "pairing.pin";
pub const TRUST_FILE: &str = "trusted_devices.json";
pub const LIVE_FILE: &str = "live_sessions.json";
/// The phone's cast the server holds now, for the desktop app (T10k).
pub const CAST_FILE: &str = "cast.json";
/// What Pocketdesk did, one event per line (ecosystem phase 1a).
pub const EVENTS_FILE: &str = "events.jsonl";
/// Task starts held for the owner (ecosystem 1c), and the owner's answers.
pub const TASK_REQUESTS_FILE: &str = "task_requests.json";
pub const TASK_ANSWERS_DIR: &str = "task_answers";
pub const KICK_FILE: &str = "kick";
/// One kick request per file (roadmap N7).
pub const KICKS_DIR: &str = "kicks";
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
/// Links the desktop app or the local API asks the server to hand to a phone (T8a).
pub const LINKS_DIR: &str = "links_out";
pub const SENT_FILE: &str = "sent.json";
/// How the bridge to Home Assistant's MQTT broker stands (ecosystem 1e).
pub const HOME_ASSISTANT_FILE: &str = "home_assistant.json";
/// Each phone's last battery report (T8b), written by the server.
pub const PHONE_STATUS_FILE: &str = "phone_status.json";
/// Each phone's last media report (T8d), written by the server.
pub const PHONE_MEDIA_FILE: &str = "phone_media.json";
/// Each phone's last listing of its shared folders (T10g).
pub const PHONE_FILES_FILE: &str = "phone_files.json";
/// The desktop app's requests to browse them (T10g).
pub const PHONE_FILES_DIR: &str = "phone_files_out";
/// Phone notifications the computer can answer (T10c).
pub const PHONE_NOTES_FILE: &str = "phone_notifications.json";
/// Answers to them the desktop app asks the server to send (T10c).
pub const NOTIFICATION_OUT_DIR: &str = "notification_out";
/// Presses for a phone's player (T8d), one file each, taken by the server.
pub const PHONE_MEDIA_DIR: &str = "phone_media_out";
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
/// `POCKETDESK_CONFIG_DIR` names another folder, so a second server (for
/// tests, or a second set of settings) has its own `config.toml` (its own
/// ports) and its own iroh identity; set `POCKETDESK_STATE_DIR` with it for
/// its own pairings and device id.
pub fn config_dir() -> io::Result<PathBuf> {
    if let Some(dir) = CONFIG_DIR_OVERRIDE.get() {
        return Ok(dir.clone());
    }
    if let Some(dir) = dir_from_env(CONFIG_DIR_VAR, std::env::var_os(CONFIG_DIR_VAR)) {
        return dir;
    }
    Ok(app_dir(
        &dirs::config_dir().ok_or_else(|| missing("config"))?,
    ))
}

/// The state folder this process uses instead of the user's, once set.
static STATE_DIR_OVERRIDE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Use `dir` as the state folder for the rest of this process, and return
/// the one in use (the first caller's: it can be set only once). For tests:
/// the server's gates then read a throwaway folder, never the user's
/// pairings and keys.
pub fn use_state_dir(dir: PathBuf) -> &'static std::path::Path {
    STATE_DIR_OVERRIDE.get_or_init(|| dir)
}

/// The config folder, set in-process as [`use_state_dir`] sets the state
/// folder: for a test that must not read the owner's config (the idle
/// harness). Set once, before anything reads the config.
static CONFIG_DIR_OVERRIDE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Use `dir` as the config folder for the rest of this process.
pub fn use_config_dir(dir: PathBuf) -> &'static std::path::Path {
    CONFIG_DIR_OVERRIDE.get_or_init(|| dir)
}

/// `<state dir>/pocketdesk`, falling back to local data (`%LOCALAPPDATA%` on
/// Windows, which has no state dir). `POCKETDESK_STATE_DIR` names another
/// folder (a portable install, or a separate set of pairings).
pub fn state_dir() -> io::Result<PathBuf> {
    if let Some(dir) = STATE_DIR_OVERRIDE.get() {
        return Ok(dir.clone());
    }
    if let Some(dir) = dir_from_env(STATE_DIR_VAR, std::env::var_os(STATE_DIR_VAR)) {
        return dir;
    }
    Ok(app_dir(
        &dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .ok_or_else(|| missing("state"))?,
    ))
}

/// Make the state folder `dir` owner-only on Unix (`0700`): it holds the
/// pairings, the identity key, received texts and session records, and a
/// folder only the owner can open keeps them private whatever mode each
/// file was written with. Creates it if needed. Returns whether the mode was
/// changed, so the caller can say so; a folder that cannot be changed (one
/// the user does not own, or a filesystem without modes) is an error the
/// caller may treat as a warning. On Windows the default folder
/// (`%LOCALAPPDATA%`) is already the user's own, and its DACL is left alone.
pub fn protect_state_dir(dir: &std::path::Path) -> io::Result<bool> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir)?.permissions().mode() & 0o777;
        if mode != 0o700 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            return Ok(true);
        }
    }
    Ok(false)
}

/// The variable that moves the state folder.
pub const STATE_DIR_VAR: &str = "POCKETDESK_STATE_DIR";
/// The variable that moves the config folder.
pub const CONFIG_DIR_VAR: &str = "POCKETDESK_CONFIG_DIR";

/// The folder the variable `name` names, if it is set. It must be absolute:
/// a relative one would resolve against each process's own working folder,
/// so the server and the desktop app could each use a different one without
/// a word.
fn dir_from_env(name: &str, value: Option<std::ffi::OsString>) -> Option<io::Result<PathBuf>> {
    let dir = PathBuf::from(value.filter(|d| !d.is_empty())?);
    Some(if dir.is_absolute() {
        Ok(dir)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must be an absolute path, not {}", dir.display()),
        ))
    })
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

    #[cfg(unix)]
    #[test]
    fn the_state_dir_is_made_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("pd-protect-{}", std::process::id()));
        let dir = base.join("state");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(protect_state_dir(&dir).unwrap(), "changed");
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        assert!(!protect_state_dir(&dir).unwrap(), "already private");
        protect_state_dir(&base.join("fresh")).unwrap();
        let mode = std::fs::metadata(base.join("fresh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700, "a new folder too");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_folder_variable_must_name_an_absolute_folder() {
        for name in [STATE_DIR_VAR, CONFIG_DIR_VAR] {
            assert!(dir_from_env(name, None).is_none());
            assert!(
                dir_from_env(name, Some("".into())).is_none(),
                "empty is unset"
            );
            let absolute = std::env::temp_dir().join("pd-second");
            assert_eq!(
                dir_from_env(name, Some(absolute.clone().into()))
                    .unwrap()
                    .unwrap(),
                absolute
            );
            let relative = dir_from_env(name, Some("pd-second".into())).unwrap();
            let message = relative.unwrap_err().to_string();
            assert!(
                message.contains(name),
                "relative is refused, not resolved against the working folder: {message}"
            );
        }
    }

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
