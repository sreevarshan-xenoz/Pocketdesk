//! The phone's folders as a drive (D8): where the server's WebDAV listener
//! is, kept owner-only in `<state>/drive.json` while it runs, for the
//! desktop app to show. The token is the whole of its protection on
//! loopback, so it is never logged.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::paths::DRIVE_FILE;

/// The drive's address: a loopback port and the token every path starts
/// with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriveInfo {
    pub port: u16,
    pub token: String,
}

impl DriveInfo {
    /// A new token: 32 random hex digits.
    pub fn new(port: u16) -> Self {
        Self {
            port,
            token: uuid::Uuid::new_v4().simple().to_string(),
        }
    }

    /// The address a file manager on Linux opens.
    pub fn dav_url(&self) -> String {
        format!("dav://127.0.0.1:{}/{}/", self.port, self.token)
    }

    /// The address Windows' Explorer opens (its WebDAV client's form), by
    /// IPv4: `localhost` may be tried as `::1` first, where nothing
    /// listens (D8 review).
    pub fn windows_path(&self) -> String {
        format!(r"\\127.0.0.1@{}\DavWWWRoot\{}", self.port, self.token)
    }
}

/// Whether `token` is one [`DriveInfo::new`] makes.
pub fn valid_token(token: &str) -> bool {
    token.len() == 32
        && token
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// The drive's address while the server serves it; `None` when it does not
/// or the file does not read.
pub fn read(state_dir: &Path) -> Option<DriveInfo> {
    std::fs::read_to_string(state_dir.join(DRIVE_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<DriveInfo>(&raw).ok())
        .filter(|info| info.port != 0 && valid_token(&info.token))
}

/// Keep `info` as the drive's address (the server's side), owner-only.
pub fn write(state_dir: &Path, info: &DriveInfo) -> io::Result<()> {
    let path = state_dir.join(DRIVE_FILE);
    let tmp = path.with_extension("json.tmp");
    std::fs::create_dir_all(state_dir)?;
    let json = serde_json::to_string(info).map_err(io::Error::other)?;
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &path)
}

/// Forget the address (the drive is off, or the server starts).
pub fn clear(state_dir: &Path) -> io::Result<()> {
    match std::fs::remove_file(state_dir.join(DRIVE_FILE)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_address_is_kept_read_and_cleared() {
        let dir = std::env::temp_dir().join(format!("pd-drive-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(read(&dir), None);
        let info = DriveInfo::new(51234);
        assert!(valid_token(&info.token));
        write(&dir, &info).unwrap();
        assert_eq!(read(&dir), Some(info.clone()));
        assert_eq!(
            info.dav_url(),
            format!("dav://127.0.0.1:51234/{}/", info.token)
        );
        assert!(
            info.windows_path()
                .starts_with(r"\\127.0.0.1@51234\DavWWWRoot\")
        );
        clear(&dir).unwrap();
        clear(&dir).unwrap();
        assert_eq!(read(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_token_is_32_lowercase_hex_digits() {
        assert!(valid_token(&"a".repeat(32)));
        assert!(!valid_token(&"A".repeat(32)));
        assert!(!valid_token(&"a".repeat(31)));
        assert!(!valid_token(&"g".repeat(32)));
    }
}
