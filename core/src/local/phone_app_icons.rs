//! The icons of the phones' apps (Apps part 1), kept in
//! `<state>/phone_app_icons/<phone>/app-<id>@<version>.png`, owner-only.
//! An icon is named by the app's id and its version code, so a phone sends
//! each once: the computer tells the phone which it has (`have`), and an
//! update of the app, with a new version, brings a new one.
//!
//! A file's name carries no more than a package name's characters (checked
//! by [`valid_app_id`]) and a number, and its content is checked by its
//! header before anything decodes it.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use super::notification_icons::{icon_fits, prune, write};
use super::paths::PHONE_APP_ICONS_DIR;
use crate::protocol::phone_action::{MAX_APPS, valid_app_id};

/// Icons kept for one phone: its apps' (at most [`MAX_APPS`]) and a few
/// more for the old versions waiting to be pruned.
const KEEP: usize = MAX_APPS + 20;

/// A name for `phone`, the same each time, safe as a folder name.
fn phone_dir(state_dir: &Path, phone: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(phone.as_bytes());
    let name: String = hash[..12].iter().map(|b| format!("{b:02x}")).collect();
    state_dir.join(PHONE_APP_ICONS_DIR).join(name)
}

/// Where `phone`'s icon for `app` at `version` is kept.
pub fn icon_path(state_dir: &Path, phone: &str, app: &str, version: u64) -> PathBuf {
    phone_dir(state_dir, phone).join(format!("app-{app}@{version}.png"))
}

/// The id and version an icon's file name says, if it is one of ours.
fn parse_name(name: &str) -> Option<(String, u64)> {
    let rest = name.strip_prefix("app-")?.strip_suffix(".png")?;
    let (id, version) = rest.rsplit_once('@')?;
    valid_app_id(id).then_some(())?;
    Some((id.to_string(), version.parse().ok()?))
}

/// The icons kept for `phone`: each app's newest version, which an `apps`
/// ask carries as `have`.
pub fn have(state_dir: &Path, phone: &str) -> BTreeMap<String, u64> {
    let mut have: BTreeMap<String, u64> = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(phone_dir(state_dir, phone)) else {
        return have;
    };
    for entry in entries.flatten() {
        let Some((id, version)) = entry.file_name().to_str().and_then(parse_name) else {
            continue;
        };
        let newest = have.entry(id).or_insert(version);
        *newest = (*newest).max(version);
    }
    have
}

/// Keep `bytes` as `phone`'s icon for `app` at `version`, if `app` is a
/// package name and the icon a small PNG; any other version of it is
/// dropped. The path kept, or why not.
pub fn keep(
    state_dir: &Path,
    phone: &str,
    app: &str,
    version: u64,
    bytes: &[u8],
) -> io::Result<PathBuf> {
    if !valid_app_id(app) || !icon_fits(bytes) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not an icon"));
    }
    let path = icon_path(state_dir, phone, app, version);
    write(&path, bytes)?;
    if let Some(dir) = path.parent() {
        drop_other_versions(dir, app, version);
        prune(dir, "app-", "png", KEEP);
    }
    Ok(path)
}

/// Remove `app`'s icons but the one at `version`.
fn drop_other_versions(dir: &Path, app: &str, version: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if let Some((id, v)) = entry.file_name().to_str().and_then(parse_name)
            && id == app
            && v != version
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Remove `phone`'s icons of apps its list no longer holds, and of old
/// versions: `listed` is the list (each app's version).
pub fn forget_unlisted(state_dir: &Path, phone: &str, listed: &BTreeMap<String, u64>) {
    let Ok(entries) = std::fs::read_dir(phone_dir(state_dir, phone)) else {
        return;
    };
    for entry in entries.flatten() {
        let Some((id, version)) = entry.file_name().to_str().and_then(parse_name) else {
            continue;
        };
        // An app listed with no version (0, an old phone) keeps what it sent.
        let current = listed.get(&id).is_some_and(|v| *v == version || *v == 0);
        if !current {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The icon kept for `app`, at the version the list says, or the newest
/// kept when it says none.
pub fn kept(state_dir: &Path, phone: &str, app: &str, version: u64) -> Option<PathBuf> {
    let exact = icon_path(state_dir, phone, app, version);
    if exact.is_file() {
        return Some(exact);
    }
    let newest = *have(state_dir, phone).get(app)?;
    let path = icon_path(state_dir, phone, app, newest);
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PNG header declaring `side` pixels each way (enough to pass `icon_fits`).
    fn png(side: u32) -> Vec<u8> {
        let mut bytes = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
        bytes.extend_from_slice(&[0, 0, 0, 13]);
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&side.to_be_bytes());
        bytes.extend_from_slice(&side.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes
    }

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-app-icons-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// An icon is kept once for its version, and `have` says which.
    #[test]
    fn an_icon_is_kept_once_for_its_version() {
        let dir = dir("once");
        assert!(have(&dir, "phone-a").is_empty());
        keep(&dir, "phone-a", "org.example.app", 3, &png(64)).unwrap();
        keep(&dir, "phone-a", "org.example.other", 9, &png(64)).unwrap();
        keep(&dir, "phone-b", "org.example.app", 1, &png(64)).unwrap();
        assert_eq!(
            have(&dir, "phone-a"),
            BTreeMap::from([
                ("org.example.app".to_string(), 3),
                ("org.example.other".to_string(), 9)
            ])
        );
        assert_eq!(have(&dir, "phone-b").len(), 1, "each phone has its own");
        // An update replaces the old version's file.
        keep(&dir, "phone-a", "org.example.app", 4, &png(64)).unwrap();
        assert_eq!(have(&dir, "phone-a")["org.example.app"], 4);
        assert!(!icon_path(&dir, "phone-a", "org.example.app", 3).exists());
        assert!(kept(&dir, "phone-a", "org.example.app", 4).is_some());
        assert!(kept(&dir, "phone-a", "org.example.gone", 1).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Only a small PNG of a real package name is kept.
    #[test]
    fn only_a_small_png_of_a_package_is_kept() {
        let dir = dir("checked");
        assert!(keep(&dir, "p", "org.example.app", 1, b"not a png").is_err());
        assert!(keep(&dir, "p", "org.example.app", 1, &png(5000)).is_err());
        assert!(keep(&dir, "p", "../../etc", 1, &png(64)).is_err());
        assert!(keep(&dir, "p", "noDots", 1, &png(64)).is_err());
        assert!(have(&dir, "p").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An app gone from the list, or updated past its icon, loses the icon.
    #[test]
    fn icons_of_apps_gone_are_forgotten() {
        let dir = dir("forget");
        keep(&dir, "p", "org.example.a", 1, &png(64)).unwrap();
        keep(&dir, "p", "org.example.b", 1, &png(64)).unwrap();
        keep(&dir, "p", "org.example.c", 1, &png(64)).unwrap();
        let listed = BTreeMap::from([
            ("org.example.a".to_string(), 1),
            ("org.example.b".to_string(), 2),
        ]);
        forget_unlisted(&dir, "p", &listed);
        assert_eq!(
            have(&dir, "p"),
            BTreeMap::from([("org.example.a".to_string(), 1)])
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_name_is_read_back_exactly() {
        assert_eq!(
            parse_name("app-org.example.app@12.png"),
            Some(("org.example.app".to_string(), 12))
        );
        assert_eq!(parse_name("app-org.example.app@x.png"), None);
        assert_eq!(parse_name("org.example.app@1.png"), None);
        assert_eq!(parse_name("app-bad id@1.png"), None);
    }
}
