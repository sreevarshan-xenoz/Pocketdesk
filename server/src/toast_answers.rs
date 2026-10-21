//! Answering a phone's notification from the computer's own notification
//! (N3): what the toast or the D-Bus notification offers, and what a press
//! on it means. Pure, so both are tested on every OS; `platform::windows`
//! and the notification plugin's D-Bus path build on it. The answer itself
//! goes the way the desktop app's Notifications page sends one (T10c):
//! `phone_notes::request`, held by the server's answer loop to what the
//! notification still offers.

use pocketdesk_core::local::phone_notes::{Answer, PhoneNote};
use pocketdesk_core::protocol::notification_action::MAX_REPLY_CHARS;

/// Windows shows at most five actions on a toast.
const MAX_TOAST_ACTIONS: usize = 5;

/// The toast argument of the reply's Send button, and its input's id.
pub const REPLY: &str = "reply";

/// The toast argument prefix of a button, followed by its index.
const ACTION: &str = "action:";

/// The D-Bus action of KDE's inline reply.
pub const DBUS_INLINE_REPLY: &str = "inline-reply";

/// The toast argument and D-Bus action of "Not from this app" (K2b).
pub const MUTE: &str = "mute";

/// Its label.
const MUTE_LABEL: &str = "Not from this app";

/// The toast argument and D-Bus action of "Open on the phone" (N3).
pub const OPEN: &str = "open";

/// Its label.
const OPEN_LABEL: &str = "Open on the phone";

/// What a shown notification can be answered with: whose it is, its
/// buttons in the order shown, whether it takes a reply, and whether it
/// opens on the phone (N3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answerable {
    pub device_id: String,
    pub key: String,
    pub actions: Vec<String>,
    pub reply: bool,
    pub open: bool,
}

impl Answerable {
    pub fn of(note: &PhoneNote) -> Self {
        Self {
            device_id: note.device_id.clone(),
            key: note.key.clone(),
            actions: note.actions.clone(),
            reply: note.reply,
            open: note.open,
        }
    }

    /// The buttons a toast has room for beside the reply, "Open on the
    /// phone" and, with `mute`, "Not from this app".
    fn toast_buttons(&self, mute: bool) -> &[String] {
        let room = MAX_TOAST_ACTIONS
            - usize::from(self.reply)
            - usize::from(self.open)
            - usize::from(mute);
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
/// that can be answered, a reply box with Send and its buttons, and with
/// `mute`, "Not from this app" last (K2b).
pub fn toast_xml(summary: &str, body: &str, answers: Option<&Answerable>, mute: bool) -> String {
    toast_xml_pictured(summary, body, answers, mute, &Pictures::default())
}

/// The pictures a phone's notification shows with it (K3): its app's icon
/// and its own picture, as kept files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pictures {
    pub icon: Option<std::path::PathBuf>,
    pub picture: Option<std::path::PathBuf>,
}

/// `path` as a `file:` URI: separators forward, and any byte but a path's
/// plain characters percent-encoded.
pub fn file_uri(path: &std::path::Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    let mut uri = String::from("file://");
    if !path.starts_with('/') {
        uri.push('/');
    }
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/:".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

/// [`toast_xml`] with the app's icon as the toast's logo and the
/// notification's picture inline (K3).
pub fn toast_xml_pictured(
    summary: &str,
    body: &str,
    answers: Option<&Answerable>,
    mute: bool,
    pictures: &Pictures,
) -> String {
    let mut xml = format!(
        "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text>",
        xml_escape(summary),
        xml_escape(body)
    );
    if let Some(icon) = &pictures.icon {
        xml.push_str(&format!(
            "<image placement=\"appLogoOverride\" src=\"{}\"/>",
            xml_escape(&file_uri(icon))
        ));
    }
    if let Some(picture) = &pictures.picture {
        xml.push_str(&format!(
            "<image src=\"{}\"/>",
            xml_escape(&file_uri(picture))
        ));
    }
    xml.push_str("</binding></visual>");
    let answers = answers.filter(|a| a.reply || a.open || !a.actions.is_empty());
    if answers.is_some() || mute {
        xml.push_str("<actions>");
    }
    if let Some(answers) = answers {
        if answers.reply {
            xml.push_str(&format!(
                "<input id=\"{REPLY}\" type=\"text\" placeHolderContent=\"Reply\"/>\
                 <action content=\"Send\" arguments=\"{REPLY}\" hint-inputId=\"{REPLY}\"/>"
            ));
        }
        for (i, title) in answers.toast_buttons(mute).iter().enumerate() {
            xml.push_str(&format!(
                "<action content=\"{}\" arguments=\"{ACTION}{i}\"/>",
                xml_escape(title)
            ));
        }
        if answers.open {
            xml.push_str(&format!(
                "<action content=\"{OPEN_LABEL}\" arguments=\"{OPEN}\"/>"
            ));
        }
    }
    if mute {
        xml.push_str(&format!(
            "<action content=\"{MUTE_LABEL}\" arguments=\"{MUTE}\"/>"
        ));
    }
    if answers.is_some() || mute {
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
    mute: bool,
) -> Option<Answer> {
    if arguments == REPLY {
        let text = reply_text?.trim();
        return (answers.reply && !text.is_empty()).then(|| Answer::Reply(text.to_string()));
    }
    if arguments == OPEN {
        return answers.open.then_some(Answer::Open);
    }
    let index: usize = arguments.strip_prefix(ACTION)?.parse().ok()?;
    answers
        .toast_buttons(mute)
        .get(index)
        .map(|title| Answer::Action(title.clone()))
}

/// What the notification server says it can show (`GetCapabilities`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DbusCaps {
    /// Buttons (`actions`).
    pub actions: bool,
    /// KDE's inline reply (`inline-reply`).
    pub inline_reply: bool,
    /// Pictures in the body (`body-images`, with `body-markup`), K3.
    pub body_images: bool,
}

impl DbusCaps {
    pub fn from_list(caps: &[String]) -> Self {
        Self {
            actions: caps.iter().any(|c| c == "actions"),
            inline_reply: caps.iter().any(|c| c == DBUS_INLINE_REPLY),
            body_images: caps.iter().any(|c| c == "body-images")
                && caps.iter().any(|c| c == "body-markup"),
        }
    }
}

/// The D-Bus notification's actions, as key and label pairs: `a0`… for its
/// buttons where the server shows actions, and KDE's inline reply where it
/// offers one; a server that offers neither gets none, never a button that
/// does nothing (N3 review).
pub fn dbus_actions(answers: Option<&Answerable>, caps: DbusCaps, mute: bool) -> Vec<String> {
    let mut actions = Vec::new();
    if let Some(answers) = answers {
        if caps.actions {
            for (i, title) in answers.actions.iter().enumerate() {
                actions.push(format!("a{i}"));
                actions.push(title.clone());
            }
        }
        if answers.reply && caps.inline_reply {
            actions.push(DBUS_INLINE_REPLY.to_string());
            actions.push("Reply".to_string());
        }
        // N3: where the server shows buttons.
        if answers.open && caps.actions {
            actions.push(OPEN.to_string());
            actions.push(OPEN_LABEL.to_string());
        }
    }
    // "Not from this app" where the server shows buttons (K2b).
    if mute && caps.actions {
        actions.push(MUTE.to_string());
        actions.push(MUTE_LABEL.to_string());
    }
    actions
}

/// Keep `kept`, newest last, to `cap` of each kind: an answerable one's
/// place is never taken by a plain one's (K2b review: thirty plain toasts
/// had left an answerable one's buttons dead).
pub fn keep_within<T>(
    kept: &mut std::collections::VecDeque<T>,
    answerable: impl Fn(&T) -> bool,
    cap: usize,
) {
    for kind in [true, false] {
        while kept.iter().filter(|t| answerable(t) == kind).count() > cap {
            if let Some(oldest) = kept.iter().position(|t| answerable(t) == kind) {
                kept.remove(oldest);
            }
        }
    }
}

/// Mute `app` on this computer from its notification's button (K2b): its
/// next notifications are kept and logged, not shown.
pub fn mute(app: &str) {
    let muted = crate::state::state_dir()
        .map_err(std::io::Error::other)
        .and_then(|dir| {
            pocketdesk_core::local::muted_apps::mute(
                &dir,
                app,
                pocketdesk_core::local::pin::now_unix(),
            )
        });
    // Said, not only logged (K2b review): a toast pops up under the
    // pointer, and a stray press would hide an app without a word.
    let (title, body) = match muted {
        Ok(()) => {
            tracing::info!("Notifications from {app} muted on this computer");
            (
                format!("{app} muted"),
                "Its notifications from phones are not shown here now. Unmute it in Pocketdesk's Settings, under Notifications and texts.".to_string(),
            )
        }
        Err(e) => {
            tracing::warn!("Could not mute {app}: {e}");
            (format!("{app} not muted"), e.to_string())
        }
    };
    crate::platform::notify(&title, &body, std::time::Duration::from_secs(8));
}

/// What `ActionInvoked` with `key` answers: a button by its `a<n>` key,
/// or "Open on the phone" (N3).
pub fn dbus_action_answer(key: &str, answers: &Answerable) -> Option<Answer> {
    if key == OPEN {
        return answers.open.then_some(Answer::Open);
    }
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
    // A reply's length, for saying why one was refused.
    let answer_kind = match &answer {
        Answer::Reply(text) => Some(text.chars().count()),
        Answer::Action(_) | Answer::Open => None,
    };
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
        Err(e) => {
            tracing::warn!("An answer from a notification was not kept: {e}");
            // The toast is gone by now: say so, or the press seems to have
            // worked (N3 review).
            let why = match &answer_kind {
                Some(len) if *len > MAX_REPLY_CHARS => format!(
                    "The reply is longer than {MAX_REPLY_CHARS} characters, so it was not sent to the phone."
                ),
                _ => "The answer could not be sent to the phone.".to_string(),
            };
            crate::platform::notify("Pocketdesk", &why, std::time::Duration::from_secs(5));
        }
    }
}

#[cfg(test)]
mod tests {

    /// N3: "Open on the phone" in the toast and over D-Bus, only where the
    /// notification offers it, and room kept for it among the buttons.
    #[test]
    fn open_on_the_phone_is_a_button_where_offered() {
        let answers = Answerable {
            device_id: "phone".to_string(),
            key: "k".to_string(),
            actions: vec![],
            reply: false,
            open: true,
        };
        let xml = toast_xml_pictured("Ann", "Hi", Some(&answers), false, &Pictures::default());
        assert!(xml.contains("arguments=\"open\""), "{xml}");
        assert_eq!(
            toast_answer(OPEN, None, &answers, false),
            Some(Answer::Open)
        );
        let caps = DbusCaps {
            actions: true,
            ..Default::default()
        };
        assert_eq!(
            dbus_actions(Some(&answers), caps, false),
            vec![OPEN.to_string(), OPEN_LABEL.to_string()]
        );
        assert_eq!(dbus_action_answer(OPEN, &answers), Some(Answer::Open));
        let closed = Answerable {
            open: false,
            ..answers.clone()
        };
        assert!(
            !toast_xml_pictured("Ann", "Hi", Some(&closed), false, &Pictures::default())
                .contains("open")
        );
        assert_eq!(toast_answer(OPEN, None, &closed, false), None);
        assert_eq!(dbus_action_answer(OPEN, &closed), None);
        let full = Answerable {
            actions: (0..5).map(|i| format!("b{i}")).collect(),
            reply: true,
            ..answers
        };
        assert_eq!(full.toast_buttons(true).len(), MAX_TOAST_ACTIONS - 3);
    }
    use super::*;

    fn answerable(actions: &[&str], reply: bool) -> Answerable {
        Answerable {
            device_id: "phone".into(),
            key: "0|chat|1|null|10001".into(),
            actions: actions.iter().map(|a| a.to_string()).collect(),
            reply,
            open: false,
        }
    }

    /// K3: the icon is the toast's logo and the picture shows inline, each
    /// a file URI the toast can read.
    #[test]
    fn a_toast_shows_its_icon_and_picture() {
        let pictures = Pictures {
            icon: Some(std::path::PathBuf::from(
                r"C:\state\notification_icons\ab.png",
            )),
            picture: Some(std::path::PathBuf::from("/state/my pictures/c&d.jpg")),
        };
        let xml = toast_xml_pictured("Chat: Ann", "hi", None, false, &pictures);
        assert!(xml.contains(
            "<image placement=\"appLogoOverride\" src=\"file:///C:/state/notification_icons/ab.png\"/>"
        ));
        assert!(xml.contains("<image src=\"file:///state/my%20pictures/c%26d.jpg\"/>"));
        assert!(xml.find("<image").unwrap() < xml.find("</binding>").unwrap());
        assert!(!toast_xml("t", "b", None, false).contains("<image"));
    }

    /// K3: pictures go in a D-Bus body only where the server shows them.
    #[test]
    fn body_images_need_markup_too() {
        let caps = |list: &[&str]| {
            DbusCaps::from_list(&list.iter().map(|c| c.to_string()).collect::<Vec<_>>())
        };
        assert!(caps(&["body-images", "body-markup"]).body_images);
        assert!(!caps(&["body-images"]).body_images);
        assert!(!caps(&["body-markup"]).body_images);
    }

    #[test]
    fn a_plain_notification_has_no_actions() {
        let xml = toast_xml("Chat: Ann", "hi", None, false);
        assert!(!xml.contains("<actions>"));
        assert!(!toast_xml("t", "b", Some(&answerable(&[], false)), false).contains("<actions>"));
    }

    #[test]
    fn an_answerable_one_has_its_reply_box_and_buttons() {
        let xml = toast_xml(
            "Chat: Ann",
            "hi",
            Some(&answerable(&["Mark read", "Mute"], true)),
            false,
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
            false,
        );
        assert!(xml.contains("<text>A &amp; B</text><text>&lt;x&gt;</text>"));
        assert!(xml.contains("content=\"Say &quot;hi&quot; &amp; &apos;go&apos;\""));
    }

    #[test]
    fn a_toast_shows_at_most_five_actions() {
        let many = answerable(&["1", "2", "3", "4", "5", "6"], true);
        let xml = toast_xml("t", "b", Some(&many), false);
        assert_eq!(xml.matches("<action ").count(), 5, "Send and four buttons");
        assert_eq!(
            toast_answer("action:4", None, &many, false),
            None,
            "the fifth was never shown"
        );
        assert_eq!(
            toast_answer("action:3", None, &many, false),
            Some(Answer::Action("4".into()))
        );
    }

    #[test]
    fn a_press_answers_with_the_reply_or_its_button() {
        let a = answerable(&["Mark read"], true);
        assert_eq!(
            toast_answer("reply", Some("  on my way "), &a, false),
            Some(Answer::Reply("on my way".into()))
        );
        assert_eq!(
            toast_answer("reply", Some("   "), &a, false),
            None,
            "an empty reply"
        );
        assert_eq!(toast_answer("reply", None, &a, false), None);
        assert_eq!(
            toast_answer("action:0", None, &a, false),
            Some(Answer::Action("Mark read".into()))
        );
        assert_eq!(toast_answer("action:1", None, &a, false), None);
        assert_eq!(toast_answer("action:x", None, &a, false), None);
        assert_eq!(
            toast_answer("", None, &a, false),
            None,
            "a click on the toast itself"
        );
        assert_eq!(
            toast_answer("reply", Some("hi"), &answerable(&[], false), false),
            None,
            "a notification that takes no reply"
        );
    }

    /// K2b: every phone toast ends with "Not from this app", which takes a
    /// place among the five, and a press on it is no answer.
    #[test]
    fn a_toast_offers_not_from_this_app() {
        let plain = toast_xml("Chat: Ann", "hi", None, true);
        assert!(plain.contains(
            "<actions><action content=\"Not from this app\" arguments=\"mute\"/></actions>"
        ));
        let many = answerable(&["1", "2", "3", "4", "5", "6"], true);
        let xml = toast_xml("t", "b", Some(&many), true);
        assert_eq!(
            xml.matches("<action ").count(),
            5,
            "Send, three buttons, and the mute"
        );
        assert!(xml.ends_with("arguments=\"mute\"/></actions></toast>"));
        assert_eq!(
            toast_answer("action:3", None, &many, true),
            None,
            "not shown beside the mute"
        );
        assert_eq!(toast_answer(MUTE, None, &many, true), None);
        let gnome = DbusCaps::from_list(&["actions".into()]);
        assert_eq!(dbus_actions(None, gnome, true), [MUTE, "Not from this app"]);
        assert!(
            dbus_actions(None, DbusCaps::default(), true).is_empty(),
            "no buttons shown"
        );
    }

    /// K2b review: plain toasts never push an answerable one out.
    #[test]
    fn each_kind_keeps_its_own_places() {
        let mut kept: std::collections::VecDeque<(u32, bool)> = [(1, true)]
            .into_iter()
            .chain((2..40).map(|n| (n, false)))
            .collect();
        keep_within(&mut kept, |(_, answerable)| *answerable, 30);
        assert!(kept.contains(&(1, true)), "the answerable one stays");
        assert_eq!(kept.iter().filter(|(_, a)| !a).count(), 30);
        assert_eq!(kept.front(), Some(&(1, true)), "the oldest plain ones went");
        assert!(!kept.contains(&(2, false)));
    }

    #[test]
    fn d_bus_actions_name_each_button_and_the_inline_reply() {
        let a = answerable(&["Mark read", "Mute"], true);
        let kde = DbusCaps::from_list(&["actions".into(), "inline-reply".into(), "body".into()]);
        assert_eq!(
            dbus_actions(Some(&a), kde, false),
            ["a0", "Mark read", "a1", "Mute", "inline-reply", "Reply"]
        );
        // GNOME shows buttons but has no inline reply: no dead Reply button.
        let gnome = DbusCaps::from_list(&["actions".into(), "body".into()]);
        assert_eq!(
            dbus_actions(Some(&a), gnome, false),
            ["a0", "Mark read", "a1", "Mute"]
        );
        assert!(
            dbus_actions(Some(&a), DbusCaps::default(), false).is_empty(),
            "no actions at all"
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
