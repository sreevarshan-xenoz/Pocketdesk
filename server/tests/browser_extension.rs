//! The browser extension (`browser/`) and its host agree: the id its `key`
//! gives in Chromium browsers and its Firefox id are the ones the host's
//! registration allows, and it asks for the host by its name.

use base64::Engine;
use pocketdesk_server::browser_host::{CHROMIUM_EXTENSION_ID, FIREFOX_EXTENSION_ID, HOST_NAME};
use sha2::{Digest, Sha256};

fn read(path: &str) -> String {
    std::fs::read_to_string(format!("{}/../browser/{path}", env!("CARGO_MANIFEST_DIR")))
        .expect(path)
}

#[test]
fn the_extension_is_the_one_the_host_lets_in() {
    let manifest: serde_json::Value = serde_json::from_str(&read("manifest.json")).unwrap();
    let key = base64::engine::general_purpose::STANDARD
        .decode(manifest["key"].as_str().expect("a key"))
        .expect("base64");
    // Chromium's id: the first 16 bytes of the key's SHA-256, a nibble a letter.
    let id: String = Sha256::digest(&key)
        .iter()
        .take(16)
        .flat_map(|b| [b >> 4, b & 0xf])
        .map(|n| char::from(b'a' + n))
        .collect();
    assert_eq!(id, CHROMIUM_EXTENSION_ID);
    assert_eq!(
        manifest["browser_specific_settings"]["gecko"]["id"],
        FIREFOX_EXTENSION_ID
    );
    let permissions = manifest["permissions"].as_array().unwrap();
    assert!(permissions.iter().any(|p| p == "nativeMessaging"));
    assert!(read("background.js").contains(&format!("\"{HOST_NAME}\"")));
}
