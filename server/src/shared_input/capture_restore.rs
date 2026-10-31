//! The input-capture portal's restore token (BK8): kept so the desktop's
//! "allow this app to capture input" dialog shows once, not at every start.
//!
//! The portal hands back a new token at each start, good for one restore
//! (it stands in for the owner's answer to that dialog), so it is kept
//! like a secret: in the state folder, readable only by the owner
//! ([`write_private`]), written whole, never logged (its `Debug` says
//! nothing of it), and forgotten when the portal refuses it.
//!
//! [`write_private`]: pocketdesk_core::secure_fs::write_private

use std::fmt;
use std::io;
use std::path::Path;

/// The token's file in the state folder.
pub const TOKEN_FILE: &str = "input_capture.token";

/// The longest token kept: a portal's is a short id; anything longer is
/// not one.
const MOST: usize = 4096;

/// A restore token. Its `Debug` hides it.
#[derive(Clone, PartialEq, Eq)]
pub struct RestoreToken(String);

impl fmt::Debug for RestoreToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RestoreToken(..)")
    }
}

impl RestoreToken {
    /// A token the portal gave: none for an empty one, or one too long or
    /// with control characters to be a token.
    pub fn new(token: &str) -> Option<RestoreToken> {
        let token = token.trim();
        (!token.is_empty() && token.len() <= MOST && !token.chars().any(char::is_control))
            .then(|| RestoreToken(token.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The token kept in `state_dir`, if there is one that reads as a token.
pub fn read(state_dir: &Path) -> Option<RestoreToken> {
    let bytes = std::fs::read(state_dir.join(TOKEN_FILE)).ok()?;
    if bytes.len() > MOST + 2 {
        return None;
    }
    RestoreToken::new(std::str::from_utf8(&bytes).ok()?)
}

/// Keep `token` in `state_dir`, owner-only, replacing the last.
pub fn write(state_dir: &Path, token: &RestoreToken) -> io::Result<()> {
    std::fs::create_dir_all(state_dir)?;
    let path = state_dir.join(TOKEN_FILE);
    let tmp = path.with_extension("token.tmp");
    pocketdesk_core::secure_fs::write_private(&tmp, token.0.as_bytes())?;
    pocketdesk_core::atomic_file::replace(&tmp, &path)
}

/// Forget the token (the portal refused it, or the owner said no).
pub fn forget(state_dir: &Path) {
    let _ = std::fs::remove_file(state_dir.join(TOKEN_FILE));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-token-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_token_is_kept_and_read_back() {
        let dir = dir("keep");
        assert_eq!(read(&dir), None);
        let token = RestoreToken::new("6f1c-example-token").unwrap();
        write(&dir, &token).unwrap();
        assert_eq!(read(&dir), Some(token));
        // The next start's token replaces it, and no temporary is left.
        let next = RestoreToken::new("next-example-token").unwrap();
        write(&dir, &next).unwrap();
        assert_eq!(read(&dir), Some(next));
        assert!(!dir.join("input_capture.token.tmp").exists());
        forget(&dir);
        assert_eq!(read(&dir), None);
        forget(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn the_token_is_readable_only_by_the_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir = dir("mode");
        std::fs::create_dir_all(&dir).unwrap();
        // An old file left open to others is tightened as it is replaced.
        let path = dir.join(TOKEN_FILE);
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write(&dir, &RestoreToken::new("example-token").unwrap()).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_is_not_a_token_is_none_and_never_shown() {
        assert_eq!(RestoreToken::new(""), None);
        assert_eq!(RestoreToken::new("  \n"), None);
        assert_eq!(RestoreToken::new("a\u{0}b"), None);
        assert_eq!(RestoreToken::new(&"x".repeat(MOST + 1)), None);
        let token = RestoreToken::new(" example-token\n").unwrap();
        assert_eq!(token.as_str(), "example-token");
        assert_eq!(format!("{token:?}"), "RestoreToken(..)");
        assert_eq!(format!("{:?}", Some(token)), "Some(RestoreToken(..))");
        // A file too large to be a token is not read.
        let dir = dir("large");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(TOKEN_FILE), "x".repeat(MOST * 2)).unwrap();
        assert_eq!(read(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
