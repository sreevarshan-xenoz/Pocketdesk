//! Approve with my phone: sudo, polkit and the lock screen on Linux. The
//! PAM helper (`pocketdesk-pam-ask`, run by `pam_exec` as `auth sufficient`)
//! makes an [`Approval`] with a fresh nonce; the server puts it to the
//! owner's phones inside BF3's `ask` packet (`"approve": {…}`); a phone whose
//! owner says Yes signs [`signed_message`] with an Android Keystore key that
//! needs the fingerprint or screen lock for each use, and sends the
//! signature in its `ask_answer` (`"signature": "<base64 DER>"`). The helper
//! checks it against the key pinned for that phone by root at opt-in. The
//! server only carries it, and nothing the server or the user's files say
//! can make a signature the helper accepts.
//!
//! The phone gives its public key with [`APPROVE_KEY`] when its owner turns
//! "Approve my computer's requests" on, and takes it back (`null`) when they
//! turn it off.

/// Phone → computer: `{public_key: "<base64 X.509 SubjectPublicKeyInfo>" |
/// null, biometric_only: bool}`.
pub const APPROVE_KEY: &str = "kdeconnect.pocketdesk.approve_key";

/// How long a request waits for the phone when not told.
pub const DEFAULT_TIMEOUT_SECS: u64 = 20;
/// The shortest and longest a request may wait: a short validity.
pub const MIN_TIMEOUT_SECS: u64 = 5;
pub const MAX_TIMEOUT_SECS: u64 = 60;
/// The nonce's length in bytes (sent as lowercase hex).
pub const NONCE_LEN: usize = 32;
/// The first line of every signed message: a signature for anything else
/// (another protocol, a later version) is never one for this.
pub const SIGNING_DOMAIN: &str = "pocketdesk-approve-v1";
/// The most characters of a command shown and signed.
pub const MAX_COMMAND: usize = 300;
/// A DER ECDSA P-256 signature is at most 72 bytes: 96 in base64; a little
/// room on top.
const MAX_SIGNATURE_B64: usize = 128;
/// A P-256 SubjectPublicKeyInfo is 91 bytes: 124 in base64.
const MAX_KEY_B64: usize = 256;

/// Where the packages install the PAM helper; the PAM lines name it.
pub const HELPER_PATH: &str = "/usr/lib/pocketdesk/pocketdesk-pam-ask";
/// Where root pins each user's phones' keys (`<user>.json`).
pub const PIN_DIR: &str = "/etc/pocketdesk/pam";

/// The PAM services Settings offers lines for, and what each approves.
pub const PAM_SERVICES: [(&str, Kind); 7] = [
    ("sudo", Kind::Admin),
    ("polkit-1", Kind::Admin),
    ("hyprlock", Kind::Unlock),
    ("swaylock", Kind::Unlock),
    ("kde", Kind::Unlock),
    ("gtklock", Kind::Unlock),
    ("gdm-password", Kind::Unlock),
];

/// The line to put first among a service's `auth` lines: `sufficient`
/// only, so a failure falls through to the password.
pub fn pam_line() -> String {
    format!("auth    sufficient    pam_exec.so quiet stdout {HELPER_PATH}")
}

/// What is being approved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// sudo, polkit: administrator rights.
    Admin,
    /// A lock screen (hyprlock, swaylock, KDE's, GNOME's).
    Unlock,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Admin => "admin",
            Kind::Unlock => "unlock",
        }
    }
}

/// One request for the phone to sign, every field of which it signs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Approval {
    /// The ask's id (BF3's form: letters, digits and `-`).
    pub id: String,
    pub kind: Kind,
    /// The PAM service (`sudo`, `polkit-1`, `hyprlock`, …).
    pub service: String,
    /// The user being authenticated (`PAM_USER`).
    pub user: String,
    /// What runs, as far as the helper can tell (sudo's command line), or
    /// empty. Cleaned as a question is, at most [`MAX_COMMAND`] characters.
    pub command: String,
    /// The computer: lowercase hex SHA-256 of its certificate, which the
    /// phone pinned when it paired. The phone checks it is the computer the
    /// request came from.
    pub computer: String,
    /// [`NONCE_LEN`] random bytes from the helper, lowercase hex.
    pub nonce: String,
    pub issued_unix: u64,
    pub expires_unix: u64,
}

impl Approval {
    /// The approval as it rides in the `ask` packet.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    /// The words a phone too old to sign would show (the ask's `question`).
    pub fn summary(&self) -> String {
        let said = match self.kind {
            Kind::Unlock => format!("Unlock the screen for {}?", self.user),
            Kind::Admin if self.command.is_empty() => {
                format!("{} for {}: administrator rights?", self.service, self.user)
            }
            Kind::Admin => format!("{} for {}: {}?", self.service, self.user, self.command),
        };
        super::ask::clean_question(&said).unwrap_or_else(|| "Approve?".to_string())
    }
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether `name` can be a PAM service or a user name: short, of ASCII
/// letters, digits, `.`, `_` and `-`, not starting with `-`.
pub fn is_plain_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// A command as it is shown and signed: one line, at most [`MAX_COMMAND`]
/// characters; empty for none.
pub fn clean_command(command: &str) -> String {
    let flat: String = command
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if flat.chars().count() <= MAX_COMMAND {
        return flat;
    }
    let kept: String = flat.chars().take(MAX_COMMAND - 1).collect();
    format!("{}…", kept.trim_end())
}

/// The approval an `ask` body carries in `approve`, checked field by field;
/// `None` for none or anything malformed.
pub fn approval_of(body: &serde_json::Value) -> Option<Approval> {
    let approval: Approval = serde_json::from_value(body.get("approve")?.clone()).ok()?;
    let wait = approval.expires_unix.checked_sub(approval.issued_unix)?;
    let ok = super::ask::is_ask_id(&approval.id)
        && is_plain_name(&approval.service)
        && is_plain_name(&approval.user)
        && clean_command(&approval.command) == approval.command
        && is_hex(&approval.computer, 64)
        && is_hex(&approval.nonce, NONCE_LEN * 2)
        && (1..=MAX_TIMEOUT_SECS).contains(&wait);
    ok.then_some(approval)
}

/// The exact bytes a Yes signs: [`SIGNING_DOMAIN`], then each field in a
/// fixed order, each written `<length in UTF-8 bytes>:<value>\n` so no two
/// requests read alike, then `yes`. The Android app builds the same bytes
/// (`ApproveRule.message`), and both sides' tests pin one example.
pub fn signed_message(approval: &Approval) -> Vec<u8> {
    let issued = approval.issued_unix.to_string();
    let expires = approval.expires_unix.to_string();
    let fields: [&str; 11] = [
        SIGNING_DOMAIN,
        &approval.id,
        approval.kind.word(),
        &approval.service,
        &approval.user,
        &approval.command,
        &approval.computer,
        &approval.nonce,
        &issued,
        &expires,
        "yes",
    ];
    let mut out = Vec::new();
    for field in fields {
        out.extend_from_slice(field.len().to_string().as_bytes());
        out.push(b':');
        out.extend_from_slice(field.as_bytes());
        out.push(b'\n');
    }
    out
}

/// The signature an `ask_answer` body carries, base64 as sent; `None` when
/// it has none or one too long to be a P-256 signature.
pub fn signature_of(body: &serde_json::Value) -> Option<String> {
    let sig = body.get("signature")?.as_str()?;
    let ok = (1..=MAX_SIGNATURE_B64).contains(&sig.len())
        && sig
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='));
    ok.then(|| sig.to_string())
}

/// The answer body with a signature (a Yes to an approval).
pub fn signed_answer_body(id: &str, signature_b64: &str) -> serde_json::Value {
    serde_json::json!({ "id": id, "answer": "yes", "signature": signature_b64 })
}

/// What an [`APPROVE_KEY`] packet says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyMessage {
    /// The phone's public key (base64 SubjectPublicKeyInfo), and whether it
    /// takes only a fingerprint (not the PIN).
    Set {
        public_key: String,
        biometric_only: bool,
    },
    /// The phone turned it off.
    Clear,
}

/// The key packet's body.
pub fn key_body(message: &KeyMessage) -> serde_json::Value {
    match message {
        KeyMessage::Set {
            public_key,
            biometric_only,
        } => serde_json::json!({ "public_key": public_key, "biometric_only": biometric_only }),
        KeyMessage::Clear => serde_json::json!({ "public_key": null }),
    }
}

/// What an [`APPROVE_KEY`] body says; `None` for anything malformed.
pub fn key_of(body: &serde_json::Value) -> Option<KeyMessage> {
    match body.get("public_key")? {
        serde_json::Value::Null => Some(KeyMessage::Clear),
        serde_json::Value::String(key) => {
            let ok = (1..=MAX_KEY_B64).contains(&key.len())
                && key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='));
            ok.then(|| KeyMessage::Set {
                public_key: key.clone(),
                biometric_only: body
                    .get("biometric_only")
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Approval {
        Approval {
            id: "a-1".to_string(),
            kind: Kind::Admin,
            service: "sudo".to_string(),
            user: "alice".to_string(),
            command: "sudo pacman -Syu".to_string(),
            computer: "ab".repeat(32),
            nonce: "01".repeat(32),
            issued_unix: 1_000,
            expires_unix: 1_020,
        }
    }

    #[test]
    fn the_signed_message_is_pinned_byte_for_byte() {
        // The Android app's ApproveRuleTest pins the same text.
        let text = String::from_utf8(signed_message(&sample())).unwrap();
        let expected = format!(
            "21:pocketdesk-approve-v1\n3:a-1\n5:admin\n4:sudo\n5:alice\n16:sudo pacman -Syu\n64:{}\n64:{}\n4:1000\n4:1020\n3:yes\n",
            "ab".repeat(32),
            "01".repeat(32)
        );
        assert_eq!(text, expected);
        // Lengths are bytes, so a field cannot spill into the next.
        let mut moved = sample();
        moved.command = "sudo pacman".to_string();
        moved.user = "alice\n4:x".to_string();
        assert_ne!(signed_message(&moved), signed_message(&sample()));
    }

    #[test]
    fn an_approval_rides_in_the_ask_and_is_checked() {
        let a = sample();
        let body = json!({ "id": "a-1", "question": a.summary(), "approve": a.to_json() });
        assert_eq!(approval_of(&body), Some(a.clone()));
        assert_eq!(a.summary(), "sudo for alice: sudo pacman -Syu?");
        for broken in [
            json!({"service": "sudo; rm"}),
            json!({"user": "-x"}),
            json!({"computer": "AB".repeat(32)}),
            json!({"nonce": "01"}),
            json!({"command": "two\nlines"}),
            json!({"expires_unix": 999}),
            json!({"expires_unix": 1_000 + MAX_TIMEOUT_SECS + 1}),
            json!({"kind": "root"}),
        ] {
            let mut v = a.to_json();
            for (k, val) in broken.as_object().unwrap() {
                v[k] = val.clone();
            }
            assert_eq!(approval_of(&json!({ "approve": v })), None, "{broken}");
        }
        assert_eq!(approval_of(&json!({ "id": "a-1" })), None);
    }

    #[test]
    fn a_signature_and_a_key_are_read_or_refused() {
        assert_eq!(
            signature_of(&signed_answer_body("a-1", "MEUCIQ+/=")),
            Some("MEUCIQ+/=".to_string())
        );
        assert_eq!(signature_of(&json!({ "signature": "a b" })), None);
        assert_eq!(signature_of(&json!({ "signature": "A".repeat(200) })), None);
        assert_eq!(signature_of(&json!({ "id": "a-1" })), None);
        let set = KeyMessage::Set {
            public_key: "MFkw".to_string(),
            biometric_only: true,
        };
        assert_eq!(key_of(&key_body(&set)), Some(set));
        assert_eq!(
            key_of(&key_body(&KeyMessage::Clear)),
            Some(KeyMessage::Clear)
        );
        assert_eq!(key_of(&json!({ "public_key": 3 })), None);
        assert_eq!(key_of(&json!({})), None);
    }

    #[test]
    fn a_command_is_one_short_line() {
        assert_eq!(clean_command(" sudo\n pacman\t-Syu "), "sudo pacman -Syu");
        assert_eq!(clean_command(""), "");
        let long = clean_command(&"x".repeat(MAX_COMMAND + 9));
        assert_eq!(long.chars().count(), MAX_COMMAND);
    }
}
