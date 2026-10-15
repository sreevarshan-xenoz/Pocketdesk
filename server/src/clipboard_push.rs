//! The computer's clipboard to connected phones as it changes (T10b).
//!
//! A phone used to see the computer's clipboard only by asking for it every
//! 4 s during a session. Now the server watches the clipboard and pushes
//! new text to every live control connection whose phone may receive
//! pushes and has the Clipboard permission, so a phone holding a link in the
//! background gets it too. Text only: an image needs a transfer that dials
//! the phone back.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket};

/// The longest clipboard text pushed. A phone asking in a session still
/// gets longer text; a push goes to every phone, so it stays small.
pub(crate) const MAX_PUSH: usize = 256 * 1024;

/// How often the clipboard is looked at while a phone could receive it. On
/// Windows a look costs a sequence number; elsewhere it runs `wl-paste`.
#[cfg(windows)]
const TICK: Duration = Duration::from_secs(1);
#[cfg(not(windows))]
const TICK: Duration = Duration::from_secs(2);

/// Change detection over successive reads of the clipboard's text. It keeps
/// a hash of what it saw, not the text: a large copy is not held twice.
#[derive(Debug, Default)]
pub(crate) struct Watch {
    seen: Option<u64>,
    primed: bool,
}

fn hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

impl Watch {
    /// The text to push after reading `text` (`None`: no text on the
    /// clipboard). The first read only learns what is there: the phone
    /// asks for the current clipboard itself when a session starts.
    pub(crate) fn observe(&mut self, text: Option<&str>) -> Option<String> {
        let first = !self.primed;
        self.primed = true;
        let text = text.filter(|t| !t.is_empty());
        let seen = text.map(hash);
        if seen == self.seen {
            return None;
        }
        self.seen = seen;
        if first {
            return None;
        }
        text.filter(|t| t.len() <= MAX_PUSH).map(str::to_string)
    }
}

/// The marks a copy carries on Windows that say it is private: password
/// managers set them so clipboard history, cloud sync and monitors skip it.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowsMarks {
    /// `ExcludeClipboardContentFromMonitorProcessing` is on the clipboard.
    pub exclude_from_monitors: bool,
    /// `Clipboard Viewer Ignore` is on the clipboard.
    pub viewer_ignore: bool,
    /// The DWORD in `CanIncludeInClipboardHistory`, when present.
    pub history: Option<u32>,
    /// The DWORD in `CanUploadToCloudClipboard`, when present.
    pub cloud: Option<u32>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl WindowsMarks {
    /// Is the copy private? Then it never leaves this computer.
    pub(crate) fn private(self) -> bool {
        self.exclude_from_monitors
            || self.viewer_ignore
            || self.history == Some(0)
            || self.cloud == Some(0)
    }
}

/// Is a Wayland copy private? KDE's password managers (KeePassXC among
/// them) offer `x-kde-passwordManagerHint` with it.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn private_types(types: &str) -> bool {
    types
        .lines()
        .any(|t| t.trim() == "x-kde-passwordManagerHint")
}

/// Which phone last set the computer's clipboard, and to what: that phone
/// is not sent its own text back.
static FROM_PHONE: Mutex<Option<(String, String)>> = Mutex::new(None);

/// Note that `device_id` just set the clipboard to `text`.
pub(crate) fn note_from_phone(device_id: &str, text: &str) {
    *FROM_PHONE.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((device_id.to_string(), text.to_string()));
}

/// The note, used once: the next change it explains, or not. A copy of the
/// same text later is the computer's own, and goes to that phone too.
fn take_from_phone() -> Option<(String, String)> {
    FROM_PHONE.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// The connections to push `text` to: those that may have it (`may`), but
/// not the phone the text came from.
pub(crate) fn targets(
    clients: Vec<Arc<dyn DeviceSender>>,
    text: &str,
    from_phone: Option<&(String, String)>,
    may: impl Fn(&dyn DeviceSender) -> bool,
) -> Vec<Arc<dyn DeviceSender>> {
    clients
        .into_iter()
        .filter(|c| !from_phone.is_some_and(|(id, sent)| id == c.device_id() && sent == text))
        .filter(|c| may(&**c))
        .collect()
}

/// The packet a push sends: what the phone's own request is answered with.
pub(crate) fn packet(text: &str) -> NetworkPacket {
    NetworkPacket::new("kdeconnect.clipboard").with_body(serde_json::json!({ "content": text }))
}

/// Watch the clipboard while any connection could receive it, and push
/// what changes. Runs for the server's life when `clipboard_push` is on.
pub(crate) async fn run() {
    let mut watch = Watch::default();
    #[cfg(windows)]
    let mut sequence = None;
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let clients = crate::state::clone_clients().await;
        if clients.is_empty() {
            // No phone to tell: what is copied meanwhile is not pushed when
            // one comes back (a phone in a session asks for it itself).
            watch = Watch::default();
            #[cfg(windows)]
            {
                sequence = None;
            }
            continue;
        }
        // On Windows the clipboard's sequence number says whether anything
        // changed, without opening the clipboard.
        #[cfg(windows)]
        {
            let now = crate::platform::clipboard_windows::sequence();
            if sequence == Some(now) {
                continue;
            }
            // Noted only once the clipboard was read: a read that failed
            // (another app held it) is tried again next tick.
            match crate::plugins::clipboard::read_text().await {
                Ok(text) => {
                    sequence = Some(now);
                    push_change(&mut watch, text, clients).await;
                }
                Err(e) => tracing::debug!("The clipboard was not read: {e}"),
            }
        }
        #[cfg(not(windows))]
        {
            let text = crate::plugins::clipboard::read_text().await.ok().flatten();
            push_change(&mut watch, text, clients).await;
        }
    }
}

/// Push `text` to `clients` if it is a change.
async fn push_change(watch: &mut Watch, text: Option<String>, clients: Vec<Arc<dyn DeviceSender>>) {
    let Some(text) = watch.observe(text.as_deref()) else {
        return;
    };
    let from_phone = take_from_phone();
    let targets = targets(clients, &text, from_phone.as_ref(), |c| {
        crate::plugins::pair::permits(c.device_id(), "kdeconnect.clipboard")
    });
    let packet = packet(&text);
    for target in targets {
        if let Err(e) = target.send_packet(&packet).await {
            tracing::debug!("Clipboard not pushed to {}: {e}", target.device_id());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::protocol::kdeconnect::TcpDeviceSender;

    #[test]
    fn a_copy_a_password_manager_marks_stays_on_the_computer() {
        assert!(!WindowsMarks::default().private());
        for marks in [
            WindowsMarks {
                exclude_from_monitors: true,
                ..Default::default()
            },
            WindowsMarks {
                viewer_ignore: true,
                ..Default::default()
            },
            WindowsMarks {
                history: Some(0),
                ..Default::default()
            },
            WindowsMarks {
                cloud: Some(0),
                ..Default::default()
            },
        ] {
            assert!(marks.private(), "{marks:?}");
        }
        assert!(
            !WindowsMarks {
                history: Some(1),
                cloud: Some(1),
                ..Default::default()
            }
            .private(),
            "allowed explicitly"
        );
        assert!(private_types("text/plain\nx-kde-passwordManagerHint\n"));
        assert!(!private_types("text/plain;charset=utf-8\nTEXT\n"));
    }

    #[test]
    fn the_note_of_a_phone_copy_is_used_once() {
        note_from_phone("a", "hello");
        assert_eq!(take_from_phone(), Some(("a".into(), "hello".into())));
        assert_eq!(take_from_phone(), None);
    }

    #[test]
    fn only_a_change_after_the_first_read_is_pushed() {
        let mut watch = Watch::default();
        assert_eq!(watch.observe(Some("there at start")), None);
        assert_eq!(watch.observe(Some("there at start")), None);
        assert_eq!(watch.observe(Some("copied")), Some("copied".into()));
        assert_eq!(watch.observe(Some("copied")), None, "once");
        assert_eq!(watch.observe(None), None, "an image or nothing");
        assert_eq!(
            watch.observe(Some("copied")),
            Some("copied".into()),
            "the same text copied again after something else"
        );
        assert_eq!(watch.observe(Some("")), None);
        let long = "x".repeat(MAX_PUSH + 1);
        assert_eq!(watch.observe(Some(&long)), None, "too long to push");
    }

    fn phone(id: &str) -> Arc<dyn DeviceSender> {
        Arc::new(TcpDeviceSender::from_arc(
            Arc::new(tokio::sync::Mutex::new(tokio::io::sink())),
            id.to_string(),
        ))
    }

    #[test]
    fn the_text_goes_to_permitted_phones_but_not_back_to_its_sender() {
        let clients = vec![phone("a"), phone("b"), phone("c")];
        let ids = |v: Vec<Arc<dyn DeviceSender>>| -> Vec<String> {
            v.iter().map(|c| c.device_id().to_string()).collect()
        };
        let from_a = ("a".to_string(), "hello".to_string());
        assert_eq!(
            ids(targets(clients.clone(), "hello", Some(&from_a), |c| c
                .device_id()
                != "c")),
            ["b"],
            "not back to a; c may not have it"
        );
        assert_eq!(
            ids(targets(clients, "other", Some(&from_a), |_| true)),
            ["a", "b", "c"],
            "a gets text it did not send"
        );
    }
}
