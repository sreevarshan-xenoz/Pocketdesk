//! The phone's recent photos on the computer (C7): the computer asks, the
//! phone answers with its newest camera photos and a small thumbnail of
//! each, and one is fetched by the phone sharing it. Both ends hold the
//! fields to the rules here.

use serde::{Deserialize, Serialize};

use super::phone_files::valid_name;

/// Computer → phone: `{}`, list the newest camera photos.
pub const LIST: &str = "kdeconnect.pocketdesk.phone_photos.list";

/// Phone → computer: the photos, or why there are none.
pub const PHOTOS: &str = "kdeconnect.pocketdesk.phone_photos";

/// Computer → phone: `{id}`, send that photo to the computer.
pub const FETCH: &str = "kdeconnect.pocketdesk.phone_photos.fetch";

/// Photos one answer carries, at most.
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
}

/// What the phone answers.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Photos {
    #[serde(default)]
    pub photos: Vec<Photo>,
    /// Why there are none (the switch off, no photo access), for the owner.
    #[serde(default)]
    pub error: Option<String>,
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
        photos.error = photos
            .error
            .map(|e| e.chars().filter(|c| !c.is_control()).take(200).collect());
        Some(photos)
    }
}

/// A fetch's body, `{id}`.
pub fn fetch_body(id: u64) -> serde_json::Value {
    serde_json::json!({ "id": id })
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
        assert_eq!(refused.error.as_deref(), Some("Turned offon the phone"));
        assert!(refused.photos.is_empty());
    }

    #[test]
    fn a_fetch_names_its_photo() {
        assert_eq!(fetch_body(42), json!({"id": 42}));
    }
}
