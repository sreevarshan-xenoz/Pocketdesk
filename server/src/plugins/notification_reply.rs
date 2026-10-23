use std::path::PathBuf;

use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use serde_json::json;

use crate::state;

/// Notification reply from the phone (KDE Connect parity, R3 Tier-2 #11c).
///
/// The desktop's captured notifications are pushed to the phone with a
/// stable `id` (see `notification_monitor`); the phone's reply arrives here
/// as `kdeconnect.pocketdesk.notification_reply` `{id, reply}`. (This is our
/// own extension, not KDE Connect's `kdeconnect.notification-reply` — R4 D4
/// moved it under the versioned `kdeconnect.pocketdesk.*` namespace and to a
/// conformant `[a-z_]` name, since KDE's type regex rejects the hyphen and
/// their doc is explicitly not a spec. Both ends ship together; no real KDE
/// Connect phone talks to us yet.) The desktop cannot hand the text back to
/// the originating app — that requires calling private per-app D-Bus objects —
/// so delivery is:
/// append to `~/.local/state/pocketdesk/replies.log`, copy to the desktop
/// clipboard (Cmd-paste into the chat window), and show a confirmation
/// notification naming the app being replied to.
#[derive(Debug, Default)]
pub struct NotificationReplyPlugin;

#[async_trait::async_trait]
impl Plugin for NotificationReplyPlugin {
    fn name(&self) -> &'static str {
        "notification_reply"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.pocketdesk.notification_reply"]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[]
    }

    async fn handle_packet(
        &self,
        packet: &NetworkPacket,
        _sender: &dyn DeviceSender,
    ) -> Result<()> {
        if packet.packet_type != "kdeconnect.pocketdesk.notification_reply" {
            return Ok(());
        }
        let Some(reply) = packet.body.get("reply").and_then(|v| v.as_str()) else {
            return Ok(());
        };
        let id = packet
            .body
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let from = packet.source.as_deref().unwrap_or("phone");

        if let Err(e) = record_reply(id, reply) {
            tracing::warn!("Failed to record notification reply: {e}");
        }
        copy_to_clipboard(reply).await;
        notify_desktop_reply(id, reply, from);
        tracing::info!("Notification reply to {id} from {from}: {reply:?}");
        Ok(())
    }
}

pub fn reply_log_path() -> Result<PathBuf> {
    Ok(state::state_dir()?.join("replies.log"))
}

/// Append `<ts>\t<notification-id>\t<reply>` to the reply log.
fn record_reply(id: &str, reply: &str) -> Result<()> {
    use std::io::Write;
    let path = reply_log_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(file, "{ts}\t{id}\t{}", reply.replace('\t', " "))?;
    Ok(())
}

/// Put the reply text on the desktop clipboard via wl-copy (Wayland) or
/// xclip (X11), as the clipboard's own writes choose (T2 review: an X11
/// box with wl-clipboard installed lost the text to wl-copy). Best-effort:
/// no clipboard tool, no clipboard. With a
/// clipboard stand-in set, the stand-ins take it, never this computer's own
/// clipboard (S1 review).
async fn copy_to_clipboard(text: &str) {
    if crate::plugins::clipboard::stand_ins_set() {
        if let Err(e) = crate::plugins::clipboard::set_clipboard(text).await {
            tracing::debug!("Could not put the notification reply on the stand-in: {e}");
        }
        return;
    }
    #[cfg(windows)]
    {
        let text = text.to_string();
        let set = tokio::task::spawn_blocking(move || {
            crate::platform::clipboard_windows::set_text(&text)
        })
        .await;
        if !matches!(set, Ok(Ok(()))) {
            tracing::debug!("Could not put the notification reply on the clipboard");
        }
    }
    #[cfg(not(windows))]
    if let Err(e) = crate::plugins::clipboard::set_clipboard(text).await {
        tracing::debug!("Could not put the notification reply on the clipboard: {e}");
    }
}

/// Confirmation desktop notification (best effort).
fn notify_desktop_reply(id: &str, reply: &str, from: &str) {
    let app = id.split('|').next().unwrap_or("notification");
    let summary = format!("Replied to {app} ({from})");
    let body = reply.to_string();
    std::thread::spawn(move || {
        crate::platform::notify(&summary, &body, std::time::Duration::from_secs(5));
    });
}

/// Payload the phone sends for a reply (shared shape with the bridge tests).
pub fn reply_payload(id: &str, reply: &str) -> NetworkPacket {
    NetworkPacket::new("kdeconnect.pocketdesk.notification_reply")
        .with_body(json!({ "id": id, "reply": reply, "passive": false }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_packet_roundtrips() {
        let pkt = reply_payload("signal|Alice", "on my way");
        let wire = String::from_utf8(pkt.to_wire().unwrap()).unwrap();
        let parsed = NetworkPacket::from_wire(&wire).unwrap();
        assert_eq!(
            parsed.packet_type,
            "kdeconnect.pocketdesk.notification_reply"
        );
        assert_eq!(parsed.body["reply"], "on my way");
        assert_eq!(parsed.body["id"], "signal|Alice");
    }

    #[test]
    fn app_name_extracted_from_id() {
        assert_eq!("signal", "signal|Meeting at 5".split('|').next().unwrap());
    }

    #[test]
    fn plugin_declares_reply_capability() {
        assert!(
            NotificationReplyPlugin
                .incoming_capabilities()
                .contains(&"kdeconnect.pocketdesk.notification_reply")
        );
    }
}
