//! The computer's music in the phone's media controls (N5): a phone asks
//! to be told what plays, and the computer pushes it on a change.

/// Phone → computer: `{on}`: tell me what plays, or stop.
pub const WATCH: &str = "kdeconnect.pocketdesk.computer_media_watch";

/// Computer → phone: `{playing, artId, art}`: T8c's `playing` (or null),
/// the cover's id while it has one, and the cover's bytes (base64) once
/// per track and phone.
pub const PUSH: &str = "kdeconnect.pocketdesk.computer_media";

/// A cover's id: the first 16 hex digits of its SHA-256, as N2's.
pub fn art_id(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_cover_is_named_by_its_bytes() {
        assert_eq!(super::art_id(b"abc"), "ba7816bf8f01cfea");
    }
}
