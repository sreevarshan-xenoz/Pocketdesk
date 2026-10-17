//! The computer's identity as KDE Connect devices see it (3a): an id and a
//! certificate of its own, never Pocketdesk's. KDE requires the
//! certificate's subject name to be the device id; Pocketdesk's
//! certificate is pinned by phones already and names no id.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The folder under `<state>/certs` that holds it.
pub const DIR: &str = "kdeconnect";

/// The compat id, a key and a certificate whose subject is CN = the id,
/// O = KDE, OU = Kde connect, as KDE devices write theirs.
#[derive(Debug, Clone)]
pub struct CompatIdentity {
    pub id: String,
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
}

impl CompatIdentity {
    /// The identity in `dir`, made there on first use: the key owner-only,
    /// each file written whole and renamed into place, the certificate last
    /// (it marks the set complete).
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        // One at a time: two first connections at once each made their own
        // set and could leave a mismatched one in place for good.
        static MAKING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _one = MAKING.lock().unwrap_or_else(|e| e.into_inner());
        let (id_path, key_path, cert_path) = paths(dir);
        // A set is taken only whole: an id, and a certificate naming it.
        if let (Ok(id), Ok(key_der), Ok(cert_der)) = (
            std::fs::read_to_string(&id_path),
            std::fs::read(&key_path),
            std::fs::read(&cert_path),
        ) && super::handshake::valid_id(id.trim())
            && super::handshake::subject_id(&cert_der).as_deref() == Some(id.trim())
        {
            return Ok(Self {
                id: id.trim().to_string(),
                cert_der,
                key_der,
            });
        }
        std::fs::create_dir_all(dir).context("could not make the KDE identity's folder")?;
        let id = uuid::Uuid::new_v4().to_string().replace('-', "_");
        let (cert_der, key_der) = make_certificate(&id)?;
        let write = |path: &PathBuf, bytes: &[u8], private: bool| -> Result<()> {
            // Its own temporary name: `with_extension` gave the key and the
            // certificate the same one.
            let tmp = PathBuf::from(format!("{}.tmp", path.display()));
            if private {
                pocketdesk_core::secure_fs::write_private(&tmp, bytes)?;
            } else {
                std::fs::write(&tmp, bytes)?;
            }
            std::fs::rename(&tmp, path)?;
            Ok(())
        };
        write(&id_path, id.as_bytes(), false)?;
        write(&key_path, &key_der, true)?;
        write(&cert_path, &cert_der, false)?;
        Ok(Self {
            id,
            cert_der,
            key_der,
        })
    }
}

fn paths(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    (
        dir.join("id"),
        dir.join("identity.key"),
        dir.join("identity.der"),
    )
}

/// A self-signed ECDSA P-256 certificate for `id`, valid from 2025 to 2045.
fn make_certificate(id: &str) -> Result<(Vec<u8>, Vec<u8>)> {
    use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_ECDSA_P256_SHA256};
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).context("no key pair")?;
    let mut params = CertificateParams::default();
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, id);
    name.push(DnType::OrganizationName, "KDE");
    name.push(DnType::OrganizationalUnitName, "Kde connect");
    params.distinguished_name = name;
    params.not_before = rcgen::date_time_ymd(2025, 1, 1);
    params.not_after = rcgen::date_time_ymd(2045, 1, 1);
    let cert = params
        .self_signed(&key)
        .context("could not sign the certificate")?;
    Ok((cert.der().to_vec(), key.serialize_der()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_identity_is_made_once_and_its_certificate_names_its_id() {
        let dir = std::env::temp_dir().join(format!("pd-kde-identity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let made = CompatIdentity::load_or_create(&dir).unwrap();
        assert!(super::super::handshake::valid_id(&made.id), "{}", made.id);
        assert!(!made.id.contains('-'), "a UUID with underscores");
        let again = CompatIdentity::load_or_create(&dir).unwrap();
        assert_eq!(again.id, made.id);
        assert_eq!(again.cert_der, made.cert_der);
        assert_eq!(
            super::super::handshake::subject_id(&made.cert_der).as_deref(),
            Some(made.id.as_str())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_set_that_does_not_match_is_made_again_and_first_uses_side_by_side_agree() {
        let dir = std::env::temp_dir().join(format!(
            "pd-kde-identity-race-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let made = CompatIdentity::load_or_create(&dir).unwrap();
        // An id that is not the certificate's: not taken.
        std::fs::write(dir.join("id"), "z".repeat(36)).unwrap();
        let again = CompatIdentity::load_or_create(&dir).unwrap();
        assert_ne!(again.id, "z".repeat(36));
        assert_ne!(again.id, made.id, "made afresh");
        let fresh = dir.join("fresh");
        let ids: Vec<String> = std::thread::scope(|scope| {
            let runs: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| CompatIdentity::load_or_create(&fresh).unwrap().id))
                .collect();
            runs.into_iter().map(|run| run.join().unwrap()).collect()
        });
        assert!(
            ids.windows(2).all(|w| w[0] == w[1]),
            "one identity: {ids:?}"
        );
        let loaded = CompatIdentity::load_or_create(&fresh).unwrap();
        assert_eq!(
            super::super::handshake::subject_id(&loaded.cert_der).as_deref(),
            Some(loaded.id.as_str())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
