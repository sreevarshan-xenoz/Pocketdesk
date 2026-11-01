//! Approve with my phone: sudo, polkit and the lock screen on Linux. The
//! PAM helper (`pocketdesk-pam-ask`, run by `pam_exec` as `auth sufficient`)
//! makes an [`Approval`] with a fresh nonce; the server puts it to the
//! owner's phones inside BF3's `ask` packet (`"approve": {…}`); a phone whose
//! owner says Yes signs [`signed_message`] with an Android Keystore key that
//! needs the fingerprint or screen lock for each use, and sends the
//! signature in its `ask_answer` (`"signature": "<base64 DER>"`). The helper
//! checks it against the key root pinned for that phone. The server only
//! carries it, and nothing the server or the user's files say can make a
//! signature the helper accepts.
//!
//! Pinning is a request too ([`Kind::Pin`]): root's `--pin` asks the phone,
//! which makes its key with the request's hash as the Key Attestation
//! challenge and answers with the attestation chain
//! (`"attestation": ["<base64 DER>", …]`), which root checks up to Google's
//! roots before it pins the key.
//!
//! Everything a phone shows is in the signed bytes, and every text field is
//! printable ASCII: anything else (invisible, bidi or control characters, or
//! any non-ASCII letter) is written as `⟨U+XXXX⟩` ([`shown_text`]).
//!
//! The phone also gives its public key with [`APPROVE_KEY`] when its owner
//! turns "Approve my computer's requests" on, for Settings to list; that key
//! is advisory and never pinned.

/// Phone → computer: `{public_key: "<base64 X.509 SubjectPublicKeyInfo>" |
/// null, biometric_only: bool}`.
pub const APPROVE_KEY: &str = "kdeconnect.pocketdesk.approve_key";

/// How long a request waits for the phone when not told.
pub const DEFAULT_TIMEOUT_SECS: u64 = 20;
/// The shortest and longest a request may wait: a short validity.
pub const MIN_TIMEOUT_SECS: u64 = 5;
pub const MAX_TIMEOUT_SECS: u64 = 60;
/// How long a pin request waits: the owner reads and touches the phone.
pub const PIN_TIMEOUT_SECS: u64 = 60;
/// The nonce's length in bytes (sent as lowercase hex).
pub const NONCE_LEN: usize = 32;
/// The first line of every signed message: a signature for anything else
/// (another protocol, an earlier version) is never one for this.
pub const SIGNING_DOMAIN: &str = "pocketdesk-approve-v2";
/// The most characters of a command shown and signed, once escaped. A
/// longer one is never cut: the helper does not ask, and the password
/// prompt comes.
pub const MAX_COMMAND: usize = 4096;
/// The most characters of the asking processes' chain, of the tty and of a
/// polkit action.
pub const MAX_ORIGIN: usize = 512;
pub const MAX_TTY: usize = 64;
pub const MAX_ACTION: usize = 256;
/// The PAM service a pin request names.
pub const PIN_SERVICE: &str = "pocketdesk-pin";
/// A DER ECDSA P-256 signature is at most 72 bytes: 96 in base64; a little
/// room on top.
const MAX_SIGNATURE_B64: usize = 128;
/// A P-256 SubjectPublicKeyInfo is 91 bytes: 124 in base64.
const MAX_KEY_B64: usize = 256;
/// An attestation chain: at most this many certificates, each at most this
/// long in base64.
pub const MAX_CHAIN: usize = 8;
const MAX_CERT_B64: usize = 16 * 1024;

/// Where the packages install the PAM helper; the PAM lines name it.
pub const HELPER_PATH: &str = "/usr/lib/pocketdesk/pocketdesk-pam-ask";
/// Where root pins each user's phones' keys (`<user>.json`).
pub const PIN_DIR: &str = "/etc/pocketdesk/pam";

/// The PAM services Settings offers lines for, and what each approves.
/// Not `gdm-password`: GNOME's greeter logs in with it too.
pub const PAM_SERVICES: [(&str, Kind); 6] = [
    ("sudo", Kind::Admin),
    ("polkit-1", Kind::Admin),
    ("hyprlock", Kind::Unlock),
    ("swaylock", Kind::Unlock),
    ("kde", Kind::Unlock),
    ("gtklock", Kind::Unlock),
];

/// The line to add to a service's `auth` lines, after `pam_faillock.so
/// preauth` where the distribution has it and before `pam_unix`:
/// `sufficient` only, so a failure falls through to the password.
pub fn pam_line() -> String {
    format!("auth    sufficient    pam_exec.so quiet stdout {HELPER_PATH}")
}

/// What is being approved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// sudo, polkit: administrator rights.
    Admin,
    /// A lock screen (hyprlock, swaylock, KDE's).
    Unlock,
    /// Root pinning this phone's key: the phone makes the key, attested.
    Pin,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Admin => "admin",
            Kind::Unlock => "unlock",
            Kind::Pin => "pin",
        }
    }
}

/// One request for the phone to sign, every field of which it signs and
/// shows.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Approval {
    /// The ask's id (BF3's form: letters, digits and `-`).
    pub id: String,
    pub kind: Kind,
    /// The PAM service (`sudo`, `polkit-1`, `hyprlock`, …).
    pub service: String,
    /// The user being authenticated (`PAM_USER`).
    pub user: String,
    /// The user who asked (`PAM_RUSER`), or empty when PAM was not told.
    #[serde(default)]
    pub requester: String,
    /// polkit's action, or empty when it does not say (PAM is not told).
    #[serde(default)]
    pub action: String,
    /// What runs: the asking program's executable and its arguments
    /// (never its `argv[0]`), [`shown_text`]; empty when not known.
    pub command: String,
    /// The terminal it was asked from (`PAM_TTY`), [`shown_text`].
    #[serde(default)]
    pub tty: String,
    /// The asking process and its parents, `name(pid) < name(pid) …`,
    /// [`shown_text`], so two requests at once can be told apart.
    #[serde(default)]
    pub origin: String,
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
            Kind::Pin => format!("Let this phone approve for {}?", self.user),
            Kind::Admin => format!("{} for {}: administrator rights?", self.service, self.user),
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

/// The escape's brackets: `⟨U+202E⟩`.
const OPEN: char = '\u{27E8}';
const CLOSE: char = '\u{27E9}';

/// `raw` as a phone shows it and signs it: printable ASCII as it is, and
/// every other character (a control, a tab, a newline, an invisible or bidi
/// mark, any non-ASCII letter, the brackets themselves) as `⟨U+XXXX⟩`, so
/// nothing hides and nothing reorders. `None` when longer than `max`
/// characters once escaped: it is never cut.
pub fn shown_text(raw: &str, max: usize) -> Option<String> {
    let mut out = String::new();
    let mut count = 0usize;
    for c in raw.chars() {
        if (' '..='~').contains(&c) {
            out.push(c);
            count += 1;
        } else {
            let escaped = format!("{OPEN}U+{:04X}{CLOSE}", c as u32);
            count += escaped.chars().count();
            out.push_str(&escaped);
        }
        if count > max {
            return None;
        }
    }
    Some(out)
}

/// Whether `text` is what [`shown_text`] makes, within `max` characters:
/// printable ASCII and well-formed escapes only.
pub fn is_shown_text(text: &str, max: usize) -> bool {
    if text.chars().count() > max {
        return false;
    }
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if (' '..='~').contains(&c) {
            rest = &rest[1..];
            continue;
        }
        if c != OPEN {
            return false;
        }
        let Some(end) = rest.find(CLOSE) else {
            return false;
        };
        let inner = &rest[OPEN.len_utf8()..end];
        let ok = inner.strip_prefix("U+").is_some_and(|h| {
            (4..=6).contains(&h.len())
                && h.bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_lowercase())
        });
        if !ok {
            return false;
        }
        rest = &rest[end + CLOSE.len_utf8()..];
    }
    true
}

/// The approval an `ask` body carries in `approve`, checked field by field;
/// `None` for none or anything malformed.
pub fn approval_of(body: &serde_json::Value) -> Option<Approval> {
    let approval: Approval = serde_json::from_value(body.get("approve")?.clone()).ok()?;
    let wait = approval.expires_unix.checked_sub(approval.issued_unix)?;
    let longest = match approval.kind {
        Kind::Pin => PIN_TIMEOUT_SECS,
        _ => MAX_TIMEOUT_SECS,
    };
    let ok = super::ask::is_ask_id(&approval.id)
        && is_plain_name(&approval.service)
        && is_plain_name(&approval.user)
        && (approval.requester.is_empty() || is_plain_name(&approval.requester))
        && is_shown_text(&approval.action, MAX_ACTION)
        && is_shown_text(&approval.command, MAX_COMMAND)
        && is_shown_text(&approval.tty, MAX_TTY)
        && is_shown_text(&approval.origin, MAX_ORIGIN)
        && (approval.kind != Kind::Pin || approval.service == PIN_SERVICE)
        && is_hex(&approval.computer, 64)
        && is_hex(&approval.nonce, NONCE_LEN * 2)
        && (1..=longest).contains(&wait);
    ok.then_some(approval)
}

/// The exact bytes a Yes signs: [`SIGNING_DOMAIN`], then each field in a
/// fixed order, each written `<length in UTF-8 bytes>:<value>\n` so no two
/// requests read alike, then `yes`. The Android app builds the same bytes
/// (`ApproveRule.message`), and both sides' tests pin one example. A pin
/// request's attestation challenge is this message's SHA-256.
pub fn signed_message(approval: &Approval) -> Vec<u8> {
    let issued = approval.issued_unix.to_string();
    let expires = approval.expires_unix.to_string();
    let fields: [&str; 15] = [
        SIGNING_DOMAIN,
        &approval.id,
        approval.kind.word(),
        &approval.service,
        &approval.user,
        &approval.requester,
        &approval.action,
        &approval.command,
        &approval.tty,
        &approval.origin,
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

fn is_b64(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
}

/// The signature an `ask_answer` body carries, base64 as sent; `None` when
/// it has none or one too long to be a P-256 signature.
pub fn signature_of(body: &serde_json::Value) -> Option<String> {
    let sig = body.get("signature")?.as_str()?;
    ((1..=MAX_SIGNATURE_B64).contains(&sig.len()) && is_b64(sig)).then(|| sig.to_string())
}

/// The attestation chain a pin answer carries (leaf first, base64 DER);
/// `None` when it has none or one out of bounds.
pub fn attestation_of(body: &serde_json::Value) -> Option<Vec<String>> {
    let chain = body.get("attestation")?.as_array()?;
    if chain.is_empty() || chain.len() > MAX_CHAIN {
        return None;
    }
    chain
        .iter()
        .map(|c| {
            let c = c.as_str()?;
            ((1..=MAX_CERT_B64).contains(&c.len()) && is_b64(c)).then(|| c.to_string())
        })
        .collect()
}

/// The answer body with a signature (a Yes to an approval).
pub fn signed_answer_body(id: &str, signature_b64: &str) -> serde_json::Value {
    serde_json::json!({ "id": id, "answer": "yes", "signature": signature_b64 })
}

/// What an [`APPROVE_KEY`] packet says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyMessage {
    /// The phone's public key (base64 SubjectPublicKeyInfo), and whether it
    /// says it takes only a fingerprint (not the PIN).
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
        serde_json::Value::String(key) => ((1..=MAX_KEY_B64).contains(&key.len()) && is_b64(key))
            .then(|| KeyMessage::Set {
                public_key: key.clone(),
                biometric_only: body
                    .get("biometric_only")
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false),
            }),
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
            requester: "alice".to_string(),
            action: String::new(),
            command: "/usr/bin/sudo pacman -Syu".to_string(),
            tty: "/dev/pts/3".to_string(),
            origin: "sudo(812) < bash(700) < kitty(650)".to_string(),
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
            "21:pocketdesk-approve-v2\n3:a-1\n5:admin\n4:sudo\n5:alice\n5:alice\n0:\n25:/usr/bin/sudo pacman -Syu\n10:/dev/pts/3\n34:sudo(812) < bash(700) < kitty(650)\n64:{}\n64:{}\n4:1000\n4:1020\n3:yes\n",
            "ab".repeat(32),
            "01".repeat(32)
        );
        assert_eq!(text, expected);
        let mut moved = sample();
        moved.command = "/usr/bin/sudo pacman".to_string();
        moved.tty = "-Syu\n10:/dev/pts/3".to_string();
        assert_ne!(signed_message(&moved), signed_message(&sample()));
    }

    #[test]
    fn hidden_characters_are_shown_and_nothing_is_cut() {
        assert_eq!(shown_text("sudo ls", 100).as_deref(), Some("sudo ls"));
        // Right-to-left override, braille blank, Hangul filler, a tab, a
        // newline, an accent, and the escape's own bracket.
        assert_eq!(
            shown_text("a\u{202E}b\u{2800}\u{3164}\t\n\u{e9}\u{27E8}", 200).as_deref(),
            Some("a⟨U+202E⟩b⟨U+2800⟩⟨U+3164⟩⟨U+0009⟩⟨U+000A⟩⟨U+00E9⟩⟨U+27E8⟩")
        );
        assert_eq!(shown_text(&"x".repeat(MAX_COMMAND + 1), MAX_COMMAND), None);
        assert_eq!(
            shown_text(&"\u{2800}".repeat(600), MAX_COMMAND),
            None,
            "escapes count"
        );
        for ok in ["sudo ls", "", "a⟨U+202E⟩b", "⟨U+1F600⟩"] {
            assert!(is_shown_text(ok, 100), "{ok}");
        }
        for bad in [
            "a\u{202E}b",
            "tab\there",
            "é",
            "⟨U+12⟩",
            "⟨u+202e⟩",
            "⟨U+202E",
            "⟨X⟩",
        ] {
            assert!(!is_shown_text(bad, 100), "{bad}");
        }
    }

    #[test]
    fn an_approval_rides_in_the_ask_and_is_checked() {
        let a = sample();
        let body = json!({ "id": "a-1", "question": a.summary(), "approve": a.to_json() });
        assert_eq!(approval_of(&body), Some(a.clone()));
        for broken in [
            json!({"service": "sudo; rm"}),
            json!({"user": "-x"}),
            json!({"requester": "a b"}),
            json!({"computer": "AB".repeat(32)}),
            json!({"nonce": "01"}),
            json!({"command": "two\nlines"}),
            json!({"command": "a\u{202E}b"}),
            json!({"origin": "x".repeat(MAX_ORIGIN + 1)}),
            json!({"tty": "\u{7}"}),
            json!({"expires_unix": 999}),
            json!({"expires_unix": 1_000 + MAX_TIMEOUT_SECS + 1}),
            json!({"kind": "root"}),
            json!({"kind": "pin"}),
        ] {
            let mut v = a.to_json();
            for (k, val) in broken.as_object().unwrap() {
                v[k] = val.clone();
            }
            assert_eq!(approval_of(&json!({ "approve": v })), None, "{broken}");
        }
        let mut pin = a.to_json();
        pin["kind"] = json!("pin");
        pin["service"] = json!(PIN_SERVICE);
        pin["expires_unix"] = json!(1_000 + PIN_TIMEOUT_SECS);
        assert!(approval_of(&json!({ "approve": pin })).is_some());
        assert_eq!(approval_of(&json!({ "id": "a-1" })), None);
    }

    #[test]
    fn a_signature_a_chain_and_a_key_are_read_or_refused() {
        assert_eq!(
            signature_of(&signed_answer_body("a-1", "MEUCIQ+/=")),
            Some("MEUCIQ+/=".to_string())
        );
        assert_eq!(signature_of(&json!({ "signature": "a b" })), None);
        assert_eq!(signature_of(&json!({ "signature": "A".repeat(200) })), None);
        assert_eq!(
            attestation_of(&json!({ "attestation": ["MIIB", "MIIC"] })),
            Some(vec!["MIIB".to_string(), "MIIC".to_string()])
        );
        assert_eq!(attestation_of(&json!({ "attestation": [] })), None);
        assert_eq!(attestation_of(&json!({ "attestation": ["a b"] })), None);
        assert_eq!(
            attestation_of(&json!({ "attestation": vec!["MIIB"; MAX_CHAIN + 1] })),
            None
        );
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
    }
}
