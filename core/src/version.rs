//! The Pocketdesk version each paired device runs, and how it compares with
//! this one's.
//!
//! The owner's devices update by different roads (AUR or a .deb, winget or
//! setup.exe, F-Droid or Obtainium), and most features need both ends. Each
//! end says its version in its identity (`pocketdeskVersion`, beside
//! `deviceName`), the other keeps it, and both apps show it: "runs 0.8.1,
//! older than this one: update it". A device that says no version (one
//! from before this, or KDE Connect itself) shows nothing. Nothing here
//! asks the internet what the newest release is.
//!
//! A version is the other device's claim, so it is cleaned like a name, and
//! compared on its release numbers only: a `-dev` or `+<git hash>` suffix
//! is not a different release, so a build from a checkout never nags.

use std::cmp::Ordering;

/// This build's version: the workspace's, which every crate shares, the
/// Android app's `versionName` included.
pub const THIS_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The identity packet's field for it.
pub const IDENTITY_FIELD: &str = "pocketdeskVersion";

/// The longest version kept, in characters: room for
/// `0.12.3-rc.1+g1a2b3c4d`, short enough for a row.
pub const MAX_VERSION_CHARS: usize = 32;

/// A version as said by another device, cleaned: trimmed, cut at the first
/// space, letters, digits and `.+-_` only, at most [`MAX_VERSION_CHARS`],
/// and starting with a digit (after an optional `v`). `None` when nothing
/// usable is left.
pub fn clean_version(raw: &str) -> Option<String> {
    let word = raw.split_whitespace().next()?;
    if !word
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-' | '_'))
    {
        return None;
    }
    let cleaned: String = word.chars().take(MAX_VERSION_CHARS).collect();
    release(&cleaned).map(|_| cleaned)
}

/// The release numbers of `version` (`major.minor.patch`; a missing minor or
/// patch is 0), before any `-pre` or `+build` suffix; a leading `v` is
/// allowed. `None` when it does not start with a number.
pub fn release(version: &str) -> Option<(u64, u64, u64)> {
    let version = version.trim();
    let version = version
        .strip_prefix('v')
        .or_else(|| version.strip_prefix('V'))
        .unwrap_or(version);
    let numbers = version
        .split(['-', '+', '_', ' '])
        .next()
        .filter(|n| !n.is_empty())?;
    let mut parts = numbers.split('.');
    let mut next = |required: bool| -> Option<u64> {
        match parts.next() {
            None if !required => Some(0),
            None => None,
            Some(part) => {
                if part.is_empty() || part.len() > 9 || !part.bytes().all(|b| b.is_ascii_digit()) {
                    None
                } else {
                    part.parse().ok()
                }
            }
        }
    };
    let major = next(true)?;
    let minor = next(false)?;
    let patch = next(false)?;
    // `1.2.3.4` is not a version this project makes.
    parts.next().is_none().then_some((major, minor, patch))
}

/// How another device's version stands to this one's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skew {
    /// The same release (suffixes aside).
    Same,
    /// The other device runs an older release: update it.
    PeerOlder,
    /// The other device runs a newer release: update this one.
    PeerNewer,
}

/// How `peer` stands to `this`; `None` when either is not a version.
pub fn skew(this: &str, peer: &str) -> Option<Skew> {
    Some(match release(peer)?.cmp(&release(this)?) {
        Ordering::Less => Skew::PeerOlder,
        Ordering::Equal => Skew::Same,
        Ordering::Greater => Skew::PeerNewer,
    })
}

/// A device's version in a row's words: "runs 0.8.1", and with `nudge`
/// (the "Tell me when my devices run different versions" setting) what to
/// update when the releases differ. Never an error: a device on another
/// release still works, with the features both have.
pub fn words(this: &str, peer: &str, nudge: bool) -> String {
    match (nudge, skew(this, peer)) {
        (true, Some(Skew::PeerOlder)) => {
            format!("runs {peer}, older than this one: update it")
        }
        (true, Some(Skew::PeerNewer)) => {
            format!("runs {peer}, newer than this one: update this one")
        }
        _ => format!("runs {peer}"),
    }
}

/// Whether [`words`] asks for an update, to draw it as a hint.
pub fn differs(this: &str, peer: &str) -> bool {
    matches!(skew(this, peer), Some(Skew::PeerOlder | Skew::PeerNewer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_numbers_ignore_suffixes() {
        assert_eq!(release("0.8.2"), Some((0, 8, 2)));
        assert_eq!(release("v0.8.2"), Some((0, 8, 2)));
        assert_eq!(release("0.8.2-dev"), Some((0, 8, 2)));
        assert_eq!(release("0.8.2+g1a2b3c4"), Some((0, 8, 2)));
        assert_eq!(release("0.8.2-5-g1a2b3c4"), Some((0, 8, 2)));
        assert_eq!(release("0.9"), Some((0, 9, 0)));
        assert_eq!(release("1"), Some((1, 0, 0)));
        assert_eq!(release("0.10.0"), Some((0, 10, 0)));
    }

    #[test]
    fn what_is_not_a_version_has_no_release() {
        for bad in [
            "",
            "dev",
            "-1.0",
            "1..2",
            "1.2.3.4",
            "1.x",
            "99999999999.0.0",
            "v",
        ] {
            assert_eq!(release(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn releases_compare_by_number_not_by_text() {
        assert_eq!(skew("0.8.2", "0.8.1"), Some(Skew::PeerOlder));
        assert_eq!(skew("0.8.2", "0.10.0"), Some(Skew::PeerNewer));
        assert_eq!(skew("0.10.0", "0.9.9"), Some(Skew::PeerOlder));
        assert_eq!(skew("1.0.0", "0.99.0"), Some(Skew::PeerOlder));
        assert_eq!(skew("0.8.2", "0.8.2"), Some(Skew::Same));
        assert_eq!(skew("0.8.2", "garbage"), None);
        assert_eq!(skew("garbage", "0.8.2"), None);
    }

    /// A build from a checkout (`-dev`, a git hash) is the release it
    /// names: no nag between it and the published one.
    #[test]
    fn a_dev_build_is_its_release() {
        assert_eq!(skew("0.8.2", "0.8.2-dev"), Some(Skew::Same));
        assert_eq!(skew("0.8.2-dev", "0.8.2+g1a2b3c4"), Some(Skew::Same));
        assert_eq!(skew("0.8.3-dev", "0.8.2"), Some(Skew::PeerOlder));
    }

    #[test]
    fn a_claimed_version_is_cleaned() {
        assert_eq!(clean_version(" 0.8.1 ").as_deref(), Some("0.8.1"));
        assert_eq!(
            clean_version("0.8.1 (built by hand)").as_deref(),
            Some("0.8.1")
        );
        assert_eq!(
            clean_version("0.8.2-dev+g1a2b3c4").as_deref(),
            Some("0.8.2-dev+g1a2b3c4")
        );
        assert_eq!(clean_version(""), None);
        assert_eq!(clean_version("   "), None);
        assert_eq!(clean_version("latest"), None);
        assert_eq!(clean_version("0.8.1\u{202E}"), None, "no bidi tricks");
        assert_eq!(clean_version("0.8.1<b>"), None);
        let long = format!("0.8.1-{}", "a".repeat(80));
        assert_eq!(
            clean_version(&long).map(|v| v.len()),
            Some(MAX_VERSION_CHARS)
        );
    }

    #[test]
    fn the_words_say_what_to_update() {
        assert_eq!(
            words("0.8.2", "0.8.1", true),
            "runs 0.8.1, older than this one: update it"
        );
        assert_eq!(
            words("0.8.2", "0.9.0", true),
            "runs 0.9.0, newer than this one: update this one"
        );
        assert_eq!(words("0.8.2", "0.8.2-dev", true), "runs 0.8.2-dev");
        assert_eq!(words("0.8.2", "0.8.1", false), "runs 0.8.1", "told not to");
        assert!(differs("0.8.2", "0.8.1"));
        assert!(!differs("0.8.2", "0.8.2+g1a2b3c4"));
        assert!(!differs("0.8.2", "nonsense"));
    }

    #[test]
    fn this_build_has_a_version() {
        assert!(release(THIS_VERSION).is_some(), "{THIS_VERSION}");
        assert_eq!(clean_version(THIS_VERSION).as_deref(), Some(THIS_VERSION));
    }
}
