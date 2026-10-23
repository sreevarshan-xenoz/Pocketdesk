//! Permission profiles (P9): a phone's permissions saved under a name on
//! the Devices page, applied to another phone there, or offered at pairing
//! beside "Mine", "Someone else's" and a guest's. Kept owner-only in
//! `<state>/permission_profiles.json`; the desktop app writes it, the
//! server reads it when a code that names a profile pairs.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::paths::PERMISSION_PROFILES_FILE;
use crate::protocol::permissions::DevicePermissions;

/// The most profiles kept.
pub const MAX_PROFILES: usize = 30;

/// The longest name, in characters.
pub const MAX_NAME_CHARS: usize = 40;

/// One profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub permissions: DevicePermissions,
}

#[derive(Default, Serialize, Deserialize)]
struct ProfilesFile {
    profiles: Vec<Profile>,
}

/// A name as it is matched: trimmed and lowercased.
fn key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// A name a profile may have: printable, not blank, at most
/// [`MAX_NAME_CHARS`]; trimmed.
pub fn valid_name(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty()
        && name.chars().count() <= MAX_NAME_CHARS
        && !name.chars().any(|c| c.is_control()))
    .then(|| name.to_string())
}

/// The profiles, in the order saved. A missing file is none; one that does
/// not parse is an error, so a save never writes over it.
pub fn load(state_dir: &Path) -> io::Result<Vec<Profile>> {
    match std::fs::read_to_string(state_dir.join(PERMISSION_PROFILES_FILE)) {
        Ok(raw) => serde_json::from_str::<ProfilesFile>(&raw)
            .map(|file| file.profiles)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// The profiles to show, none when the file cannot be read.
pub fn read(state_dir: &Path) -> Vec<Profile> {
    load(state_dir).unwrap_or_default()
}

/// The profile called `name`, if any.
pub fn get(state_dir: &Path, name: &str) -> Option<Profile> {
    let wanted = key(name);
    read(state_dir).into_iter().find(|p| key(&p.name) == wanted)
}

fn write(state_dir: &Path, profiles: Vec<Profile>) -> io::Result<()> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let json = serde_json::to_string(&ProfilesFile { profiles }).map_err(io::Error::other)?;
    let path = state_dir.join(PERMISSION_PROFILES_FILE);
    let tmp = path.with_extension(format!(
        "json.{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(state_dir)?;
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn change(
    state_dir: &Path,
    edit: impl FnOnce(&mut Vec<Profile>) -> io::Result<()>,
) -> io::Result<()> {
    let _lock =
        crate::protocol::kdeconnect::StoreLock::acquire(&state_dir.join(PERMISSION_PROFILES_FILE))
            .map_err(|e| io::Error::other(e.to_string()))?;
    let mut profiles = load(state_dir)?;
    edit(&mut profiles)?;
    write(state_dir, profiles)
}

/// Save `permissions` as `name`, replacing a profile of that name.
pub fn save(state_dir: &Path, name: &str, permissions: &DevicePermissions) -> io::Result<String> {
    let name = valid_name(name).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("a name of 1 to {MAX_NAME_CHARS} characters"),
        )
    })?;
    let saved = name.clone();
    change(state_dir, |profiles| {
        let wanted = key(&name);
        if let Some(profile) = profiles.iter_mut().find(|p| key(&p.name) == wanted) {
            profile.permissions = permissions.clone();
            return Ok(());
        }
        if profiles.len() >= MAX_PROFILES {
            return Err(io::Error::other(format!(
                "{MAX_PROFILES} profiles are kept already; remove one first"
            )));
        }
        profiles.push(Profile {
            name,
            permissions: permissions.clone(),
        });
        Ok(())
    })?;
    Ok(saved)
}

/// Remove the profile called `name`; one not there changes nothing.
pub fn remove(state_dir: &Path, name: &str) -> io::Result<()> {
    let gone = key(name);
    change(state_dir, |profiles| {
        profiles.retain(|p| key(&p.name) != gone);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::permissions::Capability;

    fn dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-profiles-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_profile_is_saved_found_by_name_replaced_and_removed() {
        let dir = dir("save");
        assert!(read(&dir).is_empty());
        let mut kids = DevicePermissions::someone_else();
        kids.set(Capability::Audio, true);
        assert_eq!(save(&dir, "  Kids ", &kids).unwrap(), "Kids");
        assert_eq!(get(&dir, "kids").map(|p| p.permissions), Some(kids.clone()));
        let mut less = kids.clone();
        less.set(Capability::Audio, false);
        save(&dir, "KIDS", &less).unwrap();
        assert_eq!(read(&dir).len(), 1, "one name, replaced");
        assert_eq!(get(&dir, "Kids").unwrap().permissions, less);
        remove(&dir, "kids").unwrap();
        assert!(get(&dir, "Kids").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_are_printable_and_bounded() {
        assert_eq!(valid_name(" Work phone "), Some("Work phone".to_string()));
        assert_eq!(valid_name(""), None);
        assert_eq!(valid_name("a\nb"), None);
        assert_eq!(valid_name(&"x".repeat(MAX_NAME_CHARS + 1)), None);
    }

    #[test]
    fn a_broken_file_is_never_written_over() {
        let dir = dir("broken");
        std::fs::write(dir.join(PERMISSION_PROFILES_FILE), b"{not json").unwrap();
        assert!(save(&dir, "Kids", &DevicePermissions::default()).is_err());
        assert_eq!(
            std::fs::read(dir.join(PERMISSION_PROFILES_FILE)).unwrap(),
            b"{not json"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
