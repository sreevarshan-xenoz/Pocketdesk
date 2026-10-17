//! Answering a phone's notification from the computer's own notification
//! (N3): what the toast or the D-Bus notification offers, and what a press
//! on it means. Pure, so both are tested on every OS; `platform::windows`
//! and the notification plugin's D-Bus path build on it. The answer itself
//! goes the way the desktop app's Notifications page sends one (T10c):
//! `phone_notes::request`, held by the server's answer loop to what the
//! notification still offers.

use pocketdesk_core::local::phone_notes::{Answer, PhoneNote};

/// Windows shows at most five actions on a toast.
const MAX_TOAST_ACTIONS: usize = 5;

/// The toast argument of the reply's Send button, and its input's id.
pub const REPLY: &str = "reply";

/// The toast argument prefix of a button, followed by its index.
const ACTION: &str = "action:";

/// The D-Bus action of KDE's inline reply.
pub const DBUS_INLINE_REPLY: &str = "inline-reply";

/// What a shown notification can be answered with: whose it is, its
/// buttons in the order shown, and whether it takes a reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answerable {
    pub device_id: String,
    pub key: String,
    pub actions: Vec<String>,
    pub reply: bool,
}

impl Answerable {
    pub fn of(note: &PhoneNote) -> Self {
        Self {
            device_id: note.device_id.clone(),
            key: note.key.clone(),
            actions: note.actions.clone(),
            reply: note.reply,
        }
    }

    /// The buttons a toast has room for beside the reply.
    fn toast_buttons(&self) -> &[String] {
        let room = MAX_TOAST_ACTIONS - usize::from(self.reply);
        &self.actions[..self.actions.len().min(room)]
    }
}

/// Text for an XML text node or attribute.
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The toast's XML: a title line and a body line, then, for a notification
/// that can be answered, a reply box with Send and its buttons.
pub fn toast_xml(summary: &str, body: &str, answers: Option<&Answerable>) -> String {
    let mut xml = format!(
        "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual>",
        xml_escape(summary),
        xml_escape(body)
    );
    if let Some(answers) = answers.filter(|a| a.reply || !a.actions.is_empty()) {
        xml.push_str("<actions>");
        if answers.reply {
            xml.push_str(&format!(
                "<input id=\"{REPLY}\" type=\"text\" placeHolderContent=\"Reply\"/>\
                 <action content=\"Send\" arguments=\"{REPLY}\" hint-inputId=\"{REPLY}\"/>"
            ));
        }
        for (i, title) in answers.toast_buttons().iter().enumerate() {
            xml.push_str(&format!(
                "<action content=\"{}\" arguments=\"{ACTION}{i}\"/>",
                xml_escape(title)
            ));
        }
        xml.push_str("</actions>");
    }
    xml.push_str("</toast>");
    xml
}

/// What a press on the toast answers: Send with the reply box's text, or a
/// button by its index into the buttons shown. Nothing for an empty reply,
/// an index the toast never had, or arguments of no answer.
pub fn toast_answer(
    arguments: &str,
    reply_text: Option<&str>,
    answers: &Answerable,
) -> Option<Answer> {
    if arguments == REPLY {
        let text = reply_text?.trim();
        return (answers.reply && !text.is_empty()).then(|| Answer::Reply(text.to_string()));
    }
    let index: usize = arguments.strip_prefix(ACTION)?.parse().ok()?;
    answers
        .toast_buttons()
        .get(index)
        .map(|title| Answer::Action(title.clone()))
}

/// The D-Bus notification's actions, as key and label pairs: `a0`… for its
/// buttons, and KDE's inline reply when it takes one.
pub fn dbus_actions(answers: &Answerable) -> Vec<String> {
    let mut actions = Vec::new();
    for (i, title) in answers.actions.iter().enumerate() {
        actions.push(format!("a{i}"));
        actions.push(title.clone());
    }
    if answers.reply {
        actions.push(DBUS_INLINE_REPLY.to_string());
        actions.push("Reply".to_string());
    }
    actions
}

/// What `ActionInvoked` with `key` answers: a button by its `a<n>` key.
pub fn dbus_action_answer(key: &str, answers: &Answerable) -> Option<Answer> {
    let index: usize = key.strip_prefix('a')?.parse().ok()?;
    answers
        .actions
        .get(index)
        .map(|title| Answer::Action(title.clone()))
}

/// What `NotificationReplied` with `text` answers.
pub fn dbus_reply_answer(text: &str, answers: &Answerable) -> Option<Answer> {
    let text = text.trim();
    (answers.reply && !text.is_empty()).then(|| Answer::Reply(text.to_string()))
}

/// Hand `answer` to the server's answer loop, as the Notifications page
/// does, which holds it to what the notification still offers.
pub fn send(answers: &Answerable, answer: Answer) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let written = crate::state::state_dir()
        .map_err(|e| e.to_string())
        .and_then(|dir| {
            pocketdesk_core::local::phone_notes::request(
                &dir,
                &answers.device_id,
                &answers.key,
                answer,
                now,
            )
            .map_err(|e| e.to_string())
        });
    match written {
        Ok(_) => tracing::info!("A phone notification was answered from its notification here"),
        Err(e) => tracing::warn!("An answer from a notification was not kept: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answerable(actions: &[&str], reply: bool) -> Answerable {
        Answerable {
            device_id: "phone".into(),
            key: "0|chat|1|null|10001".into(),
            actions: actions.iter().map(|a| a.to_string()).collect(),
            reply,
        }
    }

    #[test]
    fn a_plain_notification_has_no_actions() {
        let xml = toast_xml("Chat: Ann", "hi", None);
        assert!(!xml.contains("<actions>"));
        assert!(!toast_xml("t", "b", Some(&answerable(&[], false))).contains("<actions>"));
    }

    #[test]
    fn an_answerable_one_has_its_reply_box_and_buttons() {
        let xml = toast_xml(
            "Chat: Ann",
            "hi",
            Some(&answerable(&["Mark read", "Mute"], true)),
        );
        assert!(xml.contains("<input id=\"reply\" type=\"text\""));
        assert!(xml.contains("arguments=\"reply\" hint-inputId=\"reply\""));
        assert!(xml.contains("<action content=\"Mark read\" arguments=\"action:0\"/>"));
        assert!(xml.contains("<action content=\"Mute\" arguments=\"action:1\"/>"));
    }

    #[test]
    fn titles_are_escaped_in_text_and_attributes() {
        let xml = toast_xml(
            "A & B",
            "<x>",
            Some(&answerable(&["Say \"hi\" & 'go'"], false)),
        );
        assert!(xml.contains("<text>A &amp; B</text><text>&lt;x&gt;</text>"));
        assert!(xml.contains("content=\"Say &quot;hi&quot; &amp; &apos;go&apos;\""));
    }

    #[test]
    fn a_toast_shows_at_most_five_actions() {
        let many = answerable(&["1", "2", "3", "4", "5", "6"], true);
        let xml = toast_xml("t", "b", Some(&many));
        assert_eq!(xml.matches("<action ").count(), 5, "Send and four buttons");
        assert_eq!(
            toast_answer("action:4", None, &many),
            None,
            "the fifth was never shown"
        );
        assert_eq!(
            toast_answer("action:3", None, &many),
            Some(Answer::Action("4".into()))
        );
    }

    #[test]
    fn a_press_answers_with_the_reply_or_its_button() {
        let a = answerable(&["Mark read"], true);
        assert_eq!(
            toast_answer("reply", Some("  on my way "), &a),
            Some(Answer::Reply("on my way".into()))
        );
        assert_eq!(
            toast_answer("reply", Some("   "), &a),
            None,
            "an empty reply"
        );
        assert_eq!(toast_answer("reply", None, &a), None);
        assert_eq!(
            toast_answer("action:0", None, &a),
            Some(Answer::Action("Mark read".into()))
        );
        assert_eq!(toast_answer("action:1", None, &a), None);
        assert_eq!(toast_answer("action:x", None, &a), None);
        assert_eq!(
            toast_answer("", None, &a),
            None,
            "a click on the toast itself"
        );
        assert_eq!(
            toast_answer("reply", Some("hi"), &answerable(&[], false)),
            None,
            "a notification that takes no reply"
        );
    }

    #[test]
    fn d_bus_actions_name_each_button_and_the_inline_reply() {
        let a = answerable(&["Mark read", "Mute"], true);
        assert_eq!(
            dbus_actions(&a),
            ["a0", "Mark read", "a1", "Mute", "inline-reply", "Reply"]
        );
        assert_eq!(
            dbus_action_answer("a1", &a),
            Some(Answer::Action("Mute".into()))
        );
        assert_eq!(dbus_action_answer("a2", &a), None);
        assert_eq!(dbus_action_answer("default", &a), None);
        assert_eq!(
            dbus_reply_answer(" ok ", &a),
            Some(Answer::Reply("ok".into()))
        );
        assert_eq!(dbus_reply_answer("ok", &answerable(&[], false)), None);
    }
}
