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

/// Keep `text`, which came from `from` (a phone or computer), or from this
/// computer with `None`; off the caller's thread.
pub(crate) fn note(text: &str, from: Option<&str>) {
    if cfg!(test) || crate::plugins::clipboard::is_pairing_link(text) {
        return;
    }
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let entry = ClipEntry::new(text, from, pocketdesk_core::local::pin::now_unix());
    std::thread::spawn(move || {
        // One at a time: each reads the file, adds and writes it whole.
        static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
        let limit = match pocketdesk_core::local::config::Config::load() {
            Ok(config) => config.clipboard_history,
            Err(_) => return,
        };
        if limit == 0 {
            return;
        }
        if let Err(e) = clip_history::record(&dir, entry, limit) {
            tracing::warn!("Could not keep the clipboard's history: {e}");
        }
    });
}

/// Daemon task: put each queued text on its phone's clipboard.
pub async fn run_sends() {
    let Ok(dir) = crate::state::state_dir() else {
        tracing::warn!("No state directory: sending the clipboard's history is off");
        return;
    };
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
            match send(&request, &clients, allowed).await {
                Ok(()) => {
                    tracing::info!("Put a copy on {}'s clipboard", request.device_id);
                    note(&request.text, None);
                }
                Err(reason) => tracing::info!("{} not sent the copy: {reason}", request.device_id),
            }
        }
    }
}

/// Send one text to its phone: allowed and connected.
async fn send(
    request: &SendRequest,
    clients: &[Arc<dyn DeviceSender>],
    allowed: bool,
) -> Result<(), String> {
    if !allowed {
        return Err("Clipboard is off for this phone".to_string());
    }
    let phone = clients
        .iter()
        .find(|c| c.device_id() == request.device_id)
        .ok_or("the phone is not connected")?;
    phone
        .send_packet(&crate::clipboard_push::packet(&request.text))
        .await
        .map_err(|e| format!("could not send: {e}"))
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
        };
        send(&request, &clients, true).await.unwrap();
        let sent = a.sent.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].packet_type, "kdeconnect.clipboard");
        assert_eq!(sent[0].body["content"], "hello");
        assert!(send(&request, &clients, false).await.is_err());
        let other = SendRequest {
            device_id: "phone-b".to_string(),
            ..request
        };
        assert!(send(&other, &clients, true).await.is_err());
        assert_eq!(a.sent.lock().unwrap().len(), 1);
    }
}
