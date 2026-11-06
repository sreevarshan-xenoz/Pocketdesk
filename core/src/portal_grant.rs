//! The screen-sharing grant a GNOME or KDE portal gave, remembered (N8):
//! with `remember_screen_grant` on, the capture asks the portal to keep its
//! grant until revoked, and keeps the restore token it answers with here,
//! owner-only, so the next session starts with no one at the desk.
//!
//! Each start gives a new token, which replaces the last. A token the portal
//! refuses (revoked, or lost) means its dialog again, which with no one at
//! the desk times out and fails that session (N8 review).

use std::path::Path;

/// Where the last restore token is kept, in the state folder.
pub const TOKEN_FILE: &str = "portal_restore_token";

/// Where a RemoteDesktop session's is kept (the screen with the keyboard and
/// pointer, where input goes through the portal): apart, since a token from
/// one kind of session is refused by the other.
pub const REMOTE_TOKEN_FILE: &str = "portal_remote_token";

/// The id the desktop portal knows Pocketdesk by: its desktop file's name,
/// as the packages install it (`packaging/linux`).
pub const APP_ID: &str = "dev.pocketdesk.Pocketdesk";

/// Name this process to the desktop portal as Pocketdesk, before its first
/// portal call. Unnamed, a host program is "an unidentifiable application"
/// in KDE's dialog, its grants are kept for every unnamed program alike,
/// and KDE's pre-authorization for remote control cannot name it alone
/// (seen on Plasma 6.8, 2026-10-31). The portal names it only when the
/// desktop file is installed; otherwise, or with a portal older than its
/// host registry (1.19), it stays unnamed, as before. True when named.
#[cfg(all(target_os = "linux", feature = "capture"))]
pub async fn register_app() -> bool {
    register(None).await
}

/// [`register_app`] on a D-Bus connection of the caller's own: the portal
/// names each connection apart (the input capture keeps one of its own).
#[cfg(all(target_os = "linux", feature = "capture"))]
pub async fn register_app_on(connection: ashpd::zbus::Connection) -> bool {
    register(Some(connection)).await
}

#[cfg(all(target_os = "linux", feature = "capture"))]
async fn register(connection: Option<ashpd::zbus::Connection>) -> bool {
    // In Flatpak or Snap the portal names the program by its package, and
    // the registry is not asked (review N4: this had logged a name it never
    // gave).
    if ashpd::is_sandboxed() {
        tracing::debug!("In a sandbox: the desktop portal names this program by its package");
        return false;
    }
    let Ok(id) = ashpd::AppID::try_from(APP_ID) else {
        return false;
    };
    let call = async {
        match connection {
            Some(connection) => ashpd::register_host_app_with_connection(connection, id).await,
            None => ashpd::register_host_app(id).await,
        }
    };
    match tokio::time::timeout(std::time::Duration::from_secs(3), call).await {
        Ok(Ok(())) => {
            tracing::info!("Named to the desktop portal as {APP_ID}");
            true
        }
        Ok(Err(e)) => {
            tracing::debug!("Not named to the desktop portal ({e}): its dialogs say so");
            false
        }
        Err(_) => {
            tracing::debug!("The desktop portal did not answer the naming in 3 s");
            false
        }
    }
}

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
    read_file(&state_dir.join(TOKEN_FILE))
}

/// The kept RemoteDesktop token, if there is a usable one.
pub fn read_remote(state_dir: &Path) -> Option<String> {
    read_file(&state_dir.join(REMOTE_TOKEN_FILE))
}

fn read_file(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let token = raw.trim();
    let usable = !token.is_empty()
        && token.len() <= MAX_TOKEN
        && token.chars().all(|c| c.is_ascii_graphic());
    usable.then(|| token.to_string())
}

/// Keep `token`, owner-only, in place of the last.
pub fn write(state_dir: &Path, token: &str) -> std::io::Result<()> {
    write_file(&state_dir.join(TOKEN_FILE), token)
}

/// Keep a RemoteDesktop session's `token`, owner-only, in place of the last.
pub fn write_remote(state_dir: &Path, token: &str) -> std::io::Result<()> {
    write_file(&state_dir.join(REMOTE_TOKEN_FILE), token)
}

fn write_file(path: &Path, token: &str) -> std::io::Result<()> {
    if token.is_empty() || token.len() > MAX_TOKEN || !token.chars().all(|c| c.is_ascii_graphic()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "not a restore token",
        ));
    }
    crate::secure_fs::write_private(path, token.as_bytes())
}

/// Whether `name` may be the name of a kept token's file: plain letters,
/// digits, dashes and dots, not starting with a dot.
fn is_token_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// The kept token in the file `name` of `dir` (one grant per window lent from
/// KDE, `window_kde::grant_name`), if there is a usable one.
pub fn read_named(dir: &Path, name: &str) -> Option<String> {
    if !is_token_name(name) {
        return None;
    }
    read_file(&dir.join(name))
}

/// Keep `token`, owner-only, in the file `name` of `dir` (made if need be,
/// owner-only), in place of the last.
pub fn write_named(dir: &Path, name: &str, token: &str) -> std::io::Result<()> {
    if !is_token_name(name) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a token file name",
        ));
    }
    crate::secure_fs::create_private_dir(dir)?;
    write_file(&dir.join(name), token)
}

/// Forget the token in the file `name` of `dir`.
pub fn forget_named(dir: &Path, name: &str) -> std::io::Result<()> {
    if !is_token_name(name) {
        return Ok(());
    }
    match std::fs::remove_file(dir.join(name)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Forget every token of `dir` whose file name `keep` does not say to keep
/// (a window that is gone has no use for its grant). A folder that is not
/// there has none.
pub fn prune_named(dir: &Path, keep: impl Fn(&str) -> bool) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if is_token_name(name) && !keep(name) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Forget the kept tokens: the next session asks at the desk.
pub fn forget(state_dir: &Path) -> std::io::Result<()> {
    for file in [TOKEN_FILE, REMOTE_TOKEN_FILE] {
        match std::fs::remove_file(state_dir.join(file)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_portal_id_is_the_desktop_file_the_packages_install() {
        // The portal names the server only when a desktop file of this id
        // is installed: the .deb and the AUR package install this one.
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../packaging/linux")
            .join(format!("{APP_ID}.desktop"));
        assert!(file.is_file(), "{} is missing", file.display());
        for script in ["../packaging/linux/build-deb.sh", "../aur/PKGBUILD"] {
            let text = std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(script),
            )
            .unwrap();
            assert!(
                text.contains(&format!("/usr/share/applications/{APP_ID}.desktop")),
                "{script} does not install {APP_ID}.desktop"
            );
        }
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-grant-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_window_grant_is_kept_in_a_file_of_its_name_and_only_a_plain_name_is_one() {
        let dir = temp_dir("named").join("grants");
        assert_eq!(read_named(&dir, "kde-window-a"), None, "no folder yet");
        write_named(&dir, "kde-window-a", "token-a").unwrap();
        write_named(&dir, "kde-window-b", "token-b").unwrap();
        assert_eq!(read_named(&dir, "kde-window-a").as_deref(), Some("token-a"));
        write_named(&dir, "kde-window-a", "token-a2").unwrap();
        assert_eq!(
            read_named(&dir, "kde-window-a").as_deref(),
            Some("token-a2")
        );
        // No name that leaves the folder, hidden, empty or odd is one.
        for bad in ["", "..", "../x", "a/b", "a\\b", ".hidden", "a b", "a\n"] {
            assert!(write_named(&dir, bad, "t").is_err(), "{bad:?}");
            assert_eq!(read_named(&dir, bad), None, "{bad:?}");
            assert!(forget_named(&dir, bad).is_ok());
        }
        // The folder keeps only what is asked for.
        prune_named(&dir, |name| name == "kde-window-b");
        assert_eq!(read_named(&dir, "kde-window-a"), None);
        assert_eq!(read_named(&dir, "kde-window-b").as_deref(), Some("token-b"));
        forget_named(&dir, "kde-window-b").unwrap();
        forget_named(&dir, "kde-window-b").unwrap();
        assert_eq!(read_named(&dir, "kde-window-b"), None);
        prune_named(&dir.join("not-there"), |_| false);
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
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

    /// A RemoteDesktop session's grant is kept apart from a screen's (a token
    /// from one is refused by the other), and Forget forgets both.
    #[test]
    fn a_remote_desktop_grant_is_kept_apart_and_forgotten_with_the_screen_s() {
        let dir = temp_dir("remote");
        write(&dir, "screen").unwrap();
        write_remote(&dir, "remote").unwrap();
        assert_eq!(read(&dir).as_deref(), Some("screen"));
        assert_eq!(read_remote(&dir).as_deref(), Some("remote"));
        forget(&dir).unwrap();
        assert_eq!(read(&dir), None);
        assert_eq!(read_remote(&dir), None);
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
