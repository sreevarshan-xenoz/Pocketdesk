//! Approve with my phone, the server's part: it carries requests from
//! `pocketdesk-pam-ask` to the linked phones and their answers back, and
//! keeps each phone's approval key as it gave it. It decides nothing: the
//! helper checks the phone's signature itself, against the key root pinned
//! (`crate::pam_ask`), so a server or a state folder someone else wrote to
//! can delay a request or drop it, never approve one.
//!
//! A request goes to a phone only when the phone is linked, has the Ask
//! permission, is one the request names (its key pinned by root), and gave
//! this server a key; when none is, the server says so at once
//! ([`Status::NoPhone`]) and the helper gives up, so the password prompt
//! shows without a wait.
//!
//! Also the signature check itself ([`verify`]), shared with the helper.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use base64::Engine as _;
use pocketdesk_core::local::approve::{self as files, PhoneKey, Request, Status};
use pocketdesk_core::protocol::approve::{Approval, KeyMessage, key_of, signed_message};
use pocketdesk_core::protocol::ask::{ASK, Answer, ask_body};
use pocketdesk_core::protocol::kdeconnect::NetworkPacket;
use pocketdesk_core::protocol::permissions::Capability;

/// The DER prefix of an EC P-256 SubjectPublicKeyInfo (as Android's
/// `PublicKey.getEncoded()` gives it): the algorithm (id-ecPublicKey,
/// prime256v1) and the bit string's header, then the 65-byte uncompressed
/// point.
const P256_SPKI_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

/// The uncompressed P-256 point in a base64 SubjectPublicKeyInfo; `None`
/// for anything else (another curve, a compressed point, junk).
pub fn p256_point(public_key_b64: &str) -> Option<Vec<u8>> {
    let der = base64::engine::general_purpose::STANDARD
        .decode(public_key_b64)
        .ok()?;
    let point = der.strip_prefix(&P256_SPKI_PREFIX[..])?;
    (point.len() == 65 && point[0] == 0x04).then(|| point.to_vec())
}

/// What both apps show to compare a key: the first 16 hex digits of the
/// SHA-256 of its SubjectPublicKeyInfo, in groups of four.
pub fn key_fingerprint(public_key_b64: &str) -> Option<String> {
    let der = base64::engine::general_purpose::STANDARD
        .decode(public_key_b64)
        .ok()?;
    let hash = ring::digest::digest(&ring::digest::SHA256, &der);
    let hex: String = hash.as_ref()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Some(
        hex.as_bytes()
            .chunks(4)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// Why an answer is not an approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The phone said No.
    Declined,
    /// Yes with no signature (an app too old to sign).
    Unsigned,
    /// From a phone whose key is not pinned.
    UnknownPhone,
    /// The signature is not this request's from that phone's key: a
    /// replay of another request's, another key's, or one over other words.
    BadSignature,
    /// After the request's time.
    Late,
}

/// Whether `device_id`'s answer approves `approval`: a Yes, in time
/// (`now_unix` before its end), signed over this very request (its nonce,
/// computer, service, user and command) by the key pinned for that phone.
pub fn verify(
    approval: &Approval,
    device_id: &str,
    answer: Answer,
    signature_b64: Option<&str>,
    pinned: &[PhoneKey],
    now_unix: u64,
) -> Result<(), Refusal> {
    if answer != Answer::Yes {
        return Err(Refusal::Declined);
    }
    if now_unix >= approval.expires_unix {
        return Err(Refusal::Late);
    }
    let signature = signature_b64.ok_or(Refusal::Unsigned)?;
    let key = pinned
        .iter()
        .find(|k| k.device_id == device_id)
        .ok_or(Refusal::UnknownPhone)?;
    let point = p256_point(&key.public_key).ok_or(Refusal::UnknownPhone)?;
    let signature = base64::engine::general_purpose::STANDARD
        .decode(signature)
        .map_err(|_| Refusal::BadSignature)?;
    ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_ASN1, point)
        .verify(&signed_message(approval), &signature)
        .map_err(|_| Refusal::BadSignature)
}

/// A request put to the phones.
struct Pending {
    request: Request,
    asked: HashSet<String>,
    sent_on: HashSet<String>,
}

static PENDING: LazyLock<Mutex<HashMap<String, Pending>>> = LazyLock::new(Default::default);

fn now_unix() -> u64 {
    pocketdesk_core::local::pin::now_unix()
}

/// Whether request `id` is one of these (the ask plugin's answers go here).
pub fn is_pending(id: &str) -> bool {
    PENDING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(id)
}

/// The phones `request` may go to: the ones it names that gave this
/// server a key.
fn named_with_key(request: &Request, keys: &[PhoneKey]) -> HashSet<String> {
    request
        .phones
        .iter()
        .filter(|p| keys.iter().any(|k| &k.device_id == *p))
        .cloned()
        .collect()
}

/// Put the requests left in `approve_out/` to the phones, for the server's
/// life.
pub async fn run() {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    loop {
        let waiting = !PENDING.lock().unwrap_or_else(|e| e.into_inner()).is_empty();
        if waiting {
            tokio::time::sleep(Duration::from_millis(250)).await;
        } else {
            crate::inbox::wait(
                pocketdesk_core::local::paths::APPROVE_DIR,
                Duration::from_millis(250),
            )
            .await;
        }
        turn(&dir).await;
    }
}

async fn turn(dir: &Path) {
    let now = now_unix();
    let taken = files::take_all(dir, now);
    let on = crate::events::current_config().phone_approves;
    let keys = files::keys(dir);
    let clients = crate::state::clone_clients().await;
    let linked = |device: &str| {
        clients.iter().any(|c| c.device_id() == device)
            && crate::plugins::pair::allows(device, Capability::Ask)
    };
    let mut sends = Vec::new();
    {
        let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        for request in taken {
            let id = request.approval.id.clone();
            let reachable = named_with_key(&request, &keys)
                .into_iter()
                .filter(|d| linked(d))
                .count();
            if !on || reachable == 0 {
                tracing::info!("An approval request found no linked phone");
                let _ = files::set_status(dir, &id, &Status::NoPhone);
                continue;
            }
            let _ = files::set_status(dir, &id, &Status::Asked { phones: reachable });
            pending.insert(
                id,
                Pending {
                    request,
                    asked: HashSet::new(),
                    sent_on: HashSet::new(),
                },
            );
        }
        pending.retain(|id, p| {
            let open = now < p.request.approval.expires_unix;
            if !open {
                files::withdraw(dir, id);
            }
            open
        });
        for (id, p) in pending.iter() {
            let named = named_with_key(&p.request, &keys);
            for client in &clients {
                let device = client.device_id();
                if named.contains(device)
                    && !p.sent_on.contains(client.connection_id())
                    && crate::plugins::pair::allows(device, Capability::Ask)
                {
                    sends.push((id.clone(), client.clone(), p.request.approval.clone()));
                }
            }
        }
    }
    for (id, client, approval) in sends {
        let mut body = ask_body(
            &approval.id,
            &approval.summary(),
            approval.expires_unix,
            now_unix(),
        );
        body["approve"] = approval.to_json();
        let sent = client
            .send_packet(&NetworkPacket::new(ASK).with_body(body))
            .await;
        let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        let Some(p) = pending.get_mut(&id) else {
            continue;
        };
        // Never the request's words.
        match sent {
            Ok(()) => {
                tracing::info!("An approval request was put to a phone");
                p.asked.insert(client.device_id().to_string());
                p.sent_on.insert(client.connection_id().to_string());
            }
            Err(e) => tracing::warn!("An approval request was not put to a phone: {e}"),
        }
    }
}

/// Pass `device`'s answer to request `id` to the helper if the request was
/// put to that phone and still waits; the first answer ends it. Whether it
/// was passed.
pub fn heard(
    dir: &Path,
    device: &str,
    id: &str,
    answer: Answer,
    signature: Option<String>,
    now: u64,
) -> bool {
    let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    let open = pending
        .get(id)
        .is_some_and(|p| p.asked.contains(device) && now < p.request.approval.expires_unix);
    if !open {
        return false;
    }
    pending.remove(id);
    let status = Status::Answered {
        device_id: device.to_string(),
        answer,
        signature,
    };
    match files::set_status(dir, id, &status) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("An approval answer could not be passed on: {e}");
            false
        }
    }
}

/// Keep (or forget) `device`'s approval key, from its `approve_key` packet.
/// Only a paired phone with the Ask permission; a key that is not P-256 is
/// refused.
pub fn key_heard(dir: &Path, device: &str, body: &serde_json::Value, now: u64) {
    if !crate::plugins::pair::allows(device, Capability::Ask) {
        tracing::info!("An approval key was dropped: the phone may not answer questions");
        return;
    }
    let key = match key_of(body) {
        Some(KeyMessage::Set {
            public_key,
            biometric_only,
        }) => {
            let (Some(_), Some(fingerprint)) =
                (p256_point(&public_key), key_fingerprint(&public_key))
            else {
                tracing::warn!("An approval key was refused: not an EC P-256 key");
                return;
            };
            Some(PhoneKey {
                device_id: device.to_string(),
                public_key,
                biometric_only,
                fingerprint,
                added_unix: now,
            })
        }
        Some(KeyMessage::Clear) => None,
        None => return,
    };
    let set = key.is_some();
    match files::set_key(dir, device, key) {
        Ok(true) if set => tracing::info!("A phone gave its approval key"),
        Ok(true) => tracing::info!("A phone took its approval key back"),
        Ok(false) => {}
        Err(e) => tracing::warn!("An approval key could not be kept: {e}"),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use pocketdesk_core::protocol::approve::Kind;
    use ring::rand::SystemRandom;
    use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};

    /// A phone's key, as the Keystore would make one.
    pub(crate) struct TestPhone {
        pair: EcdsaKeyPair,
        pub(crate) device_id: String,
    }

    impl TestPhone {
        pub(crate) fn new(device_id: &str) -> Self {
            let rng = SystemRandom::new();
            let pkcs8 =
                EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
            let pair =
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
                    .unwrap();
            Self {
                pair,
                device_id: device_id.to_string(),
            }
        }

        pub(crate) fn spki_b64(&self) -> String {
            let mut der = P256_SPKI_PREFIX.to_vec();
            der.extend_from_slice(self.pair.public_key().as_ref());
            base64::engine::general_purpose::STANDARD.encode(der)
        }

        pub(crate) fn key(&self) -> PhoneKey {
            PhoneKey {
                device_id: self.device_id.clone(),
                public_key: self.spki_b64(),
                biometric_only: false,
                fingerprint: key_fingerprint(&self.spki_b64()).unwrap(),
                added_unix: 1,
            }
        }

        pub(crate) fn sign(&self, approval: &Approval) -> String {
            let sig = self
                .pair
                .sign(&SystemRandom::new(), &signed_message(approval))
                .unwrap();
            base64::engine::general_purpose::STANDARD.encode(sig.as_ref())
        }
    }

    pub(crate) fn approval(nonce_byte: &str) -> Approval {
        Approval {
            id: "r-1".to_string(),
            kind: Kind::Admin,
            service: "sudo".to_string(),
            user: "alice".to_string(),
            command: "sudo pacman -Syu".to_string(),
            computer: "ab".repeat(32),
            nonce: nonce_byte.repeat(32),
            issued_unix: 1_000,
            expires_unix: 1_020,
        }
    }

    #[test]
    fn a_yes_signed_over_this_request_by_the_pinned_key_approves() {
        let phone = TestPhone::new("phone-a");
        let a = approval("01");
        let sig = phone.sign(&a);
        let pinned = [phone.key()];
        assert_eq!(
            verify(&a, "phone-a", Answer::Yes, Some(&sig), &pinned, 1_005),
            Ok(())
        );
    }

    #[test]
    fn a_replayed_signature_is_refused() {
        // A signature for an earlier request (another nonce) is not one for
        // this.
        let phone = TestPhone::new("phone-a");
        let earlier = approval("01");
        let sig = phone.sign(&earlier);
        let now = approval("02");
        assert_eq!(
            verify(
                &now,
                "phone-a",
                Answer::Yes,
                Some(&sig),
                &[phone.key()],
                1_005
            ),
            Err(Refusal::BadSignature)
        );
    }

    #[test]
    fn a_signature_by_another_key_is_refused() {
        let phone = TestPhone::new("phone-a");
        let thief = TestPhone::new("phone-a");
        let a = approval("01");
        assert_eq!(
            verify(
                &a,
                "phone-a",
                Answer::Yes,
                Some(&thief.sign(&a)),
                &[phone.key()],
                1_005
            ),
            Err(Refusal::BadSignature)
        );
        // A phone whose key root never pinned.
        assert_eq!(
            verify(
                &a,
                "phone-b",
                Answer::Yes,
                Some(&phone.sign(&a)),
                &[phone.key()],
                1_005
            ),
            Err(Refusal::UnknownPhone)
        );
    }

    #[test]
    fn a_signature_over_another_request_is_refused() {
        let phone = TestPhone::new("phone-a");
        let shown = approval("01");
        let sig = phone.sign(&shown);
        for changed in [
            Approval {
                command: "sudo rm -rf /".to_string(),
                ..shown.clone()
            },
            Approval {
                user: "root".to_string(),
                ..shown.clone()
            },
            Approval {
                computer: "cd".repeat(32),
                ..shown.clone()
            },
            Approval {
                kind: Kind::Unlock,
                ..shown.clone()
            },
            Approval {
                expires_unix: 1_060,
                ..shown.clone()
            },
        ] {
            assert_eq!(
                verify(
                    &changed,
                    "phone-a",
                    Answer::Yes,
                    Some(&sig),
                    &[phone.key()],
                    1_005
                ),
                Err(Refusal::BadSignature)
            );
        }
    }

    #[test]
    fn a_late_a_no_or_an_unsigned_answer_is_refused() {
        let phone = TestPhone::new("phone-a");
        let a = approval("01");
        let sig = phone.sign(&a);
        let pinned = [phone.key()];
        assert_eq!(
            verify(&a, "phone-a", Answer::Yes, Some(&sig), &pinned, 1_020),
            Err(Refusal::Late)
        );
        assert_eq!(
            verify(&a, "phone-a", Answer::No, Some(&sig), &pinned, 1_005),
            Err(Refusal::Declined)
        );
        assert_eq!(
            verify(&a, "phone-a", Answer::Yes, None, &pinned, 1_005),
            Err(Refusal::Unsigned)
        );
        assert_eq!(
            verify(&a, "phone-a", Answer::Yes, Some("@@"), &pinned, 1_005),
            Err(Refusal::BadSignature)
        );
    }

    #[test]
    fn only_a_p256_key_is_taken() {
        let phone = TestPhone::new("phone-a");
        assert!(p256_point(&phone.spki_b64()).is_some());
        assert_eq!(p256_point("MFkw"), None);
        // A P-384 header is not this one.
        let mut other = P256_SPKI_PREFIX.to_vec();
        other[19] = 0x22;
        other.extend_from_slice(&[4u8; 65]);
        assert_eq!(
            p256_point(&base64::engine::general_purpose::STANDARD.encode(other)),
            None
        );
        let fp = key_fingerprint(&phone.spki_b64()).unwrap();
        assert_eq!(fp.len(), 19);
        assert_eq!(fp.split(' ').count(), 4);
    }

    #[test]
    fn an_answer_is_passed_on_once_and_only_from_a_phone_asked() {
        let dir = std::env::temp_dir().join(format!("pd-approve-srv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut a = approval("01");
        a.id = "srv-1".to_string();
        PENDING.lock().unwrap().insert(
            a.id.clone(),
            Pending {
                request: Request {
                    approval: a.clone(),
                    phones: vec!["phone-a".to_string()],
                },
                asked: ["phone-a".to_string()].into(),
                sent_on: HashSet::new(),
            },
        );
        assert!(is_pending("srv-1"));
        assert!(
            !heard(&dir, "phone-b", "srv-1", Answer::Yes, None, 1_005),
            "not asked"
        );
        assert!(
            !heard(&dir, "phone-a", "srv-1", Answer::Yes, None, 1_020),
            "late"
        );
        assert!(heard(&dir, "phone-a", "srv-1", Answer::No, None, 1_005));
        assert!(
            !heard(&dir, "phone-a", "srv-1", Answer::Yes, None, 1_005),
            "once"
        );
        assert_eq!(
            files::status(&dir, "srv-1"),
            Some(Status::Answered {
                device_id: "phone-a".to_string(),
                answer: Answer::No,
                signature: None
            })
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
