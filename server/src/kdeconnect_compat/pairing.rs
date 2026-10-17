//! Pairing with a KDE Connect device (3b), as its protocol has it (design
//! §"The protocol" 6): `kdeconnect.pair {pair: true, timestamp}` asks,
//! `{pair: true}` answers yes, `{pair: false}` no or unpairs. Both screens
//! show a code from the two certificates' public keys and the timestamp, so
//! the owner can tell this device from another asking at the same time.

use pocketdesk_core::protocol::kdeconnect::NetworkPacket;

/// The packet type, both ways.
pub const PAIR: &str = "kdeconnect.pair";

/// The most a request's clock may be off from this one's, in seconds.
pub const MAX_SKEW_SECS: u64 = 1800;

/// What a pair packet says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairSays {
    /// Pair (a request, or yes to ours); the request's timestamp if it has one.
    Yes(Option<u64>),
    /// No, or unpair.
    No,
}

/// Read a pair packet; `None` for one that says neither.
pub fn read(packet: &NetworkPacket) -> Option<PairSays> {
    if packet.packet_type != PAIR {
        return None;
    }
    match packet.body.get("pair").and_then(|v| v.as_bool())? {
        true => Some(PairSays::Yes(
            packet.body.get("timestamp").and_then(|v| v.as_u64()),
        )),
        false => Some(PairSays::No),
    }
}

/// A request, with `timestamp`.
pub fn ask(timestamp: u64) -> NetworkPacket {
    NetworkPacket::new(PAIR).with_body(serde_json::json!({"pair": true, "timestamp": timestamp}))
}

/// The answer, or an unpairing.
pub fn answer(yes: bool) -> NetworkPacket {
    NetworkPacket::new(PAIR).with_body(serde_json::json!({"pair": yes}))
}

/// Whether a request's `timestamp` is near enough to `now`.
pub fn timestamp_ok(timestamp: u64, now: u64) -> bool {
    timestamp.abs_diff(now) <= MAX_SKEW_SECS
}

/// A certificate's public key, as DER (its SubjectPublicKeyInfo).
pub fn public_key_der(cert_der: &[u8]) -> Option<Vec<u8>> {
    let (_, cert) = x509_parser::parse_x509_certificate(cert_der).ok()?;
    Some(cert.public_key().raw.to_vec())
}

/// The code both screens show: the first 8 hex digits, upper case, of the
/// SHA-256 of the larger public key's DER, then the smaller's, then the
/// timestamp in decimal.
pub fn code(key_a: &[u8], key_b: &[u8], timestamp: u64) -> String {
    let (larger, smaller) = if key_a > key_b {
        (key_a, key_b)
    } else {
        (key_b, key_a)
    };
    let mut data = Vec::with_capacity(larger.len() + smaller.len() + 20);
    data.extend_from_slice(larger);
    data.extend_from_slice(smaller);
    data.extend_from_slice(timestamp.to_string().as_bytes());
    let digest = ring::digest::digest(&ring::digest::SHA256, &data);
    digest.as_ref()[..4]
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_code_matches_one_worked_out_apart_and_either_order() {
        // From Python's hashlib over the same bytes, written down when this
        // was written: sha256(bytes(range(1,41)) + bytes(range(0,40)) +
        // b"1792194097").hexdigest()[:8].upper().
        let a: Vec<u8> = (0..40).collect();
        let b: Vec<u8> = (1..41).collect();
        assert_eq!(code(&a, &b, 1_792_194_097), "1ACBAC75");
        assert_eq!(
            code(&b, &a, 1_792_194_097),
            "1ACBAC75",
            "either side computes it"
        );
        assert_ne!(
            code(&a, &b, 1_792_194_098),
            "1ACBAC75",
            "the timestamp counts"
        );
    }

    #[test]
    fn keys_are_ordered_by_unsigned_bytes() {
        // A first byte of 0x80 is larger than 0x7f only unsigned: a signed
        // comparison would put the other key first. From Python:
        // sha256(bytes([0x80,0,0,0]) + bytes([0x7f,1,2,3]) +
        // b"1792194097").hexdigest()[:8].upper(); the other order gives
        // E2343559.
        let high = [0x80, 0, 0, 0];
        let low = [0x7f, 1, 2, 3];
        assert_eq!(code(&low, &high, 1_792_194_097), "074479E5");
        assert_eq!(code(&high, &low, 1_792_194_097), "074479E5");
    }

    #[test]
    fn a_requests_clock_may_be_off_half_an_hour() {
        assert!(timestamp_ok(1_000_000, 1_000_000 + MAX_SKEW_SECS));
        assert!(timestamp_ok(1_000_000 + MAX_SKEW_SECS, 1_000_000));
        assert!(!timestamp_ok(1_000_000, 1_000_000 + MAX_SKEW_SECS + 1));
    }

    #[test]
    fn a_pair_packet_says_yes_with_its_time_or_no() {
        assert_eq!(read(&ask(5)), Some(PairSays::Yes(Some(5))));
        assert_eq!(read(&answer(true)), Some(PairSays::Yes(None)));
        assert_eq!(read(&answer(false)), Some(PairSays::No));
        assert_eq!(read(&NetworkPacket::new(PAIR)), None);
        assert_eq!(read(&NetworkPacket::new("kdeconnect.ping")), None);
    }

    #[test]
    fn a_certificates_public_key_is_its_spki() {
        let dir = std::env::temp_dir().join(format!(
            "pd-kde-spki-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let identity = super::super::identity::CompatIdentity::load_or_create(&dir).unwrap();
        let key = public_key_der(&identity.cert_der).unwrap();
        // An EC P-256 SubjectPublicKeyInfo: 91 bytes, a SEQUENCE.
        assert_eq!(key.len(), 91);
        assert_eq!(key[0], 0x30);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
