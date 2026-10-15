//! Answering a phone's notification from the computer (T10c), with KDE
//! Connect's packet names.
//!
//! The phone offers, in its `kdeconnect.notification`, the buttons a
//! notification has (`actions`, by title) and, when one of them takes typed
//! text, `requestReplyId`. The computer answers with
//! `kdeconnect.notification.reply {requestReplyId, message}` or
//! `kdeconnect.notification.action {key, action}`. Both ends hold the
//! fields to the limits here.

use super::kdeconnect::NetworkPacket;

/// The computer's reply to a notification that takes typed text.
pub const NOTIFICATION_REPLY: &str = "kdeconnect.notification.reply";

/// The computer's press of a notification's button.
pub const NOTIFICATION_ACTION: &str = "kdeconnect.notification.action";

/// Buttons a notification offers the computer, at most.
pub const MAX_ACTIONS: usize = 3;

/// A button's title, at most, in characters.
pub const MAX_ACTION_CHARS: usize = 40;

/// A reply, at most, in characters.
pub const MAX_REPLY_CHARS: usize = 1000;

/// A notification id (the phone's `StatusBarNotification` key), at most.
pub const MAX_KEY_BYTES: usize = 512;

/// The button titles a `kdeconnect.notification` body offers, held to the
/// limits: at most [`MAX_ACTIONS`], each non-empty, single-line and at most
/// [`MAX_ACTION_CHARS`]; anything else is left out.
pub fn offered_actions(body: &serde_json::Value) -> Vec<String> {
    body.get("actions")
        .and_then(|v| v.as_array())
        .map(|actions| {
            actions
                .iter()
                .filter_map(|a| a.as_str())
                .filter(|a| valid_action(a))
                .take(MAX_ACTIONS)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `title` may name a button.
pub fn valid_action(title: &str) -> bool {
    !title.trim().is_empty()
        && title.chars().count() <= MAX_ACTION_CHARS
        && !title.chars().any(char::is_control)
}

/// Whether `key` may name a notification.
pub fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= MAX_KEY_BYTES && !key.chars().any(char::is_control)
}

/// Whether `message` may be sent as a reply.
pub fn valid_reply(message: &str) -> bool {
    !message.trim().is_empty() && message.chars().count() <= MAX_REPLY_CHARS
}

/// The reply packet, or `None` when a field is out of bounds.
pub fn reply_packet(key: &str, message: &str) -> Option<NetworkPacket> {
    (valid_key(key) && valid_reply(message)).then(|| {
        NetworkPacket::new(NOTIFICATION_REPLY)
            .with_body(serde_json::json!({ "requestReplyId": key, "message": message }))
    })
}

/// The button packet, or `None` when a field is out of bounds.
pub fn action_packet(key: &str, action: &str) -> Option<NetworkPacket> {
    (valid_key(key) && valid_action(action)).then(|| {
        NetworkPacket::new(NOTIFICATION_ACTION)
            .with_body(serde_json::json!({ "key": key, "action": action }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_bounded_single_line_titles_are_offered() {
        let body = serde_json::json!({
            "actions": ["Mark as read", "", "x".repeat(MAX_ACTION_CHARS + 1), "Two\nlines", 7, "Mute", "Archive", "Fourth"]
        });
        assert_eq!(offered_actions(&body), ["Mark as read", "Mute", "Archive"]);
        assert!(offered_actions(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn answers_are_held_to_their_limits() {
        let reply = reply_packet("0|com.chat|1|null|10123", "On my way").unwrap();
        assert_eq!(reply.packet_type, NOTIFICATION_REPLY);
        assert_eq!(reply.body["requestReplyId"], "0|com.chat|1|null|10123");
        assert_eq!(reply.body["message"], "On my way");
        assert!(reply_packet("k", "  ").is_none());
        assert!(reply_packet("k", &"x".repeat(MAX_REPLY_CHARS + 1)).is_none());
        assert!(reply_packet("", "hi").is_none());
        assert!(reply_packet("bad\nkey", "hi").is_none());
        let action = action_packet("k", "Mark as read").unwrap();
        assert_eq!(action.packet_type, NOTIFICATION_ACTION);
        assert_eq!(action.body["action"], "Mark as read");
        assert!(action_packet("k", "").is_none());
    }
}
