//! The pictures phones' notifications bring (K3): each app's icon, kept by
//! phone and app so later notifications show it, and a notification's own
//! picture, kept for its toast. In `<state>/notification_icons/`, owner-only,
//! each checked by its header before anything decodes it.

use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::paths::NOTIFICATION_ICONS_DIR;

/// The longest side an app's icon may declare.
pub const MAX_ICON_SIDE: u32 = 1024;

/// The longest side a notification's picture may declare.
pub const MAX_PICTURE_SIDE: u32 = 4096;

/// The most bytes either may hold.
pub const MAX_BYTES: usize = 2 << 20;

/// Icons kept at most, the oldest dropped first.
pub const MAX_ICONS: usize = 200;

/// Pictures kept at most, the oldest dropped first.
pub const MAX_PICTURES: usize = 30;

/// A PNG's width and height from its header, if it is one.
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    if bytes.len() < 24 || bytes[..8] != SIGNATURE || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((width, height))
}

/// A JPEG's width and height from its frame header, if it is one.
pub fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut at = 2;
    while at + 4 <= bytes.len() {
        if bytes[at] != 0xFF {
            return None;
        }
        let marker = bytes[at + 1];
        // Fill bytes, and markers with no length.
        if marker == 0xFF {
            at += 1;
            continue;
        }
        if marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            at += 2;
            continue;
        }
        let len = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
        if len < 2 {
            return None;
        }
        // A start of frame (not DHT, JPG or DAC, which share the range).
        if (0xC0..=0xCF).contains(&marker) && ![0xC4, 0xC8, 0xCC].contains(&marker) {
            let frame = bytes.get(at + 4..at + 9)?;
            let height = u32::from(u16::from_be_bytes([frame[1], frame[2]]));
            let width = u32::from(u16::from_be_bytes([frame[3], frame[4]]));
            return Some((width, height));
        }
        // The scan's data follows: no frame header before it is none.
        if marker == 0xDA {
            return None;
        }
        at += 2 + len;
    }
    None
}

/// Whether `size` fits `max` on each side and is not empty.
fn fits((width, height): (u32, u32), max: u32) -> bool {
    width > 0 && height > 0 && width <= max && height <= max
}

/// Whether `bytes` is an icon to keep: a PNG of at most [`MAX_BYTES`],
/// declaring at most [`MAX_ICON_SIDE`] a side.
pub fn icon_fits(bytes: &[u8]) -> bool {
    bytes.len() <= MAX_BYTES && png_size(bytes).is_some_and(|size| fits(size, MAX_ICON_SIDE))
}

/// Whether `bytes` is a picture to keep: a JPEG of at most [`MAX_BYTES`],
/// declaring at most [`MAX_PICTURE_SIDE`] a side.
pub fn picture_fits(bytes: &[u8]) -> bool {
    bytes.len() <= MAX_BYTES && jpeg_size(bytes).is_some_and(|size| fits(size, MAX_PICTURE_SIDE))
}

/// A name for `parts`, the same each time, safe as a file name.
fn name_for(parts: &[&str]) -> String {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update(part.as_bytes());
        hash.update([0]);
    }
    hash.finalize()[..12]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Where `phone`'s icon for `app` is kept (the app's name in any case).
pub fn icon_path(state_dir: &Path, phone: &str, app: &str) -> PathBuf {
    state_dir
        .join(NOTIFICATION_ICONS_DIR)
        .join(format!("{}.png", name_for(&[phone, &app.to_lowercase()])))
}

/// Where the picture of `phone`'s notification known by `tag` is kept.
pub fn picture_path(state_dir: &Path, phone: &str, tag: &str) -> PathBuf {
    state_dir
        .join(NOTIFICATION_ICONS_DIR)
        .join("pictures")
        .join(format!("{}.jpg", name_for(&[phone, tag])))
}

/// `phone`'s icon for `app`, if one was kept.
pub fn kept_icon(state_dir: &Path, phone: &str, app: &str) -> Option<PathBuf> {
    let path = icon_path(state_dir, phone, app);
    path.is_file().then_some(path)
}

/// Keep `bytes` as `phone`'s icon for `app`, if it fits; at most
/// [`MAX_ICONS`] are kept. The path kept, or why not.
pub fn keep_icon(state_dir: &Path, phone: &str, app: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    if !icon_fits(bytes) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not an icon"));
    }
    let path = icon_path(state_dir, phone, app);
    write(&path, bytes)?;
    prune(path.parent().unwrap_or(state_dir), "png", MAX_ICONS);
    Ok(path)
}

/// Keep `bytes` as the picture of `phone`'s notification `tag`, if it fits;
/// at most [`MAX_PICTURES`] are kept. The path kept, or why not.
pub fn keep_picture(state_dir: &Path, phone: &str, tag: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    if !picture_fits(bytes) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not a picture"));
    }
    let path = picture_path(state_dir, phone, tag);
    write(&path, bytes)?;
    prune(path.parent().unwrap_or(state_dir), "jpg", MAX_PICTURES);
    Ok(path)
}

fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = path.parent().unwrap_or(Path::new("."));
    crate::secure_fs::create_private_dir(dir)?;
    let tmp = path.with_extension(format!(
        "{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    crate::secure_fs::write_private(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Drop the oldest files ending in `ext` in `dir` past `keep`.
fn prune(dir: &Path, ext: &str, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == ext))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if files.len() <= keep {
        return;
    }
    files.sort();
    for (_, path) in &files[..files.len() - keep] {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = vec![
            0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0, 0, 0, 13,
        ];
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes
    }

    fn jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8];
        // An APP0 segment first, as JFIF has.
        bytes.extend_from_slice(&[0xFF, 0xE0, 0, 4, 0, 0]);
        // A DHT, in the SOF range but not a frame.
        bytes.extend_from_slice(&[0xFF, 0xC4, 0, 3, 0]);
        bytes.extend_from_slice(&[0xFF, 0xC0, 0, 11, 8]);
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&[1, 1, 0x11, 0]);
        bytes
    }

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-icons-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// K3: sizes come from the header, before anything decodes.
    #[test]
    fn sizes_are_read_from_the_header() {
        assert_eq!(png_size(&png(64, 48)), Some((64, 48)));
        assert_eq!(png_size(b"not a png at all, not at all"), None);
        assert_eq!(jpeg_size(&jpeg(720, 405)), Some((720, 405)));
        assert_eq!(
            jpeg_size(&[0xFF, 0xD8, 0xFF, 0xDA, 0, 2]),
            None,
            "a scan first"
        );
        assert_eq!(
            jpeg_size(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 200]),
            None,
            "cut short"
        );
        assert_eq!(jpeg_size(&png(1, 1)), None);
    }

    /// K3: an icon or picture declaring too much is refused, a few bytes
    /// or not.
    #[test]
    fn what_declares_too_much_is_refused() {
        assert!(icon_fits(&png(64, 64)));
        assert!(!icon_fits(&png(1025, 10)));
        assert!(!icon_fits(&png(0, 10)));
        assert!(!icon_fits(&jpeg(64, 64)), "an icon is a PNG");
        assert!(picture_fits(&jpeg(4096, 1)));
        assert!(!picture_fits(&jpeg(4097, 1)));
        let mut big = png(64, 64);
        big.resize(MAX_BYTES + 1, 0);
        assert!(!icon_fits(&big));
    }

    /// K3: an icon is kept by phone and app in any case, and the oldest go
    /// past the cap.
    #[test]
    fn icons_are_kept_by_phone_and_app_and_capped() {
        let dir = dir("keep");
        assert_eq!(kept_icon(&dir, "phone-a", "Chat"), None);
        let path = keep_icon(&dir, "phone-a", "Chat", &png(64, 64)).unwrap();
        assert_eq!(kept_icon(&dir, "phone-a", "chat"), Some(path));
        assert_eq!(kept_icon(&dir, "phone-b", "Chat"), None);
        assert!(keep_icon(&dir, "phone-a", "Mail", &png(2000, 2000)).is_err());
        for i in 0..MAX_ICONS + 5 {
            keep_icon(&dir, "phone-a", &format!("app {i}"), &png(8, 8)).unwrap();
        }
        let icons = std::fs::read_dir(dir.join(NOTIFICATION_ICONS_DIR))
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "png"))
            .count();
        assert_eq!(icons, MAX_ICONS);
        let picture = keep_picture(&dir, "phone-a", "tag1", &jpeg(10, 10)).unwrap();
        assert!(picture.is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
