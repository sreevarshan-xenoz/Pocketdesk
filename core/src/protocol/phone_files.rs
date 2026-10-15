//! Browsing the phone's storage from the computer (T10g): only folders the
//! phone's owner granted, listed by the phone, a file fetched by the phone
//! sharing it. Both ends hold the fields to the rules here.

use serde::{Deserialize, Serialize};

use super::kdeconnect::is_invisible_format;

/// Computer → phone: `{root, path}`, list that folder.
pub const LIST: &str = "kdeconnect.pocketdesk.phone_files.list";

/// Phone → computer: the granted roots and one folder's entries.
pub const LISTING: &str = "kdeconnect.pocketdesk.phone_files.listing";

/// Computer → phone: `{root, path}`, send that file to the computer.
pub const FETCH: &str = "kdeconnect.pocketdesk.phone_files.fetch";

/// Entries one listing carries, at most.
pub const MAX_ENTRIES: usize = 2000;

/// Names a path holds below its root, at most.
pub const MAX_DEPTH: usize = 32;

/// A name, at most, in characters.
pub const MAX_NAME: usize = 255;

/// Granted roots a phone offers, at most.
pub const MAX_ROOTS: usize = 32;

/// Whether `name` is one part of a path: not empty, not `.` or `..`, no
/// separator, no control or invisible format character, not too long.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.chars().count() <= MAX_NAME
        && !name
            .chars()
            .any(|c| c == '/' || c == '\\' || c.is_control() || is_invisible_format(c))
}

/// Whether every part of `path` is a [`valid_name`], within [`MAX_DEPTH`].
pub fn valid_path(path: &[String]) -> bool {
    path.len() <= MAX_DEPTH && path.iter().all(|n| valid_name(n))
}

/// A granted root: the phone's id for it and its name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Root {
    pub id: u32,
    pub name: String,
}

/// One entry of a folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    #[serde(default)]
    pub dir: bool,
    #[serde(default)]
    pub size: u64,
    /// Milliseconds since the epoch, 0 when unknown.
    #[serde(default)]
    pub modified: u64,
}

/// What the phone answers.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Listing {
    #[serde(default)]
    pub roots: Vec<Root>,
    #[serde(default)]
    pub root: Option<u32>,
    #[serde(default)]
    pub path: Vec<String>,
    #[serde(default)]
    pub entries: Vec<Entry>,
    #[serde(default)]
    pub error: Option<String>,
}

impl Listing {
    /// A listing body as a phone sent it, held to the rules: a bad path or
    /// root name refuses it; entries with bad names are left out, and past
    /// [`MAX_ENTRIES`] cut.
    pub fn from_body(body: &serde_json::Value) -> Option<Self> {
        let mut listing: Listing = serde_json::from_value(body.clone()).ok()?;
        if listing.roots.len() > MAX_ROOTS
            || !listing.roots.iter().all(|r| valid_name(&r.name))
            || !valid_path(&listing.path)
        {
            return None;
        }
        listing.entries.retain(|e| valid_name(&e.name));
        listing.entries.truncate(MAX_ENTRIES);
        listing.error = listing
            .error
            .map(|e| e.chars().filter(|c| !c.is_control()).take(200).collect());
        Some(listing)
    }
}

/// A request's body, `{root, path}`, or `None` out of bounds.
pub fn request_body(root: u32, path: &[String]) -> Option<serde_json::Value> {
    valid_path(path).then(|| serde_json::json!({ "root": root, "path": path }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_names_and_nothing_that_climbs() {
        assert!(valid_name("DCIM") && valid_name("photo 1.jpg") && valid_name(".hidden"));
        for bad in [
            "",
            ".",
            "..",
            "a/b",
            "a\\b",
            "line\nbreak",
            "\u{202E}gpj.exe",
        ] {
            assert!(!valid_name(bad), "{bad:?}");
        }
        assert!(valid_path(&["DCIM".into(), "Camera".into()]));
        assert!(!valid_path(&["..".into()]));
        assert!(!valid_path(&vec!["a".to_string(); MAX_DEPTH + 1]));
        assert!(request_body(0, &["..".into()]).is_none());
        assert_eq!(request_body(1, &[]).unwrap()["root"], 1);
    }

    #[test]
    fn a_listing_keeps_only_good_entries_and_refuses_a_bad_path() {
        let body = serde_json::json!({
            "roots": [{ "id": 0, "name": "Download" }],
            "root": 0, "path": ["Docs"],
            "entries": [
                { "name": "a.pdf", "size": 10 },
                { "name": "../etc", "size": 1 },
                { "name": "Sub", "dir": true }
            ]
        });
        let listing = Listing::from_body(&body).unwrap();
        assert_eq!(listing.entries.len(), 2);
        assert!(listing.entries[1].dir);
        let bad = serde_json::json!({ "roots": [], "path": [".."] });
        assert!(Listing::from_body(&bad).is_none());
    }
}
