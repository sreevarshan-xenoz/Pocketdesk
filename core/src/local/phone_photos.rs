//! The phone's recent photos on the computer (C7): each paired phone's last
//! answer, kept by the server in `<state>/phone_photos.json` with its
//! thumbnails under `<state>/phone_photos/<device>/<id>.jpg` (all
//! owner-only), and the desktop app's requests for the server to send, one
//! file each in `<state>/phone_photos_out/`, taken within
//! [`REQUEST_TTL_SECS`].

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use base64::Engine;
use serde::{Deserialize, Serialize};

use super::approval::is_valid_id;
use super::paths::{PHONE_PHOTOS_DIR, PHONE_PHOTOS_FILE, PHONE_PHOTOS_OUT_DIR};
use crate::protocol::phone_photos::{MAX_THUMB_BYTES, Photos, THUMB_SIDE};

/// How long a request waits for the server before it is dropped.
pub const REQUEST_TTL_SECS: u64 = 60;

/// One photo as kept: what the phone said of it; its thumbnail is a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptPhoto {
    pub id: u64,
    pub name: String,
    pub taken: u64,
    pub size: u64,
    /// Whether its thumbnail was kept ([`thumb_path`]).
    pub thumb: bool,
    /// A video, not a photo (T4).
    #[serde(default)]
    pub video: bool,
}

/// A phone's last answer, and when it came.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Kept {
    pub photos: Vec<KeptPhoto>,
    #[serde(default)]
    pub error: Option<String>,
    pub at_unix: u64,
    /// Whether older ones wait for another page (T4).
    #[serde(default)]
    pub more: bool,
    /// Where the next page starts, when the phone said (T4).
    #[serde(default)]
    pub next: Option<u64>,
}

impl Kept {
    /// The `before` of the next page: where the phone said, else the last
    /// photo kept; `None` when there is none to ask for.
    pub fn next_before(&self) -> Option<u64> {
        if !self.more {
            return None;
        }
        self.next.or_else(|| self.photos.last().map(|p| p.id))
    }
}

/// Photos kept for one phone, at most, its pages together (T4): ten pages.
pub const MAX_KEPT: usize = 10 * crate::protocol::phone_photos::MAX_PHOTOS;

#[derive(Debug, Default, Serialize, Deserialize)]
struct PhotosFile {
    phones: BTreeMap<String, Kept>,
}

/// Each phone's last answer; none when there is no file or it cannot be read.
pub fn read(state_dir: &Path) -> BTreeMap<String, Kept> {
    std::fs::read_to_string(state_dir.join(PHONE_PHOTOS_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<PhotosFile>(&raw).ok())
        .map(|f| f.phones)
        .unwrap_or_default()
}

/// Where `device_id`'s thumbnail of photo `id` is kept; `None` for a device
/// id that is not a plain name.
pub fn thumb_path(state_dir: &Path, device_id: &str, id: u64) -> Option<PathBuf> {
    plain(device_id).then(|| {
        state_dir
            .join(PHONE_PHOTOS_DIR)
            .join(device_id)
            .join(format!("{id}.jpg"))
    })
}

/// A device id fit to name a folder: letters, digits, `-` and `_`.
fn plain(device_id: &str) -> bool {
    !device_id.is_empty()
        && device_id.len() <= 64
        && device_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The largest side a kept thumbnail may declare: twice what a phone makes.
/// A few kilobytes of JPEG can declare 65535 by 65535, which the desktop
/// would decode to gigabytes, or a texture its GPU refuses (C7 review).
pub const MAX_THUMB_SIDE: u32 = 2 * THUMB_SIDE;

/// A thumbnail's bytes from its base64, if they are a JPEG within bounds,
/// its declared size included.
fn thumb_bytes(thumb: &str) -> Option<Vec<u8>> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(thumb)
        .ok()?;
    if bytes.len() > MAX_THUMB_BYTES || !bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return None;
    }
    let (width, height) = jpeg_size(&bytes)?;
    (width > 0 && height > 0 && width <= MAX_THUMB_SIDE && height <= MAX_THUMB_SIDE)
        .then_some(bytes)
}

/// A JPEG's width and height, from its first frame header (SOF0–SOF15,
/// less DHT, JPG and DAC), walking its markers; `None` when it has none.
pub fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut at = 2;
    while at + 4 <= bytes.len() {
        if bytes[at] != 0xFF {
            return None;
        }
        let marker = bytes[at + 1];
        // Fill bytes before a marker.
        if marker == 0xFF {
            at += 1;
            continue;
        }
        // Markers that stand alone.
        if marker == 0x01 || (0xD0..=0xD8).contains(&marker) {
            at += 2;
            continue;
        }
        let length = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
        let frame = (0xC0..=0xCF).contains(&marker) && ![0xC4, 0xC8, 0xCC].contains(&marker);
        if frame {
            let header = bytes.get(at + 4..at + 9)?;
            let height = u16::from_be_bytes([header[1], header[2]]) as u32;
            let width = u16::from_be_bytes([header[3], header[4]]) as u32;
            return Some((width, height));
        }
        if marker == 0xDA || length < 2 {
            return None;
        }
        at += 2 + length;
    }
    None
}

static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Keep `photos` as `device_id`'s last answer: its thumbnails written, the
/// ones it no longer lists removed, and phones `paired` no longer names
/// (unpaired or removed since) dropped with theirs. An older page (T4)
/// joins the pages kept, up to [`MAX_KEPT`], when it follows the last one
/// kept; otherwise it is dropped (the newest page was asked for since).
pub fn keep(
    state_dir: &Path,
    device_id: &str,
    photos: Photos,
    now_unix: u64,
    paired: &dyn Fn(&str) -> bool,
) -> io::Result<()> {
    if !plain(device_id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a device id",
        ));
    }
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut phones = read(state_dir);
    phones.retain(|id, _| paired(id));
    let root = state_dir.join(PHONE_PHOTOS_DIR);
    crate::secure_fs::create_private_dir(&root)?;
    // Folders of phones no longer kept go with them.
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name != device_id && !phones.contains_key(&name) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    let folder = root.join(device_id);
    // T4: an older page joins the kept ones when it follows the last of
    // them. One that does not (the newest was asked for since, or a page
    // asked twice) adds nothing, but is still an answer: the app stops
    // waiting (T4 review: it said the phone did not answer).
    let (mut kept, adds, more_before, next_before) = match photos.before {
        None => {
            let _ = std::fs::remove_dir_all(&folder);
            (Vec::with_capacity(photos.photos.len()), true, false, None)
        }
        Some(before) => match phones.get(device_id) {
            Some(last) if last.next_before() == Some(before) => {
                (last.photos.clone(), true, last.more, last.next)
            }
            Some(last) => (last.photos.clone(), false, last.more, last.next),
            None => return Ok(()),
        },
    };
    crate::secure_fs::create_private_dir(&folder)?;
    let room = if adds {
        MAX_KEPT.saturating_sub(kept.len())
    } else {
        0
    };
    for photo in photos.photos.into_iter().take(room) {
        if kept.iter().any(|k| k.id == photo.id) {
            continue;
        }
        let thumb = match thumb_bytes(&photo.thumb) {
            Some(bytes) => {
                crate::secure_fs::write_private(&folder.join(format!("{}.jpg", photo.id)), &bytes)?;
                true
            }
            None => false,
        };
        kept.push(KeptPhoto {
            id: photo.id,
            name: photo.name,
            taken: photo.taken,
            size: photo.size,
            thumb,
            video: photo.video,
        });
    }
    // Past MAX_KEPT no more pages are offered.
    let (more, next) = if adds {
        (photos.more && kept.len() < MAX_KEPT, photos.next)
    } else {
        (more_before, next_before)
    };
    phones.insert(
        device_id.to_string(),
        Kept {
            photos: kept,
            error: photos.error,
            at_unix: now_unix,
            more,
            next,
        },
    );
    let json = serde_json::to_string(&PhotosFile { phones }).map_err(io::Error::other)?;
    let path = state_dir.join(PHONE_PHOTOS_FILE);
    let tmp = path.with_extension("json.tmp");
    // What is on a phone: owner-only.
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &path)
}

/// Drop them all (at start: they describe a run that ended).
pub fn clear(state_dir: &Path) -> io::Result<()> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let _ = std::fs::remove_dir_all(state_dir.join(PHONE_PHOTOS_DIR));
    match std::fs::remove_file(state_dir.join(PHONE_PHOTOS_FILE)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// What the app asks of the phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "photo")]
pub enum Ask {
    /// Its newest camera photos.
    List,
    /// Those older than that photo: the next page (T4).
    ListBefore(u64),
    /// That photo, sent to the computer.
    Fetch(u64),
    /// That photo, put on the computer's clipboard by the phone (T4: a HEIC
    /// one, which the app cannot decode).
    Copy(u64),
    /// A new photo, the phone's owner taking it with the camera (F3).
    Take,
}

/// One request for the server to send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    pub device_id: String,
    pub ask: Ask,
    pub created_unix: u64,
}

/// Ask the server to send a request (the app's side).
pub fn request(state_dir: &Path, device_id: &str, ask: Ask, now_unix: u64) -> io::Result<Request> {
    let request = Request {
        id: uuid::Uuid::new_v4().to_string(),
        device_id: device_id.to_string(),
        ask,
        created_unix: now_unix,
    };
    let dir = state_dir.join(PHONE_PHOTOS_OUT_DIR);
    crate::secure_fs::create_private_dir(&dir)?;
    let file = dir.join(format!("{}.json", request.id));
    let json = serde_json::to_string(&request).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &file)?;
    Ok(request)
}

/// Take every waiting request (the server's side), oldest first; a stale
/// or misnamed one is dropped.
pub fn take_all(state_dir: &Path, now_unix: u64) -> Vec<Request> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(PHONE_PHOTOS_OUT_DIR)) else {
        return Vec::new();
    };
    let mut taken = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".json"))
            .map(str::to_string)
        else {
            continue;
        };
        let raw = std::fs::read_to_string(&path);
        let _ = std::fs::remove_file(&path);
        let Some(request) = raw
            .ok()
            .and_then(|raw| serde_json::from_str::<Request>(&raw).ok())
        else {
            continue;
        };
        let fresh =
            request.created_unix <= now_unix && now_unix - request.created_unix <= REQUEST_TTL_SECS;
        if fresh && request.id == stem && is_valid_id(&request.id) {
            taken.push(request);
        }
    }
    taken.sort_by_key(|r| r.created_unix);
    taken
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::phone_photos::Photo;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pd-phone-photos-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A JPEG's markers as far as the store reads them: an APP0, then a
    /// baseline frame header of `width` by `height`.
    fn jpeg_of(width: u16, height: u16) -> Vec<u8> {
        let mut b = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x4A, 0x46];
        b.extend([0xFF, 0xC0, 0x00, 0x11, 0x08]);
        b.extend(height.to_be_bytes());
        b.extend(width.to_be_bytes());
        b.extend([0x03; 12]);
        b
    }

    fn jpeg() -> String {
        base64::engine::general_purpose::STANDARD.encode(jpeg_of(256, 192))
    }

    #[test]
    fn a_thumbnail_declaring_a_huge_picture_is_not_kept() {
        assert_eq!(jpeg_size(&jpeg_of(256, 192)), Some((256, 192)));
        assert!(thumb_bytes(&jpeg()).is_some());
        let huge = base64::engine::general_purpose::STANDARD.encode(jpeg_of(65535, 8));
        assert!(
            thumb_bytes(&huge).is_none(),
            "a tiny file declaring 65535 px"
        );
        let zero = base64::engine::general_purpose::STANDARD.encode(jpeg_of(0, 0));
        assert!(thumb_bytes(&zero).is_none());
        let no_frame =
            base64::engine::general_purpose::STANDARD.encode([0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3]);
        assert!(thumb_bytes(&no_frame).is_none(), "no frame header to read");
    }

    fn photo(id: u64, thumb: String) -> Photo {
        Photo {
            id,
            name: format!("IMG_{id}.jpg"),
            taken: 1_000 + id,
            size: 10,
            thumb,
            video: false,
        }
    }

    fn page(ids: std::ops::RangeInclusive<u64>, before: Option<u64>, more: bool) -> Photos {
        Photos {
            photos: ids.rev().map(|id| photo(id, jpeg())).collect(),
            error: None,
            before,
            more,
            next: None,
        }
    }

    /// T4: an older page joins the kept ones when it follows the last of
    /// them, thumbnails kept; one that does not is dropped; a newest page
    /// starts over; and no more than MAX_KEPT are kept.
    #[test]
    fn older_pages_join_the_kept_ones() {
        let dir = temp_dir("pages");
        keep(&dir, "phone-a", page(31..=40, None, true), 5, &|_| true).unwrap();
        keep(&dir, "phone-a", page(21..=30, Some(31), true), 6, &|_| true).unwrap();
        let kept = &read(&dir)["phone-a"];
        assert_eq!(kept.photos.len(), 20);
        assert_eq!(kept.photos.last().unwrap().id, 21);
        assert!(kept.more);
        assert!(
            thumb_path(&dir, "phone-a", 40).unwrap().exists(),
            "the first page's thumbnails stay"
        );
        // A page that does not follow the last one kept adds nothing, but
        // is an answer (its time moves on).
        keep(&dir, "phone-a", page(1..=10, Some(11), false), 7, &|_| true).unwrap();
        let kept = &read(&dir)["phone-a"];
        assert_eq!(kept.photos.len(), 20);
        assert!(kept.more, "still more after the stale page");
        assert_eq!(kept.at_unix, 7);
        // The phone says where the next page starts when it looked past
        // rows it does not list.
        let mut skipping = page(15..=20, Some(21), true);
        skipping.next = Some(3);
        keep(&dir, "phone-a", skipping, 8, &|_| true).unwrap();
        let kept = &read(&dir)["phone-a"];
        assert_eq!(kept.next_before(), Some(3));
        keep(&dir, "phone-a", page(1..=2, Some(3), false), 9, &|_| true).unwrap();
        assert_eq!(read(&dir)["phone-a"].photos.last().unwrap().id, 1);
        assert_eq!(read(&dir)["phone-a"].next_before(), None);
        // The newest page starts over.
        keep(&dir, "phone-a", page(41..=42, None, false), 10, &|_| true).unwrap();
        let kept = &read(&dir)["phone-a"];
        assert_eq!(kept.photos.len(), 2);
        assert!(!kept.more);
        assert!(!thumb_path(&dir, "phone-a", 40).unwrap().exists());
        // Bounded: past MAX_KEPT, no more pages are offered.
        let mut last = 1_000_000;
        keep(
            &dir,
            "phone-b",
            page(last - 23..=last, None, true),
            9,
            &|_| true,
        )
        .unwrap();
        for _ in 0..12 {
            let next = last - 24;
            keep(
                &dir,
                "phone-b",
                page(next - 23..=next, Some(last - 23), true),
                10,
                &|_| true,
            )
            .unwrap();
            last = next;
        }
        let kept = &read(&dir)["phone-b"];
        assert_eq!(kept.photos.len(), MAX_KEPT);
        assert!(!kept.more);
    }

    #[test]
    fn an_answer_is_kept_with_its_thumbnails_and_replaces_the_last() {
        let dir = temp_dir("keep");
        let answer = Photos {
            photos: vec![photo(1, jpeg()), photo(2, "PHN2Zz4=".into())],
            error: None,
            ..Default::default()
        };
        keep(&dir, "phone-a", answer, 5, &|_| true).unwrap();
        let kept = &read(&dir)["phone-a"];
        assert_eq!(kept.photos.len(), 2);
        assert!(kept.photos[0].thumb, "a JPEG is kept");
        assert!(!kept.photos[1].thumb, "an SVG passed off as one is not");
        let one = thumb_path(&dir, "phone-a", 1).unwrap();
        assert!(
            std::fs::read(&one)
                .unwrap()
                .starts_with(&[0xFF, 0xD8, 0xFF])
        );
        assert!(!thumb_path(&dir, "phone-a", 2).unwrap().exists());

        // A newer answer replaces it, thumbnails too.
        let newer = Photos {
            photos: vec![photo(3, jpeg())],
            error: None,
            ..Default::default()
        };
        keep(&dir, "phone-a", newer, 6, &|_| true).unwrap();
        assert_eq!(read(&dir)["phone-a"].photos[0].id, 3);
        assert!(!one.exists(), "the old thumbnail went");
    }

    #[test]
    fn an_unpaired_phones_photos_go_and_a_bad_device_id_is_refused() {
        let dir = temp_dir("unpaired");
        keep(
            &dir,
            "phone-a",
            Photos {
                photos: vec![photo(1, jpeg())],
                error: None,
                ..Default::default()
            },
            5,
            &|_| true,
        )
        .unwrap();
        keep(&dir, "phone-b", Photos::default(), 6, &|id| id == "phone-b").unwrap();
        assert_eq!(read(&dir).keys().collect::<Vec<_>>(), ["phone-b"]);
        assert!(!thumb_path(&dir, "phone-a", 1).unwrap().exists());
        assert!(keep(&dir, "../up", Photos::default(), 7, &|_| true).is_err());
        assert!(thumb_path(&dir, "../up", 1).is_none());
        clear(&dir).unwrap();
        assert!(read(&dir).is_empty());
    }

    #[test]
    fn a_request_is_taken_once_while_fresh() {
        let dir = temp_dir("requests");
        let asked = request(&dir, "phone", Ask::Fetch(7), 1_000).unwrap();
        request(&dir, "phone", Ask::List, 1_000 - REQUEST_TTL_SECS - 1).unwrap();
        assert_eq!(take_all(&dir, 1_001), vec![asked]);
        assert!(take_all(&dir, 1_001).is_empty());
    }
}
