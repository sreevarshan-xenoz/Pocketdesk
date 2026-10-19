//! The clipboard's history across devices (D3): each text that crosses
//! between this computer and its phones is kept (`core::local::clip_history`)
//! while `clipboard_history` asks for any, read at each text so a change in
//! Settings applies at once; and the desktop app's or the local API's "send
//! this to a phone" (`clipboard_out/`) is put on that phone's clipboard over
//! its live link, under its Clipboard permission.

use std::sync::Arc;
use std::time::Duration;

use pocketdesk_core::local::clip_history::{self, ClipEntry, SendRequest};
use pocketdesk_core::protocol::kdeconnect::DeviceSender;

/// How many texts may wait for the writer; more are dropped (D3 review: a
/// phone sending many could otherwise pile up threads and disk writes).
const WAITING: usize = 64;

/// The one writer: it keeps the texts in the order they came (D3 review).
fn writer() -> Option<&'static std::sync::mpsc::SyncSender<ClipEntry>> {
    static WRITER: std::sync::OnceLock<Option<std::sync::mpsc::SyncSender<ClipEntry>>> =
        std::sync::OnceLock::new();
    WRITER
        .get_or_init(|| {
            let dir = crate::state::state_dir().ok()?;
            let (tx, rx) = std::sync::mpsc::sync_channel::<ClipEntry>(WAITING);
            std::thread::Builder::new()
                .name("clip-history".to_string())
                .spawn(move || {
                    for entry in rx {
                        write_one(&dir, entry);
                    }
                })
                .ok()?;
            Some(tx)
        })
        .as_ref()
}

/// Keep one text, as the setting allows, read at each text so a change
/// applies at once.
fn write_one(dir: &std::path::Path, entry: ClipEntry) {
    let Ok(config) = pocketdesk_core::local::config::Config::load() else {
        return;
    };
    let limit = config.clipboard_history;
    // Held to the setting first (D3 review): off empties it, lowered trims it.
    if let Err(e) = clip_history::hold_to(dir, limit) {
        tracing::warn!("Could not hold the clipboard's history to its setting: {e}");
    }
    if limit == 0 {
        return;
    }
    // The same text as the newest again (a phone asking for the computer's
    // clipboard every few seconds): nothing to write.
    if clip_history::read(dir)
        .first()
        .is_some_and(|newest| newest.text == entry.text)
    {
        return;
    }
    if let Err(e) = clip_history::record(dir, entry, limit) {
        tracing::warn!("Could not keep the clipboard's history: {e}");
    }
}

/// Keep `text`, which came from `from` (a phone or computer), or from this
/// computer with `None`; off the caller's thread.
pub(crate) fn note(text: &str, from: Option<&str>) {
    if cfg!(test) || crate::plugins::clipboard::is_pairing_link(text) {
        return;
    }
    let entry = ClipEntry::new(text, from, pocketdesk_core::local::pin::now_unix());
    if let Some(writer) = writer()
        && writer.try_send(entry).is_err()
    {
        tracing::debug!("A text was not kept in the history: too many waiting");
    }
}

/// Daemon task: put each queued text on its phone's clipboard. At start the
/// history is held to the setting, so one left from when it was on goes.
pub async fn run_sends() {
    let Ok(dir) = crate::state::state_dir() else {
        tracing::warn!("No state directory: sending the clipboard's history is off");
        return;
    };
    if let Ok(config) = pocketdesk_core::local::config::Config::load()
        && let Err(e) = clip_history::hold_to(&dir, config.clipboard_history)
    {
        tracing::warn!("Could not hold the clipboard's history to its setting: {e}");
    }
    loop {
        crate::inbox::wait(
            pocketdesk_core::local::paths::CLIPBOARD_OUT_DIR,
            Duration::from_millis(250),
        )
        .await;
        let now = pocketdesk_core::local::pin::now_unix();
        for request in clip_history::take_sends(&dir, now) {
            let clients = crate::state::clone_clients().await;
            let allowed = crate::plugins::pair::permits(&request.device_id, "kdeconnect.clipboard");
            // D5: the clipboard now, read as a push reads it (never a
            // private copy or a pairing link).
            let now_text = async { crate::plugins::clipboard::read_text().await.ok().flatten() };
            match send(&request, &clients, allowed, now_text).await {
                Ok(text) => {
                    // A text from the history is not kept again (D3
                    // review); the clipboard now crossed for the first
                    // time, perhaps (D5 review), and the writer skips it
                    // when it is the newest kept.
                    if request.current {
                        note(&text, None);
                    }
                    tracing::info!("Put a copy on {}'s clipboard", request.device_id)
                }
                Err(reason) => {
                    tracing::info!("{} not sent the copy: {reason}", request.device_id);
                    // Asked from the tray, its window hidden: the refusal
                    // is shown, or the pick seems to do nothing (D5 review).
                    if request.current && !cfg!(test) {
                        let short: String = request.device_id.chars().take(8).collect();
                        let body = format!("Phone {short}: {reason}");
                        std::thread::spawn(move || {
                            crate::platform::notify(
                                "The clipboard was not sent",
                                &body,
                                Duration::from_secs(10),
                            );
                        });
                    }
                }
            }
        }
    }
}

/// Send one text to its phone: allowed and connected, and say what went. A
/// request for the clipboard now (D5) sends what `now_text` reads, only
/// once it may go.
async fn send(
    request: &SendRequest,
    clients: &[Arc<dyn DeviceSender>],
    allowed: bool,
    now_text: impl std::future::Future<Output = Option<String>>,
) -> Result<String, String> {
    if !allowed {
        return Err("Clipboard is off for this phone".to_string());
    }
    let phone = clients
        .iter()
        .find(|c| c.device_id() == request.device_id)
        .ok_or("the phone is not connected")?;
    let text = if request.current {
        // Held as a push is (D5 review): never blank, never past a push's
        // size, which the phone's control line takes.
        let text = now_text.await.filter(|t| !t.trim().is_empty()).ok_or(
            "nothing to send: the clipboard holds no text, or a private copy or a pairing link",
        )?;
        if text.len() > crate::clipboard_push::MAX_PUSH {
            return Err(format!(
                "the clipboard holds more than {} KiB of text, too much to send",
                crate::clipboard_push::MAX_PUSH / 1024
            ));
        }
        text
    } else {
        request.text.clone()
    };
    phone
        .send_packet(&crate::clipboard_push::packet(&text))
        .await
        .map_err(|e| format!("could not send: {e}"))?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::error::Result;
    use pocketdesk_core::protocol::kdeconnect::NetworkPacket;
    use std::sync::Mutex;

    struct Phone {
        id: &'static str,
        sent: Mutex<Vec<NetworkPacket>>,
    }

    #[async_trait::async_trait]
    impl DeviceSender for Phone {
        fn device_id(&self) -> &str {
            self.id
        }
        fn connection_id(&self) -> &str {
            self.id
        }
        async fn send_packet(&self, packet: &NetworkPacket) -> Result<()> {
            self.sent.lock().unwrap().push(packet.clone());
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_text_goes_to_its_phone_only_with_clipboard_on() {
        let a = Arc::new(Phone {
            id: "phone-a",
            sent: Mutex::default(),
        });
        let clients: Vec<Arc<dyn DeviceSender>> = vec![a.clone()];
        let request = SendRequest {
            id: "x".to_string(),
            device_id: "phone-a".to_string(),
            text: "hello".to_string(),
            created_unix: 1,
            current: false,
        };
        let none = || async { None };
        assert_eq!(
            send(&request, &clients, true, none()).await.unwrap(),
            "hello"
        );
        let sent = a.sent.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].packet_type, "kdeconnect.clipboard");
        assert_eq!(sent[0].body["content"], "hello");
        assert!(send(&request, &clients, false, none()).await.is_err());
        let other = SendRequest {
            device_id: "phone-b".to_string(),
            ..request
        };
        assert!(send(&other, &clients, true, none()).await.is_err());
        assert_eq!(a.sent.lock().unwrap().len(), 1);
    }

    /// D5: the clipboard now goes as read, and nothing when there is none
    /// to send (empty, private, a pairing link).
    #[tokio::test]
    async fn the_clipboard_now_goes_as_read() {
        let a = Arc::new(Phone {
            id: "phone-a",
            sent: Mutex::default(),
        });
        let clients: Vec<Arc<dyn DeviceSender>> = vec![a.clone()];
        let request = SendRequest {
            id: "x".to_string(),
            device_id: "phone-a".to_string(),
            text: String::new(),
            created_unix: 1,
            current: true,
        };
        let sent = send(&request, &clients, true, async { Some("now".to_string()) }).await;
        assert_eq!(sent.unwrap(), "now", "what went, for the history");
        assert_eq!(a.sent.lock().unwrap()[0].body["content"], "now");
        let refused = send(&request, &clients, true, async { None }).await;
        assert!(refused.unwrap_err().starts_with("nothing to send"));
        // Blank, or more than a push carries (D5 review).
        let blank = send(&request, &clients, true, async { Some(" \t ".to_string()) }).await;
        assert!(blank.unwrap_err().starts_with("nothing to send"));
        let long = "x".repeat(crate::clipboard_push::MAX_PUSH + 1);
        let refused = send(&request, &clients, true, async move { Some(long) }).await;
        assert!(refused.unwrap_err().contains("too much"));
        assert_eq!(a.sent.lock().unwrap().len(), 1);
    }

    /// D5 review: the clipboard is read only once the send may go.
    #[tokio::test]
    async fn the_clipboard_is_not_read_for_a_send_that_cannot_go() {
        let a = Arc::new(Phone {
            id: "phone-a",
            sent: Mutex::default(),
        });
        let clients: Vec<Arc<dyn DeviceSender>> = vec![a.clone()];
        let mut request = SendRequest {
            id: "x".to_string(),
            device_id: "phone-a".to_string(),
            text: String::new(),
            created_unix: 1,
            current: true,
        };
        let never = || async { panic!("the clipboard was read") };
        assert!(send(&request, &clients, false, never()).await.is_err());
        request.device_id = "phone-b".to_string();
        assert!(send(&request, &clients, true, never()).await.is_err());
    }
}
