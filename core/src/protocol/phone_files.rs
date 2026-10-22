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

/// Computer → phone: `{root, path}`, make that folder in a folder the
/// phone lets the computer add to (N6); answered with its listing.
pub const MKDIR: &str = "kdeconnect.pocketdesk.phone_files.mkdir";

/// Computer → phone: `{root, path, name, id, size}`, fetch the computer's
/// offer `id` into that folder as `name` (N6).
pub const PUT: &str = "kdeconnect.pocketdesk.phone_files.put";

/// The largest file the computer's drive takes for a phone (N6).
pub const MAX_PUT_BYTES: u64 = 4 << 30;

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
    /// The phone's owner lets the computer add files and folders here (N6).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub add: bool,
}

/// Whether `name` may be added to a phone's folder (N6): a [`valid_name`]
/// that is not hidden, so nothing lands where the listing never shows it.
pub fn addable_name(name: &str) -> bool {
    valid_name(name) && !name.starts_with('.')
}

/// Whether `id` names an offer as the computer makes one: 32 lowercase hex
/// digits.
pub fn valid_offer_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A [`MKDIR`] body, or `None` out of bounds: a folder below the root,
/// every name addable.
pub fn mkdir_body(root: u32, path: &[String]) -> Option<serde_json::Value> {
    (!path.is_empty() && path.iter().all(|n| addable_name(n)))
        .then(|| request_body(root, path))
        .flatten()
}

/// A [`PUT`] body, or `None` out of bounds: `folder` below the root (every
/// name addable), an addable `name`, an offer id and a size within
/// [`MAX_PUT_BYTES`].
pub fn put_body(
    root: u32,
    folder: &[String],
    name: &str,
    id: &str,
    size: u64,
) -> Option<serde_json::Value> {
    let fits = folder.len() < MAX_DEPTH
        && folder.iter().all(|n| addable_name(n))
        && addable_name(name)
        && valid_offer_id(id)
        && size <= MAX_PUT_BYTES;
    fits.then(|| {
        serde_json::json!({ "root": root, "path": folder, "name": name, "id": id, "size": size })
    })
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
    /// Whether the folder holds more than the listing carries.
    #[serde(default)]
    pub truncated: bool,
}

impl Listing {
    /// A listing body as a phone sent it, held to the rules: a bad path
    /// refuses it; roots and entries with bad names are left out, and past
    /// [`MAX_ROOTS`] and [`MAX_ENTRIES`] cut.
    pub fn from_body(body: &serde_json::Value) -> Option<Self> {
        let mut listing: Listing = serde_json::from_value(body.clone()).ok()?;
        if !valid_path(&listing.path) {
            return None;
        }
        // One root the phone named badly must not hide the others.
        listing.roots.retain(|r| valid_name(&r.name));
        listing.roots.truncate(MAX_ROOTS);
        listing.entries.retain(|e| valid_name(&e.name));
        if listing.entries.len() > MAX_ENTRIES {
            listing.entries.truncate(MAX_ENTRIES);
            listing.truncated = true;
        }
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

    /// N6: what may be added, and where.
    #[test]
    fn only_shown_names_go_into_a_phone_s_folder() {
        assert!(addable_name("report.pdf") && !addable_name(".hidden") && !addable_name("a/b"));
        let id = "0123456789abcdef0123456789abcdef";
        assert!(
            valid_offer_id(id) && !valid_offer_id("0123") && !valid_offer_id(&id.to_uppercase())
        );
        assert!(mkdir_body(0, &[]).is_none(), "not the root itself");
        assert!(mkdir_body(0, &["New".into(), ".git".into()]).is_none());
        assert_eq!(mkdir_body(2, &["New".into()]).unwrap()["path"][0], "New");
        let put = put_body(1, &["Docs".into()], "a.txt", id, 3).unwrap();
        assert_eq!(
            (put["name"].as_str(), put["size"].as_u64()),
            (Some("a.txt"), Some(3))
        );
        assert!(put_body(1, &[], ".a", id, 3).is_none());
        assert!(put_body(1, &[".x".into()], "a", id, 3).is_none());
        assert!(put_body(1, &[], "a", "nope", 3).is_none());
        assert!(put_body(1, &[], "a", id, MAX_PUT_BYTES + 1).is_none());
        // A root says it takes files only when it does; an older phone's
        // root reads as not.
        let old: Root = serde_json::from_value(serde_json::json!({"id": 0, "name": "A"})).unwrap();
        assert!(!old.add);
        let marked = Root {
            id: 1,
            name: "B".into(),
            add: true,
        };
        assert_eq!(serde_json::to_value(&marked).unwrap()["add"], true);
        assert!(serde_json::to_value(&old).unwrap().get("add").is_none());
    }

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
        assert!(!listing.truncated);
        let many: Vec<_> = (0..MAX_ENTRIES + 1)
            .map(|i| serde_json::json!({ "name": format!("f{i}") }))
            .collect();
        let cut = Listing::from_body(&serde_json::json!({ "entries": many })).unwrap();
        assert_eq!(cut.entries.len(), MAX_ENTRIES);
        assert!(cut.truncated, "more than a listing carries is said");
        let bad = serde_json::json!({ "roots": [], "path": [".."] });
        assert!(Listing::from_body(&bad).is_none());
    }

    #[test]
    fn a_badly_named_root_is_left_out_not_the_listing() {
        let body = serde_json::json!({
            "roots": [
                { "id": 0, "name": "Pics\u{200D}Family" },
                { "id": 1, "name": "Download" }
            ],
            "root": 1, "path": []
        });
        let listing = Listing::from_body(&body).unwrap();
        assert_eq!(
            listing.roots,
            vec![Root {
                id: 1,
                name: "Download".into(),
                add: false,
            }]
        );
    }
}
