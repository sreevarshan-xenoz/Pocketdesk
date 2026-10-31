//! What a phone needs to recognise this desktop: the TLS certificate it pins,
//! the iroh endpoint it dials over WAN, and the desktop's device id.

use std::io;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::paths::{CERT_FILE, DEVICE_ID_FILE, ENDPOINT_FILE};

/// SHA-256 of `<state_dir>/certs/identity.der` — the hash a phone pins for
/// this desktop (the streaming transport's TOFU verifier pins the same).
pub fn cert_fingerprint(state_dir: &Path) -> io::Result<[u8; 32]> {
    let der = std::fs::read(state_dir.join(CERT_FILE))?;
    Ok(Sha256::digest(&der).into())
}

/// Lower-case hex of a fingerprint.
pub fn fingerprint_hex(fingerprint: [u8; 32]) -> String {
    fingerprint.iter().map(|b| format!("{b:02x}")).collect()
}

/// Record the iroh endpoint the server is reachable at — the body of the
/// `kdeconnect.pocketdesk.endpoint` packet it pushes to paired phones — so a
/// pairing code can carry it without the app linking iroh.
pub fn write_endpoint(state_dir: &Path, body: &serde_json::Value) -> io::Result<()> {
    std::fs::create_dir_all(state_dir)?;
    let path = state_dir.join(ENDPOINT_FILE);
    let tmp = state_dir.join(format!("{ENDPOINT_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(body)?)?;
    crate::atomic_file::replace(&tmp, path)
}

/// The endpoint the server last recorded, if any.
pub fn read_endpoint(state_dir: &Path) -> Option<serde_json::Value> {
    let raw = std::fs::read(state_dir.join(ENDPOINT_FILE)).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// This desktop's device id (`<state_dir>/device_id`), once the server has
/// created it.
pub fn device_id(state_dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(state_dir.join(DEVICE_ID_FILE)).ok()?;
    let id = raw.trim();
    (!id.is_empty()).then(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::paths::CERT_FILE;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-id-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_fingerprint_is_sha256_of_the_der_file() {
        let dir = temp_dir("fp");
        std::fs::create_dir_all(dir.join("certs")).unwrap();
        std::fs::write(dir.join(CERT_FILE), b"abc").unwrap();
        assert_eq!(
            fingerprint_hex(cert_fingerprint(&dir).unwrap()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn the_endpoint_file_round_trips_and_is_absent_until_written() {
        let dir = temp_dir("ep");
        assert!(read_endpoint(&dir).is_none());
        let body =
            serde_json::json!({"endpointId": "e1", "relayUrls": ["https://r/"], "directAddrs": []});
        write_endpoint(&dir, &body).unwrap();
        assert_eq!(read_endpoint(&dir), Some(body));
    }

    #[test]
    fn the_device_id_is_read_trimmed() {
        let dir = temp_dir("id");
        assert_eq!(device_id(&dir), None);
        std::fs::write(dir.join(crate::local::paths::DEVICE_ID_FILE), "abc-123\n").unwrap();
        assert_eq!(device_id(&dir).as_deref(), Some("abc-123"));
    }
}
