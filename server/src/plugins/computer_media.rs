//! The computer's music in the phone's media controls (N5): a phone with
//! its switch on asks to be told (`computer_media_watch {on}`, held to its
//! Audio permission); while any linked phone watches, what plays is read
//! every 2 s and pushed to each on a change and again each minute, with the
//! cover's bytes once per track and phone.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::computer_media::{PUSH, WATCH, art_id};
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use pocketdesk_core::protocol::permissions::Capability;
use serde_json::Value;

/// How often what plays is read while a phone watches.
const LOOK_EVERY: Duration = Duration::from_secs(2);

/// What plays is said again this often unchanged, so a phone knows the
/// link still stands.
pub const AGAIN_EVERY: Duration = Duration::from_secs(60);

/// The phones that asked to be told, by device id.
static WATCHERS: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);
static WOKEN: LazyLock<tokio::sync::Notify> = LazyLock::new(tokio::sync::Notify::new);

/// The phones that asked since the last look: each is told afresh, the
/// cover's bytes included, since one that asks again may hold nothing (an
/// app restarted while the computer still saw its link).
static ASKED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

#[derive(Debug, Default)]
pub struct ComputerMediaPlugin;

#[async_trait::async_trait]
impl Plugin for ComputerMediaPlugin {
    fn name(&self) -> &'static str {
        "computer_media"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[WATCH]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[PUSH]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type != WATCH
            || !crate::plugins::pair::is_paired_device(sender.device_id())
        {
            return Ok(());
        }
        let on = packet.body.get("on").and_then(|v| v.as_bool()) == Some(true);
        let mut watchers = WATCHERS.lock().unwrap_or_else(|e| e.into_inner());
        if on {
            watchers.insert(sender.device_id().to_string());
            ASKED
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(sender.device_id().to_string());
        } else {
            watchers.remove(sender.device_id());
        }
        drop(watchers);
        // A new watcher hears what plays now, not in 2 s.
        WOKEN.notify_one();
        Ok(())
    }
}

/// What each phone was last told, and when.
#[derive(Default)]
struct Pushes {
    told: HashMap<String, Told>,
}

struct Told {
    playing: Value,
    at: Instant,
    art: Option<String>,
}

impl Pushes {
    /// Whether `device` is to be told `playing` (with cover `art`) at `now`,
    /// and if so whether the cover's bytes go with it: a change, or the
    /// minute's word again; the bytes once per cover.
    fn due(
        &mut self,
        device: &str,
        playing: &Value,
        art: Option<&str>,
        now: Instant,
    ) -> Option<bool> {
        let told = self.told.get(device);
        let changed = told.is_none_or(|t| t.playing != *playing || t.art.as_deref() != art);
        let again = told.is_some_and(|t| now.duration_since(t.at) >= AGAIN_EVERY);
        if !changed && !again {
            return None;
        }
        let with_art = art.is_some() && told.is_none_or(|t| t.art.as_deref() != art);
        self.told.insert(
            device.to_string(),
            Told {
                playing: playing.clone(),
                at: now,
                art: art.map(str::to_string),
            },
        );
        Some(with_art)
    }

    /// Forget every phone not in `watching`, and what `asked` were told.
    fn keep_only(&mut self, watching: &HashSet<String>, asked: &HashSet<String>) {
        self.told
            .retain(|id, _| watching.contains(id) && !asked.contains(id));
    }
}

/// The track a cover belongs to, so it is read once a track.
fn track(playing: &Value) -> String {
    let field = |k: &str| {
        playing
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    format!(
        "{}\u{1f}{}\u{1f}{}",
        field("title"),
        field("artist"),
        field("app")
    )
}

/// Daemon task: push what plays to the phones that watch it.
pub async fn run_watcher() {
    let mut pushes = Pushes::default();
    // The cover of the track last read: (its track, its id and bytes).
    let mut cover: Option<(String, Option<(String, String)>)> = None;
    loop {
        let idle = WATCHERS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty();
        let _ = tokio::time::timeout(
            if idle {
                Duration::from_secs(30)
            } else {
                LOOK_EVERY
            },
            WOKEN.notified(),
        )
        .await;
        let clients = crate::state::clone_clients().await;
        let watching: HashSet<String> = {
            let mut watchers = WATCHERS.lock().unwrap_or_else(|e| e.into_inner());
            // A phone that unlinked asks again when it links again.
            watchers.retain(|id| clients.iter().any(|c| c.device_id() == id));
            watchers
                .iter()
                .filter(|id| crate::plugins::pair::allows(id, Capability::Audio))
                .cloned()
                .collect()
        };
        let asked = std::mem::take(&mut *ASKED.lock().unwrap_or_else(|e| e.into_inner()));
        pushes.keep_only(&watching, &asked);
        if watching.is_empty() {
            continue;
        }
        let playing = tokio::task::spawn_blocking(crate::media::now_playing)
            .await
            .ok()
            .and_then(|r| r.ok())
            .flatten();
        let answer = crate::media::answer(playing);
        let playing = answer.get("playing").cloned().unwrap_or(Value::Null);
        let this_track = (!playing.is_null()).then(|| track(&playing));
        if cover.as_ref().map(|(t, _)| Some(t)) != Some(this_track.as_ref()) {
            let read = match &this_track {
                Some(_) => tokio::task::spawn_blocking(crate::media::cover)
                    .await
                    .ok()
                    .flatten()
                    .map(|bytes| {
                        use base64::Engine;
                        (
                            art_id(&bytes),
                            base64::engine::general_purpose::STANDARD.encode(bytes),
                        )
                    }),
                None => None,
            };
            cover = this_track.clone().map(|t| (t, read));
        }
        let art = cover.as_ref().and_then(|(_, a)| a.as_ref());
        let now = Instant::now();
        for client in clients.iter().filter(|c| watching.contains(c.device_id())) {
            let Some(with_art) = pushes.due(
                client.device_id(),
                &playing,
                art.map(|(id, _)| id.as_str()),
                now,
            ) else {
                continue;
            };
            let mut body = serde_json::json!({ "playing": playing });
            if let Some((id, bytes)) = art {
                body["artId"] = Value::from(id.as_str());
                if with_art {
                    body["art"] = Value::from(bytes.as_str());
                }
            }
            if let Err(e) = client
                .send_packet(&NetworkPacket::new(PUSH).with_body(body))
                .await
            {
                tracing::debug!(
                    "The computer's music was not told to {}: {e}",
                    client.device_id()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// N5: a phone is told a change at once, the same again each minute,
    /// and a cover's bytes once.
    #[test]
    fn a_phone_is_told_a_change_and_each_minute() {
        let mut pushes = Pushes::default();
        let t0 = Instant::now();
        let song = json!({"title": "One", "isPlaying": true});
        assert_eq!(
            pushes.due("p", &song, Some("aa"), t0),
            Some(true),
            "first, with its cover"
        );
        assert_eq!(
            pushes.due("p", &song, Some("aa"), t0 + Duration::from_secs(5)),
            None
        );
        assert_eq!(
            pushes.due("p", &song, Some("aa"), t0 + AGAIN_EVERY),
            Some(false),
            "again, no bytes"
        );
        let paused = json!({"title": "One", "isPlaying": false});
        assert_eq!(
            pushes.due("p", &paused, Some("aa"), t0 + AGAIN_EVERY),
            Some(false),
            "a change"
        );
        assert_eq!(
            pushes.due("p", &paused, Some("bb"), t0 + AGAIN_EVERY),
            Some(true),
            "a new cover"
        );
        assert_eq!(
            pushes.due("p", &Value::Null, None, t0 + AGAIN_EVERY),
            Some(false),
            "nothing plays"
        );
        // One that asks again is told afresh, its cover's bytes included.
        let asked = HashSet::from(["p".to_string()]);
        pushes.keep_only(&HashSet::from(["p".to_string()]), &asked);
        assert_eq!(
            pushes.due("p", &paused, Some("bb"), t0 + AGAIN_EVERY),
            Some(true),
            "asked again"
        );
        pushes.keep_only(&HashSet::new(), &HashSet::new());
        assert!(pushes.told.is_empty());
    }
}
