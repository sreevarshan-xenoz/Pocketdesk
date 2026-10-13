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

fn missing(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("no {what} directory on this system"),
    )
}

/// `<config dir>/linux-link` (`%APPDATA%\linux-link`, `~/.config/linux-link`).
pub fn config_dir() -> io::Result<PathBuf> {
    Ok(dirs::config_dir()
        .ok_or_else(|| missing("config"))?
        .join("linux-link"))
}

/// `<state dir>/linux-link`, falling back to local data (`%LOCALAPPDATA%` on
/// Windows, which has no state dir).
pub fn state_dir() -> io::Result<PathBuf> {
    Ok(dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .ok_or_else(|| missing("state"))?
        .join("linux-link"))
}
