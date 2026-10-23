//! The phone's recent photos on the computer (C7): the computer asks, the
//! phone answers with its newest camera photos and a small thumbnail of
//! each, and one is fetched by the phone sharing it. Both ends hold the
//! fields to the rules here. T4: older pages (`before` the last one shown),
//! the camera's videos with them, and a photo put on the computer's
//! clipboard by the phone (a HEIC one the computer cannot decode).

use serde::{Deserialize, Serialize};

use super::kdeconnect::is_invisible_format;
use super::phone_files::valid_name;

/// Computer → phone: `{}`, list the newest camera photos; `{before: id}`,
/// the page of those older than photo `id` (T4).
pub const LIST: &str = "kdeconnect.pocketdesk.phone_photos.list";

/// Phone → computer: the photos, or why there are none.
pub const PHOTOS: &str = "kdeconnect.pocketdesk.phone_photos";

/// Computer → phone: `{id}`, send that photo to the computer; `{id,
/// clipboard: true}`, put it on the computer's clipboard as a PNG (T4).
pub const FETCH: &str = "kdeconnect.pocketdesk.phone_photos.fetch";

/// Computer → phone: `{}`, offer its owner the camera; the photo taken is
/// sent to the computer as a file (F3).
pub const TAKE: &str = "kdeconnect.pocketdesk.phone_photos.take";

/// Photos one answer (a page) carries, at most.
pub const MAX_PHOTOS: usize = 24;

/// A thumbnail's bytes, at most (a JPEG of at most 256 px a side).
pub const MAX_THUMB_BYTES: usize = 48 * 1024;

/// A thumbnail's longest side, in pixels.
pub const THUMB_SIDE: u32 = 256;

/// The base64 text of [`MAX_THUMB_BYTES`], at most.
const MAX_THUMB_TEXT: usize = MAX_THUMB_BYTES.div_ceil(3) * 4;

/// One photo the phone offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Photo {
    /// The phone's MediaStore id for it.
    pub id: u64,
    pub name: String,
    /// When it was taken, milliseconds since the epoch; 0 when unknown.
    #[serde(default)]
    pub taken: u64,
    /// Its size in bytes.
    #[serde(default)]
    pub size: u64,
    /// Its thumbnail, a JPEG in standard base64.
    #[serde(default)]
    pub thumb: String,
    /// A video, not a photo (T4).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub video: bool,
}

/// What the phone answers.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Photos {
    #[serde(default)]
    pub photos: Vec<Photo>,
    /// Why there are none (the switch off, no photo access), for the owner.
    #[serde(default)]
    pub error: Option<String>,
    /// The page this answers: those older than this photo; `None` for the
    /// newest (T4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<u64>,
    /// Whether older ones wait for another page (T4).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub more: bool,
    /// Where the next page starts (its `before`), when it is not this
    /// page's last photo: the phone looked past rows it does not list (T4
    /// review: a page of none would ask for itself again).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<u64>,
}

/// Whether `thumb` could be a thumbnail: standard base64 within
/// [`MAX_THUMB_BYTES`] (empty means none).
pub fn valid_thumb(thumb: &str) -> bool {
    thumb.len() <= MAX_THUMB_TEXT
        && thumb.len().is_multiple_of(4)
        && thumb
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
}

impl Photos {
    /// An answer as a phone sent it, held to the rules: photos with a bad
    /// name or thumbnail are left out, and past [`MAX_PHOTOS`] cut; `None`
    /// for a body that is not an answer at all.
    pub fn from_body(body: &serde_json::Value) -> Option<Self> {
        let mut photos: Photos = serde_json::from_value(body.clone()).ok()?;
        photos
            .photos
            .retain(|p| valid_name(&p.name) && valid_thumb(&p.thumb));
        photos.photos.truncate(MAX_PHOTOS);
        // Shown to the owner: no control or invisible format characters
        // (a right-to-left override could turn the words around).
        photos.error = photos.error.map(|e| {
            e.chars()
                .filter(|c| !c.is_control() && !is_invisible_format(*c))
                .take(200)
                .collect()
        });
        Some(photos)
    }
}

/// A list's body: `{}` for the newest, `{before}` for an older page (T4).
pub fn list_body(before: Option<u64>) -> serde_json::Value {
    match before {
        Some(id) => serde_json::json!({ "before": id }),
        None => serde_json::json!({}),
    }
}

/// A fetch's body, `{id}`; `{id, clipboard: true}` for the computer's
/// clipboard (T4).
pub fn fetch_body(id: u64, clipboard: bool) -> serde_json::Value {
    if clipboard {
        serde_json::json!({ "id": id, "clipboard": true })
    } else {
        serde_json::json!({ "id": id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_thumbnail_is_bounded_base64() {
        assert!(valid_thumb(""), "none");
        assert!(valid_thumb("/9j/4AAQ"));
        assert!(!valid_thumb("/9j/4AA"), "not a multiple of four");
        assert!(!valid_thumb("/9j/4A\nQ"), "a line break");
        assert!(!valid_thumb("<svg></svg>"));
        assert!(valid_thumb(&"A".repeat(MAX_THUMB_TEXT)));
        assert!(!valid_thumb(&"A".repeat(MAX_THUMB_TEXT + 4)), "over 48 KiB");
    }

    #[test]
    fn an_answer_keeps_only_good_photos_and_at_most_so_many() {
        let good = |i: u64| json!({"id": i, "name": format!("IMG_{i}.jpg"), "taken": 1, "size": 2, "thumb": "/9j/4AAQ"});
        let mut photos: Vec<_> = (0..30).map(good).collect();
        photos.insert(
            0,
            json!({"id": 99, "name": "../escape.jpg", "thumb": "/9j/4AAQ"}),
        );
        photos.insert(
            1,
            json!({"id": 98, "name": "ok.jpg", "thumb": "not base64!"}),
        );
        let answer = Photos::from_body(&json!({"photos": photos})).unwrap();
        assert_eq!(answer.photos.len(), MAX_PHOTOS);
        assert_eq!(
            answer.photos[0].id, 0,
            "the bad ones left out, the order kept"
        );
        assert!(Photos::from_body(&json!({"photos": "nope"})).is_none());
        let refused = Photos::from_body(&json!({"error": "Turned off\non the phone"})).unwrap();
        let turned = Photos::from_body(&json!({"error": "abc\u{202E}def"})).unwrap();
        assert_eq!(
            turned.error.as_deref(),
            Some("abcdef"),
            "no right-to-left override"
        );
        assert_eq!(refused.error.as_deref(), Some("Turned offon the phone"));
        assert!(refused.photos.is_empty());
    }

    #[test]
    fn a_fetch_names_its_photo() {
        assert_eq!(fetch_body(42, false), json!({"id": 42}));
        assert_eq!(fetch_body(42, true), json!({"id": 42, "clipboard": true}));
    }

    /// T4: a page names the photo it is older than, an answer says which
    /// page it is and whether more wait, and a video says so.
    #[test]
    fn pages_and_videos() {
        assert_eq!(list_body(None), json!({}));
        assert_eq!(list_body(Some(7)), json!({"before": 7}));
        let answer = Photos::from_body(&json!({
            "photos": [{"id": 5, "name": "VID_5.mp4", "video": true}, {"id": 4, "name": "IMG_4.jpg"}],
            "before": 7,
            "more": true,
        }))
        .unwrap();
        assert_eq!(answer.before, Some(7));
        assert!(answer.more);
        assert!(answer.photos[0].video && !answer.photos[1].video);
        // An answer from before T4 is the newest page, with no more.
        let old = Photos::from_body(&json!({"photos": []})).unwrap();
        assert_eq!((old.before, old.more), (None, false));
    }
}
