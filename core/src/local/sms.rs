//! Texts (SMS) between a phone and this desktop (roadmap N5b).
//!
//! `<state>/sms.json` holds the conversation: the texts a phone forwarded and
//! the replies sent from here, the newest [`KEEP`]. Only the server writes it.
//! The desktop app asks for a reply by writing one file in
//! `<state>/sms_outbox/`; the server takes it once the phone is connected,
//! adds it to the conversation as sending, and records the phone's answer.
//! Both are owner-only: they hold private messages.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::approval::is_valid_id;
use crate::secure_fs;

/// The conversation file.
pub const SMS_FILE: &str = "sms.json";
/// Where the desktop app leaves replies for the server.
pub const SMS_OUTBOX_DIR: &str = "sms_outbox";
/// How many messages the conversation remembers, unless config.toml's
/// `sms_keep` says otherwise.
pub const KEEP: usize = 1000;
/// A reply no phone took within this long fails.
pub const OUTBOX_TTL_SECS: u64 = 24 * 60 * 60;
/// A reply handed to the phone that it has not confirmed within this long fails.
pub const CONFIRM_TTL_SECS: u64 = 10 * 60;
/// The longest text kept or sent, in characters.
pub const MAX_BODY: usize = 2000;

/// Which way a message went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// A text the phone received and forwarded.
    Received,
    /// A reply sent from this desktop through the phone.
    Reply,
}

/// Where a reply stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum ReplyState {
    /// Handed to the phone, not yet confirmed; `attempts` hand-overs so far.
    Sending {
        since_unix: u64,
        #[serde(default = "one")]
        attempts: u32,
    },
    Sent,
    Failed {
        reason: String,
    },
}

/// One text in a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// The phone it came through (its paired device id).
    pub device_id: String,
    /// The other party's number.
    pub address: String,
    pub body: String,
    pub at_unix: u64,
    pub direction: Direction,
    /// A reply's id and state; `None` for a received text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<Reply>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub id: String,
    #[serde(flatten)]
    pub state: ReplyState,
}

/// A reply the desktop app asked for, waiting in the outbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyRequest {
    pub id: String,
    pub device_id: String,
    pub address: String,
    pub body: String,
    pub created_unix: u64,
}

/// A conversation: one phone, one number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    pub device_id: String,
    pub address: String,
    pub last_at_unix: u64,
    pub last_body: String,
    pub count: usize,
}

#[derive(Default, Serialize, Deserialize)]
struct Store {
    messages: Vec<Message>,
}

fn one() -> u32 {
    1
}

/// `text` cut to [`MAX_BODY`] characters.
pub fn bounded(text: &str) -> String {
    text.chars().take(MAX_BODY).collect()
}

/// The conversation, oldest first. A missing or unreadable file is empty.
pub fn read(state_dir: &Path) -> Vec<Message> {
    read_checked(state_dir).unwrap_or_default()
}

/// The conversation for a writer: a missing file is empty, but one that
/// cannot be read or parsed is an error, so a bad read never becomes "no
/// history" that the next write would save over everything.
pub fn read_checked(state_dir: &Path) -> io::Result<Vec<Message>> {
    match std::fs::read(state_dir.join(SMS_FILE)) {
        Ok(raw) => serde_json::from_slice::<Store>(&raw)
            .map(|store| store.messages)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Replace the conversation, keeping the newest `keep`: written to a private
/// temp file beside it and renamed over it, so a crash mid-write leaves the
/// old file whole and a reader never sees a half-written one.
fn write(state_dir: &Path, mut messages: Vec<Message>, keep: usize) -> io::Result<()> {
    let excess = messages.len().saturating_sub(keep.max(1));
    messages.drain(..excess);
    let json = serde_json::to_vec(&Store { messages }).map_err(io::Error::other)?;
    // The state folder is shared (the trust store, the keys…): only this
    // file is made private, never the folder's own permissions.
    std::fs::create_dir_all(state_dir)?;
    let tmp = state_dir.join(format!("{SMS_FILE}.tmp"));
    secure_fs::write_private(&tmp, &json)?;
    std::fs::rename(&tmp, state_dir.join(SMS_FILE))
}

/// Add `message`, keeping the newest `keep` (the server's side). The caller
/// serializes calls.
pub fn record(state_dir: &Path, message: Message, keep: usize) -> io::Result<()> {
    record_all(state_dir, vec![message], keep)
}

/// Add `messages` in one write, keeping the newest `keep`.
pub fn record_all(state_dir: &Path, messages: Vec<Message>, keep: usize) -> io::Result<()> {
    if messages.is_empty() {
        return Ok(());
    }
    let mut all = read_checked(state_dir)?;
    all.extend(messages);
    write(state_dir, all, keep)
}

/// Set reply `id`'s state (the server's side). An unknown id is ignored.
pub fn set_reply_state(
    state_dir: &Path,
    id: &str,
    state: ReplyState,
    keep: usize,
) -> io::Result<()> {
    let mut messages = read_checked(state_dir)?;
    let Some(reply) = messages
        .iter_mut()
        .filter_map(|m| m.reply.as_mut())
        .find(|r| r.id == id)
    else {
        return Ok(());
    };
    reply.state = state;
    write(state_dir, messages, keep)
}

/// Replies handed to a phone longer than [`CONFIRM_TTL_SECS`] ago, still
/// unconfirmed: their ids.
pub fn unconfirmed(messages: &[Message], now_unix: u64) -> Vec<String> {
    messages
        .iter()
        .filter_map(|m| m.reply.as_ref())
        .filter(|r| {
            matches!(r.state, ReplyState::Sending { since_unix, .. }
                if now_unix.saturating_sub(since_unix) >= CONFIRM_TTL_SECS)
        })
        .map(|r| r.id.clone())
        .collect()
}

fn outbox_dir(state_dir: &Path) -> PathBuf {
    state_dir.join(SMS_OUTBOX_DIR)
}

/// Ask for a reply to `address` through `device_id` (the desktop app's side).
pub fn queue_reply(
    state_dir: &Path,
    device_id: &str,
    address: &str,
    body: &str,
    now_unix: u64,
) -> io::Result<ReplyRequest> {
    let request = ReplyRequest {
        id: uuid::Uuid::new_v4().to_string(),
        device_id: device_id.to_string(),
        address: address.trim().to_string(),
        body: bounded(body.trim()),
        created_unix: now_unix,
    };
    if request.address.is_empty() || request.body.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a reply needs a number and some text",
        ));
    }
    let dir = outbox_dir(state_dir);
    secure_fs::create_private_dir(&dir)?;
    let json = serde_json::to_vec(&request).map_err(io::Error::other)?;
    secure_fs::write_private(&dir.join(format!("{}.json", request.id)), &json)?;
    Ok(request)
}

/// The replies waiting in the outbox, oldest first (read only).
pub fn pending(state_dir: &Path) -> Vec<ReplyRequest> {
    let Ok(entries) = std::fs::read_dir(outbox_dir(state_dir)) else {
        return Vec::new();
    };
    let mut requests: Vec<ReplyRequest> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let id = name.strip_suffix(".json")?;
            if !is_valid_id(id) {
                return None;
            }
            let raw = std::fs::read(entry.path()).ok()?;
            serde_json::from_slice::<ReplyRequest>(&raw)
                .ok()
                .filter(|r| r.id == id)
        })
        .collect();
    requests.sort_by_key(|r| r.created_unix);
    requests
}

/// Remove request `id` from the outbox (the server took it).
pub fn remove_pending(state_dir: &Path, id: &str) -> io::Result<()> {
    if !is_valid_id(id) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "bad id"));
    }
    match std::fs::remove_file(outbox_dir(state_dir).join(format!("{id}.json"))) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The conversations in `messages`, newest first.
pub fn conversations(messages: &[Message]) -> Vec<Conversation> {
    let mut by_key: Vec<Conversation> = Vec::new();
    for m in messages {
        match by_key
            .iter_mut()
            .find(|c| c.device_id == m.device_id && c.address == m.address)
        {
            Some(c) => {
                c.count += 1;
                if m.at_unix >= c.last_at_unix {
                    c.last_at_unix = m.at_unix;
                    c.last_body = m.body.clone();
                }
            }
            None => by_key.push(Conversation {
                device_id: m.device_id.clone(),
                address: m.address.clone(),
                last_at_unix: m.at_unix,
                last_body: m.body.clone(),
                count: 1,
            }),
        }
    }
    by_key.sort_by_key(|c| std::cmp::Reverse(c.last_at_unix));
    by_key
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-sms-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn received(device: &str, address: &str, body: &str, at: u64) -> Message {
        Message {
            device_id: device.into(),
            address: address.into(),
            body: body.into(),
            at_unix: at,
            direction: Direction::Received,
            reply: None,
        }
    }

    #[test]
    fn the_conversation_keeps_the_newest_thousand_and_reads_back() {
        let dir = dir("keep");
        assert!(read(&dir).is_empty(), "missing is empty");
        for i in 0..(KEEP as u64 + 3) {
            record(&dir, received("p", "+1", &format!("m{i}"), i), KEEP).unwrap();
        }
        let messages = read(&dir);
        assert_eq!(messages.len(), KEEP);
        assert_eq!(messages[0].body, "m3", "the oldest went first");
        std::fs::write(dir.join(SMS_FILE), "not json").unwrap();
        assert!(
            read(&dir).is_empty(),
            "the app shows nothing for an unreadable file"
        );
        assert!(read_checked(&dir).is_err(), "but a writer is stopped");
        assert!(
            record(&dir, received("p", "+1", "new", 1), KEEP).is_err(),
            "and does not save one text over a history it could not read"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join(SMS_FILE)).unwrap(),
            "not json"
        );
    }

    #[test]
    fn a_reply_waits_in_the_outbox_then_its_state_is_recorded() {
        let dir = dir("reply");
        assert!(queue_reply(&dir, "p", " ", "hi", 1).is_err(), "no number");
        assert!(queue_reply(&dir, "p", "+1", "  ", 1).is_err(), "no text");
        let request = queue_reply(&dir, "p", " +1 555 ", "on my way", 10).unwrap();
        assert_eq!(request.address, "+1 555");
        assert_eq!(pending(&dir), vec![request.clone()]);

        record(
            &dir,
            Message {
                device_id: "p".into(),
                address: request.address.clone(),
                body: request.body.clone(),
                at_unix: 20,
                direction: Direction::Reply,
                reply: Some(Reply {
                    id: request.id.clone(),
                    state: ReplyState::Sending {
                        since_unix: 20,
                        attempts: 1,
                    },
                }),
            },
            KEEP,
        )
        .unwrap();
        remove_pending(&dir, &request.id).unwrap();
        assert!(pending(&dir).is_empty());

        assert!(unconfirmed(&read(&dir), 20 + CONFIRM_TTL_SECS - 1).is_empty());
        assert_eq!(
            unconfirmed(&read(&dir), 20 + CONFIRM_TTL_SECS),
            vec![request.id.clone()]
        );
        set_reply_state(&dir, &request.id, ReplyState::Sent, KEEP).unwrap();
        assert_eq!(
            read(&dir)[0].reply.as_ref().unwrap().state,
            ReplyState::Sent
        );
        set_reply_state(&dir, "unknown", ReplyState::Sent, KEEP).unwrap();
        record(&dir, received("p", "+1", "x", 30), 1).unwrap();
        assert_eq!(read(&dir).len(), 1, "a smaller sms_keep trims");
    }

    #[test]
    fn a_planted_outbox_file_with_another_id_or_name_is_ignored() {
        let dir = dir("planted");
        let request = queue_reply(&dir, "p", "+1", "hi", 1).unwrap();
        let outbox = dir.join(SMS_OUTBOX_DIR);
        std::fs::copy(
            outbox.join(format!("{}.json", request.id)),
            outbox.join("00000000-0000-0000-0000-000000000000.json"),
        )
        .unwrap();
        std::fs::write(outbox.join("../escape.json"), "{}").unwrap();
        assert_eq!(
            pending(&dir),
            vec![request],
            "the copy's id does not match its name"
        );
        assert!(remove_pending(&dir, "../sms").is_err());
    }

    #[test]
    fn conversations_are_by_phone_and_number_newest_first() {
        let messages = vec![
            received("p", "+1", "a", 1),
            received("p", "+2", "b", 2),
            received("p", "+1", "c", 3),
            received("q", "+1", "d", 0),
        ];
        let list = conversations(&messages);
        assert_eq!(list.len(), 3);
        assert_eq!((list[0].address.as_str(), list[0].count), ("+1", 2));
        assert_eq!(list[0].last_body, "c");
        assert_eq!(list[1].address, "+2");
        assert_eq!(list[2].device_id, "q");
    }

    /// Keeping texts must never touch the shared state folder's permissions:
    /// it once made the folder private, which locked the server out of the
    /// trust store and the device keys sitting next to it.
    #[test]
    fn keeping_texts_leaves_the_files_beside_them_readable() {
        let dir = dir("neighbours");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("trusted_devices.json"), b"{}").unwrap();
        record(&dir, received("p", "+1", "hi", 1), KEEP).unwrap();
        queue_reply(&dir, "p", "+1", "hello", 2).unwrap();
        assert_eq!(
            std::fs::read(dir.join("trusted_devices.json")).unwrap(),
            b"{}"
        );
        std::fs::write(dir.join("endpoint.json"), b"[]").unwrap();
        assert_eq!(std::fs::read(dir.join("endpoint.json")).unwrap(), b"[]");
        assert_eq!(read(&dir).len(), 1);
    }

    #[test]
    fn a_long_text_is_cut() {
        assert_eq!(bounded(&"x".repeat(MAX_BODY + 5)).chars().count(), MAX_BODY);
    }
}
