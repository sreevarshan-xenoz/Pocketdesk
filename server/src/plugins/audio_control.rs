use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use serde_json::json;

use crate::audio_control;

/// Desktop audio control (Tier-3 #16): volume sync + default-output routing
/// for the phone, straight against the sound server (wpctl → pactl; Hyprland
/// `dispatch setvolume` is unusable upstream — hyprwm/Hyprland#16224).
///
/// Answers `kdeconnect.pocketdesk.audio`:
/// - `{action: "status"}` → `{ok, volume, muted, sink?}`
/// - `{action: "setVolume", volume: 0..100}`
/// - `{action: "setMuted", muted: bool}`
/// - `{action: "sinks"}` → `{ok, sinks: [{index, name, description, isDefault}], default}`
/// - `{action: "selectSink", name}` → routes the default output
/// - `{action: "media"}` → `{ok, playing}`: what is playing (T8c, [`crate::media`])
/// - `{action: "mediaControl", command: "playPause" | "next" | "previous"}`
#[derive(Debug, Default)]
pub struct AudioControlPlugin;

/// The audio request and its reply.
pub const AUDIO: &str = "kdeconnect.pocketdesk.audio";

/// Tell a phone whose Audio permission refused its audio request why, at
/// once: it waits for the reply (8 s) and would otherwise say only that the
/// computer did not answer.
pub(crate) async fn answer_denied(packet: &NetworkPacket, sender: &dyn DeviceSender) {
    if packet.packet_type != AUDIO {
        return;
    }
    let action = packet
        .body
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("status");
    let body = reply(
        action,
        Err("Audio is turned off for this phone on the computer".to_string()),
    );
    let _ = sender
        .send_packet(&NetworkPacket::new(AUDIO).with_body(body))
        .await;
}

#[async_trait::async_trait]
impl Plugin for AudioControlPlugin {
    fn name(&self) -> &'static str {
        "audio_control"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.pocketdesk.audio"]
    }

    // What it answers with: the identity announces it (ecosystem 1g).
    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.pocketdesk.audio"]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type != "kdeconnect.pocketdesk.audio" {
            return Ok(());
        }
        let action = packet
            .body
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("status")
            .to_string();
        let volume = packet
            .body
            .get("volume")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);
        let muted = packet.body.get("muted").and_then(|v| v.as_bool());
        let command = packet
            .body
            .get("command")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let sink = packet
            .body
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        // All helpers spawn blocking CLI processes.
        let action_owned = action.clone();
        let outcome = tokio::task::spawn_blocking(move || match action_owned.as_str() {
            "setVolume" => match volume {
                Some(v) => audio_control::set_volume(v)
                    .map(|_| json!({ "ok": true, "volume": v.min(100) })),
                None => Err("setVolume requires {volume: 0..100}".to_string()),
            },
            "setMuted" => match muted {
                Some(m) => audio_control::set_muted(m).map(|_| json!({ "ok": true, "muted": m })),
                None => Err("setMuted requires {muted: bool}".to_string()),
            },
            "sinks" => audio_control::sinks(),
            "selectSink" => match sink {
                Some(name) => audio_control::select_sink(name).map(|_| json!({ "ok": true })),
                None => Err("selectSink requires {name}".to_string()),
            },
            "media" => crate::media::now_playing().map(crate::media::answer),
            "mediaControl" => match command.as_deref().and_then(crate::media::Command::parse) {
                Some(command) => crate::media::command(command).map(|_| json!({ "ok": true })),
                None => Err("mediaControl takes playPause, next or previous".to_string()),
            },
            _ => audio_control::status(),
        })
        .await;

        let body = reply(&action, outcome.unwrap_or_else(|e| Err(e.to_string())));
        let response = NetworkPacket::new("kdeconnect.pocketdesk.audio").with_body(body);
        sender.send_packet(&response).await
    }
}

/// The reply to `action`: its outcome, `{ok: false, error}` when it failed,
/// with `action` added so the phone can tell replies apart (the bridge
/// matches a reply by its packet type alone).
fn reply(
    action: &str,
    outcome: std::result::Result<serde_json::Value, String>,
) -> serde_json::Value {
    let mut body = match outcome {
        Ok(body) => body,
        Err(e) => {
            // The Now playing card asks every 2 s: a computer with no
            // player tool would otherwise warn as often.
            if action == "media" {
                tracing::debug!("audio-control: {action} failed: {e}");
            } else {
                tracing::warn!("audio-control: {action} failed: {e}");
            }
            json!({ "ok": false, "error": e })
        }
    };
    if let Some(fields) = body.as_object_mut() {
        fields.insert("action".to_string(), json!(action));
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every reply names the action it answers, so a phone that has
    /// several audio requests out takes each reply for its own request.
    #[test]
    fn a_reply_names_its_action_whether_it_worked_or_not() {
        let done = reply("media", Ok(json!({"ok": true, "playing": null})));
        assert_eq!(
            done,
            json!({"ok": true, "playing": null, "action": "media"})
        );
        let failed = reply("mediaControl", Err("nothing is playing".to_string()));
        assert_eq!(
            failed,
            json!({"ok": false, "error": "nothing is playing", "action": "mediaControl"})
        );
    }

    struct Phone(std::sync::Mutex<Vec<NetworkPacket>>);

    #[async_trait::async_trait]
    impl DeviceSender for Phone {
        fn device_id(&self) -> &str {
            "phone-a"
        }
        fn connection_id(&self) -> &str {
            "phone-a"
        }
        async fn send_packet(&self, packet: &NetworkPacket) -> Result<()> {
            self.0.lock().unwrap().push(packet.clone());
            Ok(())
        }
    }

    /// A phone with Audio off is told so at once, not left waiting for a
    /// reply that never comes.
    #[tokio::test]
    async fn a_refused_audio_request_is_answered_with_why() {
        let phone = Phone(Default::default());
        let asked = NetworkPacket::new(AUDIO).with_body(json!({"action": "media"}));
        crate::plugins::answer_denied(&asked, &phone).await;
        let sent = phone.0.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].packet_type, AUDIO);
        assert_eq!(sent[0].body["ok"], false);
        assert_eq!(sent[0].body["action"], "media");
        assert!(sent[0].body["error"].as_str().unwrap().contains("Audio"));
    }

    #[test]
    fn plugin_declares_audio_capability() {
        assert!(
            AudioControlPlugin
                .incoming_capabilities()
                .contains(&"kdeconnect.pocketdesk.audio")
        );
    }
}
