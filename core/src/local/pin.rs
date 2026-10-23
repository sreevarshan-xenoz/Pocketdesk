//! The pairing PIN file, `<state>/pairing.pin`.
//!
//! `pocketdesk pair` and the desktop app write it; the server's pair plugin
//! accepts a phone that sends the PIN while the file is fresh. Format, one
//! value per line: the six digits, the unix second it was minted, and an
//! optional grant length in seconds (R4 D3: pairing with this PIN trusts the
//! phone only for that long), and the word `someone-else` when the phone is
//! not the owner's (M2b: it pairs with files and links only). The lines after
//! the time are read in any order. A file with no timestamp line holds no
//! PIN: it would never expire.

use std::io;
use std::path::Path;
use std::time::Duration;

use super::paths::PIN_FILE;

/// How long a PIN stays valid after it is minted.
pub const PIN_TTL: Duration = Duration::from_secs(300);

/// A timestamp further ahead of the clock than this is not clock skew, and
/// would otherwise make a PIN that never expires.
const MAX_FUTURE_SECS: u64 = 60;

/// The PIN file's contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePin {
    pub pin: String,
    /// When it was minted.
    pub minted_unix: u64,
    /// How long pairing with it trusts the phone; `None` is permanent.
    pub grant_secs: Option<u64>,
    /// M2b: the phone is someone else's, so it pairs with files and links only.
    pub someone_else: bool,
    /// P9: the profile whose permissions it pairs with, as someone else's
    /// phone does otherwise.
    pub profile: Option<String>,
}

/// The PIN file's word for a phone that is someone else's (M2b).
const SOMEONE_ELSE: &str = "someone-else";

/// The PIN file's word before a profile's name (P9).
const PROFILE: &str = "profile ";

/// Exactly six ASCII digits.
pub fn is_valid(pin: &str) -> bool {
    pin.len() == 6 && pin.bytes().all(|b| b.is_ascii_digit())
}

/// A six-digit PIN from the OS CSPRNG, uniform over 000000-999999.
pub fn mint_value() -> String {
    use rand::Rng;
    format!("{:06}", rand::rngs::OsRng.gen_range(0..1_000_000u32))
}

/// Write `pin` to `<state_dir>/pairing.pin`, owner-only: anyone who can read
/// the file can pair a phone while it is fresh.
pub fn write(
    state_dir: &Path,
    pin: &str,
    now_unix: u64,
    grant_secs: Option<u64>,
) -> io::Result<()> {
    write_preset(state_dir, pin, now_unix, grant_secs, false, None)
}

/// [`write`], saying whether the phone it pairs is someone else's (M2b),
/// and with which profile's permissions (P9), which makes it someone else's.
pub fn write_preset(
    state_dir: &Path,
    pin: &str,
    now_unix: u64,
    grant_secs: Option<u64>,
    someone_else: bool,
    profile: Option<&str>,
) -> io::Result<()> {
    if !is_valid(pin) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "PIN must be exactly 6 numeric digits",
        ));
    }
    std::fs::create_dir_all(state_dir)?;
    let grant_line = grant_secs.map(|s| format!("{s}\n")).unwrap_or_default();
    let profile = profile.and_then(super::permission_profiles::valid_name);
    let preset_line = if someone_else || profile.is_some() {
        format!("{SOMEONE_ELSE}\n")
    } else {
        String::new()
    };
    let profile_line = profile
        .map(|name| format!("{PROFILE}{name}\n"))
        .unwrap_or_default();
    crate::secure_fs::write_private(
        &state_dir.join(PIN_FILE),
        format!("{pin}\n{now_unix}\n{grant_line}{preset_line}{profile_line}").as_bytes(),
    )
}

/// The PIN in `<state_dir>/pairing.pin` if there is one and it is still
/// valid at `now_unix`.
pub fn read(state_dir: &Path, now_unix: u64) -> Option<FilePin> {
    let raw = std::fs::read_to_string(state_dir.join(PIN_FILE)).ok()?;
    let mut lines = raw.lines().map(str::trim);
    let pin = lines.next()?.to_string();
    if !is_valid(&pin) {
        return None;
    }
    // No minting time is no PIN: it would never expire (an old CLI's file,
    // or one planted in the state folder).
    let minted = lines.next().and_then(|s| s.parse::<u64>().ok())?;
    // After the time, in any order: a grant's seconds, and the preset.
    let rest: Vec<&str> = lines.collect();
    let grant_secs = rest.iter().find_map(|s| s.parse::<u64>().ok());
    let profile = rest
        .iter()
        .find_map(|s| s.strip_prefix(PROFILE.trim_end()))
        .and_then(super::permission_profiles::valid_name);
    let someone_else = rest.contains(&SOMEONE_ELSE) || profile.is_some();
    if minted > now_unix.saturating_add(MAX_FUTURE_SECS) {
        return None;
    }
    if now_unix.saturating_sub(minted) >= PIN_TTL.as_secs() {
        return None;
    }
    Some(FilePin {
        pin,
        minted_unix: minted,
        grant_secs,
        someone_else,
        profile,
    })
}

/// Delete the PIN file if it still holds `pin` (a newer PIN written since is
/// left alone). Returns whether a file was removed. Used to make a PIN
/// one-shot once it has paired a phone, to discard it after too many wrong
/// guesses, and when the desktop app stops showing it.
pub fn remove_if(state_dir: &Path, pin: &str) -> io::Result<bool> {
    let path = state_dir.join(PIN_FILE);
    let held = match std::fs::read_to_string(&path) {
        Ok(raw) => raw.lines().next().map(str::trim) == Some(pin),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    if held {
        std::fs::remove_file(&path)?;
    }
    Ok(held)
}

/// How much longer `pin` is valid at `now_unix`.
pub fn remaining(pin: &FilePin, now_unix: u64) -> Duration {
    let age = now_unix.saturating_sub(pin.minted_unix);
    Duration::from_secs(PIN_TTL.as_secs().saturating_sub(age))
}

/// Seconds since the Unix epoch.
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-pin-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_written_pin_reads_back_until_its_ttl() {
        let dir = temp_dir("ttl");
        write(&dir, "123456", 1_000, None).unwrap();
        let read_back = read(&dir, 1_100).unwrap();
        assert_eq!(read_back.pin, "123456");
        assert_eq!(remaining(&read_back, 1_100), Duration::from_secs(200));
        assert!(
            read(&dir, 1_000 + 300).is_none(),
            "expired at exactly the TTL"
        );
    }

    #[test]
    fn the_grant_line_survives_and_a_pin_without_a_stamp_is_no_pin() {
        let dir = temp_dir("grant");
        write(&dir, "000042", 50, Some(3600)).unwrap();
        assert_eq!(read(&dir, 60).unwrap().grant_secs, Some(3600));
        // A file with no minting time (an old CLI's, or one planted in the
        // state folder) would otherwise be a PIN that never expires.
        std::fs::write(dir.join(super::super::paths::PIN_FILE), "654321\n").unwrap();
        assert!(read(&dir, 60).is_none());
    }

    #[test]
    fn a_stamp_from_the_future_is_not_a_pin_that_never_expires() {
        let dir = temp_dir("future");
        write(&dir, "123456", 10_000, None).unwrap();
        assert!(read(&dir, 9_990).is_some(), "small clock skew is fine");
        assert!(read(&dir, 9_000).is_none(), "far-future stamps are refused");
    }

    #[test]
    fn a_pin_is_removed_only_while_it_is_still_the_one_on_file() {
        let dir = temp_dir("remove");
        write(&dir, "123456", 10, None).unwrap();
        assert!(
            !remove_if(&dir, "654321").unwrap(),
            "a different PIN is left alone"
        );
        assert!(read(&dir, 11).is_some());
        assert!(remove_if(&dir, "123456").unwrap());
        assert!(read(&dir, 11).is_none());
        assert!(
            !remove_if(&dir, "123456").unwrap(),
            "nothing left to remove"
        );
    }

    #[test]
    fn malformed_files_are_no_pin() {
        let dir = temp_dir("bad");
        let path = dir.join(super::super::paths::PIN_FILE);
        for raw in ["12345\n1\n", "abcdef\n1\n", "", "1234567\n"] {
            std::fs::write(&path, raw).unwrap();
            assert!(read(&dir, 1).is_none(), "{raw:?}");
        }
    }

    #[test]
    fn minted_pins_are_six_digits_and_do_not_follow_the_clock() {
        let pins: Vec<u32> = (0..256)
            .map(|_| {
                let pin = mint_value();
                assert!(is_valid(&pin), "{pin}");
                pin.parse().unwrap()
            })
            .collect();
        // Minted back to back, clock-derived PINs mostly rise; random ones
        // rise about half the time.
        let rising = pins.windows(2).filter(|w| w[1] > w[0]).count();
        assert!((64..192).contains(&rising), "{rising} of 255 steps rose");
        let mut distinct = pins.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert!(distinct.len() > 240, "only {} distinct", distinct.len());
    }

    #[test]
    fn someone_elses_phone_is_said_with_or_without_a_grant() {
        let dir =
            std::env::temp_dir().join(format!("pocketdesk-pin-preset-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_preset(&dir, "123456", 1_000, None, true, None).unwrap();
        let read_back = read(&dir, 1_010).unwrap();
        assert!(read_back.someone_else);
        assert_eq!(read_back.grant_secs, None);
        write_preset(&dir, "123456", 1_000, Some(900), true, None).unwrap();
        let read_back = read(&dir, 1_010).unwrap();
        assert!(read_back.someone_else);
        assert_eq!(read_back.grant_secs, Some(900));
        write(&dir, "123456", 1_000, Some(900)).unwrap();
        assert!(!read(&dir, 1_010).unwrap().someone_else);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// P9: a profile's name rides the file, and makes the phone someone
    /// else's.
    #[test]
    fn a_profile_rides_the_pin_file() {
        let dir = temp_dir("profile");
        write_preset(&dir, "123456", 1_000, None, false, Some(" Kids ")).unwrap();
        let read = read(&dir, 1_000).unwrap();
        assert_eq!(read.profile.as_deref(), Some("Kids"));
        assert!(read.someone_else);
        write_preset(&dir, "123456", 1_000, None, false, Some("bad\nname")).unwrap();
        assert_eq!(read_profile(&dir), None, "a name that is no name is none");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn read_profile(dir: &std::path::Path) -> Option<String> {
        read(dir, 1_000).and_then(|p| p.profile)
    }
}
