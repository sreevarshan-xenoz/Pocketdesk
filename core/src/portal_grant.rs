//! The screen-sharing grant a GNOME or KDE portal gave, remembered (N8):
//! with `remember_screen_grant` on, the capture asks the portal to keep its
//! grant until revoked, and keeps the restore token it answers with here,
//! owner-only, so the next session starts with no one at the desk.
//!
//! Each start gives a new token, which replaces the last. A token the portal
//! refuses (revoked, or lost) only means a dialog again.

use std::path::Path;

/// Where the last restore token is kept, in the state folder.
pub const TOKEN_FILE: &str = "portal_restore_token";

/// The longest token kept; the portal's are UUIDs.
const MAX_TOKEN: usize = 256;

/// What to ask the portal for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Persist {
    /// Ask at the desk each time (the setting is off).
    No,
    /// Keep the grant until the owner revokes it.
    UntilRevoked,
}

/// What the capture asks the portal: whether to keep the grant, and the
/// token to restore one with, if there is one to use.
pub fn request(remember: bool, saved: Option<&str>) -> (Persist, Option<&str>) {
    if remember {
        (Persist::UntilRevoked, saved)
    } else {
        (Persist::No, None)
    }
}

/// The kept token, if there is a usable one.
pub fn read(state_dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(state_dir.join(TOKEN_FILE)).ok()?;
    let token = raw.trim();
    let usable = !token.is_empty()
        && token.len() <= MAX_TOKEN
        && token.chars().all(|c| c.is_ascii_graphic());
    usable.then(|| token.to_string())
}

/// Keep `token`, owner-only, in place of the last.
pub fn write(state_dir: &Path, token: &str) -> std::io::Result<()> {
    if token.is_empty() || token.len() > MAX_TOKEN || !token.chars().all(|c| c.is_ascii_graphic()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "not a restore token",
        ));
    }
    crate::secure_fs::write_private(&state_dir.join(TOKEN_FILE), token.as_bytes())
}

/// Forget the kept token: the next session asks at the desk.
pub fn forget(state_dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(state_dir.join(TOKEN_FILE)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-grant-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_token_is_kept_read_back_replaced_and_forgotten() {
        let dir = temp_dir("kept");
        assert_eq!(read(&dir), None, "none yet");
        write(&dir, "2f1c7b3e-0d5a-4b8e-9c61-5a0e7d2f9b11").unwrap();
        assert_eq!(
            read(&dir).as_deref(),
            Some("2f1c7b3e-0d5a-4b8e-9c61-5a0e7d2f9b11")
        );
        write(&dir, "second").unwrap();
        assert_eq!(
            read(&dir).as_deref(),
            Some("second"),
            "each start replaces it"
        );
        forget(&dir).unwrap();
        assert_eq!(read(&dir), None);
        forget(&dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unusable_token_reads_as_none_and_is_not_written() {
        let dir = temp_dir("unusable");
        std::fs::write(dir.join(TOKEN_FILE), "  \n").unwrap();
        assert_eq!(read(&dir), None, "blank");
        std::fs::write(dir.join(TOKEN_FILE), "a b").unwrap();
        assert_eq!(read(&dir), None, "a space inside");
        std::fs::write(dir.join(TOKEN_FILE), "x".repeat(MAX_TOKEN + 1)).unwrap();
        assert_eq!(read(&dir), None, "too long");
        assert!(write(&dir, "").is_err());
        assert!(write(&dir, "line\nbreak").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_portal_is_asked_to_keep_the_grant_only_when_the_owner_chose_it() {
        assert_eq!(request(false, Some("t")), (Persist::No, None));
        assert_eq!(request(true, Some("t")), (Persist::UntilRevoked, Some("t")));
        assert_eq!(request(true, None), (Persist::UntilRevoked, None));
    }
}
