//! Android Key Attestation, for pinning a phone's approval key (approve with
//! my phone, review I1). Root's `--pin` asks the phone to make its key with
//! a challenge; the phone answers with the key's attestation chain. Here the
//! chain is checked up to one of Google's hardware attestation roots
//! (embedded, `attestation_roots.txt`, matched by public key), each
//! certificate signed by the next and in date, every issuer a CA; and the
//! leaf's key description must say the key is in hardware (the TEE or
//! StrongBox), was generated there, signs, is EC P-256, needs the user's
//! authentication for each use, belongs to the Pocketdesk app, and carries
//! the challenge. Only then is the leaf's key pinned.
//!
//! Not checked: Google's revocation list (online; SECURITY.md limit 73 says
//! so) and the boot state.

use base64::Engine as _;
use x509_parser::prelude::*;

/// The KeyDescription extension.
const KEY_DESCRIPTION_OID: &str = "1.3.6.1.4.1.11129.2.1.17";
/// The app the key must belong to (its debug build carries a suffix).
const PACKAGE: &str = "dev.pocketdesk.android";
/// The SHA-256 of the release APK's signing certificate (packaging/README.md),
/// which the attestation names among the app's signers (review N9). A debug
/// build is signed with each machine's own debug key, so it is taken only
/// with `--pin … --debug-app`.
pub const RELEASE_SIGNER: &str = "3a2452b38b4be5ac8493b330a91ed5a43e8490e6c74b8a7934599bc5e8ac3a0f";

/// Where the key lives, as the attestation says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityLevel {
    Software,
    TrustedEnvironment,
    StrongBox,
}

impl SecurityLevel {
    fn of(v: i64) -> Option<Self> {
        match v {
            0 => Some(Self::Software),
            1 => Some(Self::TrustedEnvironment),
            2 => Some(Self::StrongBox),
            _ => None,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Self::Software => "software",
            Self::TrustedEnvironment => "tee",
            Self::StrongBox => "strongbox",
        }
    }
}

/// What a leaf's key description says, as far as pinning needs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KeyDescription {
    pub attestation_level: Option<SecurityLevel>,
    pub keymint_level: Option<SecurityLevel>,
    pub challenge: Vec<u8>,
    /// From the hardware-enforced list.
    pub purposes: Vec<i64>,
    pub algorithm: Option<i64>,
    pub ec_curve: Option<i64>,
    pub no_auth_required: bool,
    pub user_auth_type: Option<i64>,
    pub auth_timeout: Option<i64>,
    pub origin: Option<i64>,
    /// From the software-enforced list: the apps the key belongs to, and
    /// the SHA-256 of their signing certificates.
    pub packages: Vec<String>,
    pub signers: Vec<Vec<u8>>,
}

/// Why a chain or its key is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    Malformed(String),
    BadSignature(usize),
    NotCa(usize),
    OutOfDate(usize),
    UnknownRoot,
    NoKeyDescription,
    Policy(&'static str),
}

/// Google's roots' public keys (raw SubjectPublicKeyInfo DER).
pub fn google_roots() -> Vec<Vec<u8>> {
    pem_certs(include_str!("attestation_roots.txt"))
        .iter()
        .filter_map(|der| {
            let (_, cert) = parse_x509_certificate(der).ok()?;
            Some(cert.tbs_certificate.subject_pki.raw.to_vec())
        })
        .collect()
}

fn pem_certs(text: &str) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("-----BEGIN CERTIFICATE-----") {
        let body = &rest[start + 27..];
        let Some(end) = body.find("-----END CERTIFICATE-----") else {
            break;
        };
        let b64: String = body[..end].split_whitespace().collect();
        if let Ok(der) = base64::engine::general_purpose::STANDARD.decode(b64) {
            out.push(der);
        }
        rest = &body[end..];
    }
    out
}

const P256: &str = "1.2.840.10045.3.1.7";
const P384: &str = "1.3.132.0.34";

/// Whether `child` is signed by `issuer`'s key.
fn signed_by(child: &X509Certificate, issuer: &X509Certificate) -> bool {
    use ring::signature as s;
    let spki = &issuer.tbs_certificate.subject_pki;
    let key = spki.subject_public_key.data.as_ref();
    let key_alg = spki.algorithm.algorithm.to_id_string();
    let curve = spki
        .algorithm
        .parameters
        .as_ref()
        .and_then(|p| p.as_oid().ok())
        .map(|o| o.to_id_string());
    let sig_alg = child.signature_algorithm.algorithm.to_id_string();
    let alg: &dyn s::VerificationAlgorithm =
        match (sig_alg.as_str(), key_alg.as_str(), curve.as_deref()) {
            ("1.2.840.10045.4.3.2", "1.2.840.10045.2.1", Some(P256)) => &s::ECDSA_P256_SHA256_ASN1,
            ("1.2.840.10045.4.3.2", "1.2.840.10045.2.1", Some(P384)) => &s::ECDSA_P384_SHA256_ASN1,
            ("1.2.840.10045.4.3.3", "1.2.840.10045.2.1", Some(P256)) => &s::ECDSA_P256_SHA384_ASN1,
            ("1.2.840.10045.4.3.3", "1.2.840.10045.2.1", Some(P384)) => &s::ECDSA_P384_SHA384_ASN1,
            ("1.2.840.113549.1.1.11", "1.2.840.113549.1.1.1", _) => &s::RSA_PKCS1_2048_8192_SHA256,
            ("1.2.840.113549.1.1.12", "1.2.840.113549.1.1.1", _) => &s::RSA_PKCS1_2048_8192_SHA384,
            _ => return false,
        };
    s::UnparsedPublicKey::new(alg, key)
        .verify(
            child.tbs_certificate.as_ref(),
            child.signature_value.data.as_ref(),
        )
        .is_ok()
}

fn is_ca(cert: &X509Certificate) -> bool {
    matches!(cert.basic_constraints(), Ok(Some(bc)) if bc.value.ca)
}

/// Check `chain` (leaf first, DER) up to one of `roots` (public keys) at
/// `now_unix`. The leaf's key (raw SubjectPublicKeyInfo DER) and its key
/// description.
pub fn verify_chain(
    chain: &[Vec<u8>],
    roots: &[Vec<u8>],
    now_unix: i64,
) -> Result<(Vec<u8>, KeyDescription), Refused> {
    if !(2..=pocketdesk_core::protocol::approve::MAX_CHAIN).contains(&chain.len()) {
        return Err(Refused::Malformed("a chain is 2 to 8 certificates".into()));
    }
    let mut certs = Vec::new();
    for (i, der) in chain.iter().enumerate() {
        let (rest, cert) = parse_x509_certificate(der)
            .map_err(|e| Refused::Malformed(format!("certificate {i}: {e}")))?;
        if !rest.is_empty() {
            return Err(Refused::Malformed(format!(
                "certificate {i}: trailing bytes"
            )));
        }
        certs.push(cert);
    }
    let now = ASN1Time::from_timestamp(now_unix).map_err(|_| Refused::Malformed("time".into()))?;
    // Every certificate but the top one, whose key is the trust anchor
    // (older phones still carry Google's 2016 root certificate, out of date
    // since 2026, around the same key).
    for (i, cert) in certs.iter().enumerate().take(certs.len() - 1) {
        if !cert.validity().is_valid_at(now) {
            return Err(Refused::OutOfDate(i));
        }
    }
    for i in 0..certs.len() - 1 {
        let (child, issuer) = (&certs[i], &certs[i + 1]);
        if !is_ca(issuer) {
            return Err(Refused::NotCa(i + 1));
        }
        if child.tbs_certificate.issuer.as_raw() != issuer.tbs_certificate.subject.as_raw()
            || !signed_by(child, issuer)
        {
            return Err(Refused::BadSignature(i));
        }
    }
    let top = certs.last().expect("two or more");
    if !roots
        .iter()
        .any(|r| r.as_slice() == top.tbs_certificate.subject_pki.raw)
    {
        return Err(Refused::UnknownRoot);
    }
    let leaf = &certs[0];
    let ext = leaf
        .extensions()
        .iter()
        .find(|e| e.oid.to_id_string() == KEY_DESCRIPTION_OID)
        .ok_or(Refused::NoKeyDescription)?;
    let description = parse_key_description(ext.value)
        .ok_or_else(|| Refused::Malformed("the key description".into()))?;
    Ok((leaf.tbs_certificate.subject_pki.raw.to_vec(), description))
}

/// Whether the key may be pinned: in hardware, generated there, for signing,
/// EC P-256, the user's authentication for each use, Pocketdesk's (its
/// release signer, or with `debug_app` any signer), with `challenge`.
pub fn check_policy(
    d: &KeyDescription,
    challenge: &[u8],
    debug_app: bool,
) -> Result<SecurityLevel, Refused> {
    let level = match (d.attestation_level, d.keymint_level) {
        (Some(a), Some(k)) if a == k && a != SecurityLevel::Software => a,
        _ => {
            return Err(Refused::Policy(
                "the key is not in the phone's secure hardware",
            ));
        }
    };
    if d.challenge != challenge {
        return Err(Refused::Policy("the challenge is not this pin's"));
    }
    if !d.purposes.contains(&2) {
        return Err(Refused::Policy("the key does not sign"));
    }
    if d.algorithm != Some(3) || d.ec_curve != Some(1) {
        return Err(Refused::Policy("the key is not EC P-256"));
    }
    if d.origin != Some(0) {
        return Err(Refused::Policy(
            "the key was not made in the phone's hardware",
        ));
    }
    if d.no_auth_required || d.user_auth_type.unwrap_or(0) == 0 {
        return Err(Refused::Policy(
            "the key does not need the fingerprint or screen lock",
        ));
    }
    if d.auth_timeout.is_some_and(|t| t != 0) {
        return Err(Refused::Policy("the key does not need it for each use"));
    }
    if !d
        .packages
        .iter()
        .any(|p| p == PACKAGE || p.strip_prefix(PACKAGE).is_some_and(|s| s.starts_with('.')))
    {
        return Err(Refused::Policy("the key is not the Pocketdesk app's"));
    }
    let release = hex_bytes(RELEASE_SIGNER);
    if !debug_app && !d.signers.contains(&release) {
        return Err(Refused::Policy(
            "the app is not signed with Pocketdesk's release key (a debug build needs --debug-app)",
        ));
    }
    Ok(level)
}

fn hex_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len() / 2)
        .filter_map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok())
        .collect()
}

/// One DER element: class bits, constructed, tag number, contents.
type Element<'a> = (u8, bool, u32, &'a [u8]);
/// An element and what follows it.
type Read<'a> = (u8, bool, u32, &'a [u8], &'a [u8]);

/// One DER element: (class bits, constructed, tag number, contents, rest).
fn tlv(d: &[u8]) -> Option<Read<'_>> {
    let first = *d.first()?;
    let class = first >> 6;
    let constructed = first & 0x20 != 0;
    let mut i = 1;
    let mut tag = u32::from(first & 0x1f);
    if tag == 0x1f {
        tag = 0;
        loop {
            let b = *d.get(i)?;
            i += 1;
            tag = tag.checked_mul(128)?.checked_add(u32::from(b & 0x7f))?;
            if b & 0x80 == 0 {
                break;
            }
            if i > 5 {
                return None;
            }
        }
    }
    let l0 = *d.get(i)?;
    i += 1;
    let len = if l0 & 0x80 == 0 {
        usize::from(l0)
    } else {
        let n = usize::from(l0 & 0x7f);
        if n == 0 || n > 4 {
            return None;
        }
        let mut len = 0usize;
        for _ in 0..n {
            len = (len << 8) | usize::from(*d.get(i)?);
            i += 1;
        }
        len
    };
    let end = i.checked_add(len)?;
    Some((class, constructed, tag, d.get(i..end)?, &d[end..]))
}

/// Every element in `d`.
fn elements(mut d: &[u8]) -> Option<Vec<Element<'_>>> {
    let mut out = Vec::new();
    while !d.is_empty() {
        let (c, k, t, v, rest) = tlv(d)?;
        out.push((c, k, t, v));
        d = rest;
        if out.len() > 256 {
            return None;
        }
    }
    Some(out)
}

fn integer(v: &[u8]) -> Option<i64> {
    if v.is_empty() || v.len() > 8 {
        return None;
    }
    let mut n: i64 = if v[0] & 0x80 != 0 { -1 } else { 0 };
    for b in v {
        n = (n << 8) | i64::from(*b);
    }
    Some(n)
}

const UNIVERSAL: u8 = 0;
const CONTEXT: u8 = 2;

/// The value inside an explicit `[tag]`: one INTEGER (or ENUMERATED).
fn tagged_int(v: &[u8]) -> Option<i64> {
    let (_, _, _, inner, _) = tlv(v)?;
    integer(inner)
}

fn parse_key_description(ext: &[u8]) -> Option<KeyDescription> {
    let (UNIVERSAL, true, 16, seq, _) = tlv(ext)? else {
        return None;
    };
    let items = elements(seq)?;
    if items.len() < 8 {
        return None;
    }
    let mut d = KeyDescription {
        attestation_level: SecurityLevel::of(integer(items[1].3)?),
        keymint_level: SecurityLevel::of(integer(items[3].3)?),
        challenge: items[4].3.to_vec(),
        ..Default::default()
    };
    // softwareEnforced: the app's identity.
    for (class, _, tag, v) in elements(items[6].3)? {
        if class == CONTEXT
            && tag == 709
            && let Some((names, signers)) = app_id(v)
        {
            d.packages = names;
            d.signers = signers;
        }
    }
    // hardwareEnforced (the TEE's or StrongBox's).
    for (class, _, tag, v) in elements(items[7].3)? {
        if class != CONTEXT {
            continue;
        }
        match tag {
            1 => {
                let (_, _, _, set, _) = tlv(v)?;
                for (_, _, _, p) in elements(set)? {
                    d.purposes.push(integer(p)?);
                }
            }
            2 => d.algorithm = tagged_int(v),
            10 => d.ec_curve = tagged_int(v),
            503 => d.no_auth_required = true,
            504 => d.user_auth_type = tagged_int(v),
            505 => d.auth_timeout = tagged_int(v),
            702 => d.origin = tagged_int(v),
            _ => {}
        }
    }
    Some(d)
}

/// The package names in an `attestationApplicationId` ([709] OCTET STRING
/// holding SEQUENCE { SET OF SEQUENCE { OCTET STRING name, INTEGER
/// version }, SET OF OCTET STRING }).
fn app_id(v: &[u8]) -> Option<(Vec<String>, Vec<Vec<u8>>)> {
    let (_, _, _, octets, _) = tlv(v)?;
    let (_, _, _, seq, _) = tlv(octets)?;
    let parts = elements(seq)?;
    let (_, _, _, infos) = *parts.first()?;
    let mut names = Vec::new();
    for (_, _, _, info) in elements(infos)? {
        let (_, _, _, name, _) = tlv(info)?;
        names.push(String::from_utf8(name.to_vec()).ok()?);
    }
    let mut signers = Vec::new();
    if let Some((_, _, _, digests)) = parts.get(1) {
        for (_, _, _, digest) in elements(digests)? {
            signers.push(digest.to_vec());
        }
    }
    Some((names, signers))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real attestation chains from Google's android-key-attestation project
    // (Apache-2.0), `src/test/resources/der/algorithm_EC_SecurityLevel_*`.
    // The TEE chain ends at Google's RSA root; the StrongBox one at a root
    // that is not Google's production root.
    fn tee() -> Vec<Vec<u8>> {
        vec![
            include_bytes!("attestation_testdata/tee_cert0.der").to_vec(),
            include_bytes!("attestation_testdata/tee_cert1.der").to_vec(),
            include_bytes!("attestation_testdata/tee_cert2.der").to_vec(),
            include_bytes!("attestation_testdata/tee_cert3.der").to_vec(),
        ]
    }

    fn strongbox() -> Vec<Vec<u8>> {
        vec![
            include_bytes!("attestation_testdata/strongbox_cert0.der").to_vec(),
            include_bytes!("attestation_testdata/strongbox_cert1.der").to_vec(),
            include_bytes!("attestation_testdata/strongbox_cert2.der").to_vec(),
            include_bytes!("attestation_testdata/strongbox_cert3.der").to_vec(),
        ]
    }

    /// 2027-01-01: within the sample chain's intermediates' dates.
    const WHEN: i64 = 1_798_761_600;

    #[test]
    fn the_embedded_roots_are_googles_two() {
        assert_eq!(google_roots().len(), 2);
    }

    #[test]
    fn a_real_tee_chain_verifies_to_googles_root_and_reads() {
        let mut chain = tee();
        // Its own root certificate is out of date: trust is by the key, so
        // drop it and end at the intermediate Google's root signed.
        chain.pop();
        let err = verify_chain(&chain, &google_roots(), WHEN).unwrap_err();
        assert_eq!(err, Refused::UnknownRoot, "the root key must be at the top");
        let mut chain = tee();
        // Its root certificate ran out in 2026; the key it carries is what
        // is trusted, so the chain still verifies.
        let (leaf, d) = verify_chain(&chain, &google_roots(), WHEN).unwrap();
        // After the intermediates' end (2028-03-18), it does not.
        assert_eq!(
            verify_chain(&chain, &google_roots(), 1_900_000_000),
            Err(Refused::OutOfDate(1))
        );
        assert_eq!(leaf.len(), 91, "a P-256 key");
        assert_eq!(d.attestation_level, Some(SecurityLevel::TrustedEnvironment));
        assert_eq!(d.challenge, b"abc");
        assert!(d.purposes.contains(&2));
        assert_eq!(d.algorithm, Some(3));
        assert_eq!(d.ec_curve, Some(1));
        assert_eq!(d.origin, Some(0));
        assert!(!d.packages.is_empty(), "the sample names its apps");
        assert!(d.signers.iter().all(|s| s.len() == 32), "SHA-256 digests");
        assert_eq!(hex_bytes(RELEASE_SIGNER).len(), 32);
        // This sample key needs no authentication, so it is never pinned.
        assert!(d.no_auth_required);
        assert_eq!(
            check_policy(&d, b"abc", true),
            Err(Refused::Policy(
                "the key does not need the fingerprint or screen lock"
            ))
        );
        assert_eq!(
            check_policy(&d, b"abd", true),
            Err(Refused::Policy("the challenge is not this pin's"))
        );
        // Tampering with any byte the leaf's signature covers breaks it.
        let i = chain[0].len() - 300;
        chain[0][i] ^= 1;
        assert!(verify_chain(&chain, &google_roots(), 1_700_000_000).is_err());
    }

    #[test]
    fn a_chain_to_another_root_or_out_of_order_is_refused() {
        // The sample StrongBox chain's leaf names the third certificate as
        // its issuer, not the second, so it breaks at once.
        assert_eq!(
            verify_chain(&strongbox(), &google_roots(), 1_700_000_000),
            Err(Refused::BadSignature(0))
        );
        // Its last three do chain, but to a root that is not Google's.
        assert_eq!(
            verify_chain(&strongbox()[1..], &google_roots(), 1_700_000_000),
            Err(Refused::UnknownRoot)
        );
        let mut swapped = tee();
        swapped.swap(1, 2);
        assert!(matches!(
            verify_chain(&swapped, &google_roots(), 1_700_000_000),
            Err(Refused::BadSignature(_))
        ));
        assert!(matches!(
            verify_chain(&tee()[..1], &google_roots(), 1_700_000_000),
            Err(Refused::Malformed(_))
        ));
        assert!(matches!(
            verify_chain(
                &[vec![0x30, 0x00], vec![1, 2]],
                &google_roots(),
                1_700_000_000
            ),
            Err(Refused::Malformed(_))
        ));
    }

    #[test]
    fn only_a_hardware_per_use_signing_key_of_ours_passes() {
        let good = KeyDescription {
            attestation_level: Some(SecurityLevel::StrongBox),
            keymint_level: Some(SecurityLevel::StrongBox),
            challenge: b"c".to_vec(),
            purposes: vec![2],
            algorithm: Some(3),
            ec_curve: Some(1),
            no_auth_required: false,
            user_auth_type: Some(3),
            auth_timeout: None,
            origin: Some(0),
            packages: vec!["dev.pocketdesk.android".to_string()],
            signers: vec![hex_bytes(RELEASE_SIGNER)],
        };
        assert_eq!(
            check_policy(&good, b"c", false),
            Ok(SecurityLevel::StrongBox)
        );
        // A build signed with another key: only a debug pin takes it.
        let debug = KeyDescription {
            packages: vec!["dev.pocketdesk.android.debug".to_string()],
            signers: vec![vec![7; 32]],
            ..good.clone()
        };
        assert!(check_policy(&debug, b"c", false).is_err());
        assert!(check_policy(&debug, b"c", true).is_ok());
        for bad in [
            KeyDescription {
                attestation_level: Some(SecurityLevel::Software),
                keymint_level: Some(SecurityLevel::Software),
                ..good.clone()
            },
            KeyDescription {
                purposes: vec![3],
                ..good.clone()
            },
            KeyDescription {
                ec_curve: Some(2),
                ..good.clone()
            },
            KeyDescription {
                origin: Some(2),
                ..good.clone()
            },
            KeyDescription {
                user_auth_type: None,
                ..good.clone()
            },
            KeyDescription {
                auth_timeout: Some(300),
                ..good.clone()
            },
            KeyDescription {
                packages: vec!["dev.pocketdesk.androidx".to_string()],
                ..good.clone()
            },
        ] {
            assert!(check_policy(&bad, b"c", true).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn der_lengths_and_tags_are_bounded() {
        assert_eq!(tlv(&[0x30, 0x85, 1, 1, 1, 1, 1]), None, "five length bytes");
        assert_eq!(tlv(&[0x30, 0x05, 1]), None, "short");
        let high = [0xbf, 0x83, 0x77, 0x02, 0x05, 0x00];
        let (class, constructed, tag, v, rest) = tlv(&high).unwrap();
        assert_eq!(
            (class, constructed, tag, v, rest.len()),
            (2, true, 503, &[0x05, 0x00][..], 0)
        );
        assert_eq!(integer(&[0xff]), Some(-1));
        assert_eq!(integer(&[]), None);
    }
}
