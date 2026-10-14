//! Notifications from a phone shown on this desktop (roadmap N5a), and the
//! answer to a phone asking whether this desktop takes them.
//!
//! A phone that shares its notifications may send many at once; at most
//! [`PER_MINUTE`] a minute from one phone are shown, the rest dropped.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};

/// The most notifications shown from one phone in a minute.
const PER_MINUTE: usize = 20;

/// How many notifications each phone had shown in the last minute.
#[derive(Default)]
struct FloodGuard {
    shown: HashMap<String, Vec<Instant>>,
    /// When a drop was last logged, per phone, so a flood logs once a minute.
    logged: HashMap<String, Instant>,
}

impl FloodGuard {
    /// Whether one more notification from `phone` may be shown at `now`.
    fn admit(&mut self, phone: &str, now: Instant) -> bool {
        let recent = self.shown.entry(phone.to_string()).or_default();
        recent.retain(|at| now.duration_since(*at) < Duration::from_secs(60));
        if recent.len() < PER_MINUTE {
            recent.push(now);
            return true;
        }
        let quiet = self
            .logged
            .get(phone)
            .is_none_or(|at| now.duration_since(*at) >= Duration::from_secs(60));
        if quiet {
            self.logged.insert(phone.to_string(), now);
            tracing::info!("Notifications from {phone} dropped: more than {PER_MINUTE} a minute");
        }
        false
    }
}

fn flood_guard() -> &'static Mutex<FloodGuard> {
    static GUARD: std::sync::OnceLock<Mutex<FloodGuard>> = std::sync::OnceLock::new();
    GUARD.get_or_init(Default::default)
}

/// A notification packet to show: its app, title and text, or `None` for
/// one that only cancels (`isCancel`, N5c) or says nothing.
fn to_show(body: &serde_json::Value) -> Option<(String, String, String)> {
    if body.get("isCancel").and_then(|v| v.as_bool()) == Some(true) {
        return None;
    }
    let field = |key: &str| {
        body.get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let (title, text) = (field("title"), field("text"));
    if title.is_none() && text.is_none() {
        return None;
    }
    Some((
        field("app").unwrap_or_else(|| "Phone".to_string()),
        title.unwrap_or_else(|| "Notification".to_string()),
        text.unwrap_or_default(),
    ))
}

#[derive(Debug)]
pub struct NotificationPlugin;

impl Default for NotificationPlugin {
    fn default() -> Self {
        Self
    }
}

impl NotificationPlugin {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl Plugin for NotificationPlugin {
    fn name(&self) -> &'static str {
        "notification"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.notification", "kdeconnect.notification.request"]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.notification"]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        match packet.packet_type.as_str() {
            "kdeconnect.notification" => {
                let Some((app, title, text)) = to_show(&packet.body) else {
                    return Ok(());
                };
                let admitted = flood_guard()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .admit(sender.device_id(), Instant::now());
                if admitted && let Err(e) = show_desktop_notification(&app, &title, &text).await {
                    tracing::warn!("Failed to show notification: {}", e);
                }
            }
            "kdeconnect.notification.request" => {
                // Acknowledge the request - we support notifications
                let response = NetworkPacket::new("kdeconnect.notification");
                sender.send_packet(&response).await?;
            }
            _ => {}
        }
        Ok(())
    }
}

/// Send a desktop notification via D-Bus (native, no external binary).
#[cfg(target_os = "linux")]
async fn send_notification_dbus(summary: &str, body: &str) -> Result<()> {
    use zbus::Connection;

    let conn = Connection::session().await.map_err(|e| {
        pocketdesk_core::error::PocketdeskError::Other {
            detail: format!("D-Bus session failed: {e}"),
        }
    })?;
    let proxy = zbus::Proxy::new(
        &conn,
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
    )
    .await
    .map_err(|e| pocketdesk_core::error::PocketdeskError::Other {
        detail: format!("D-Bus proxy failed: {e}"),
    })?;

    proxy
        .call::<_, _, u32>(
            "Notify",
            &(
                "pocketdesk",                                                      // app_name
                0u32,                                                              // replaces_id
                "",                                                                // app_icon
                summary,                                                           // summary
                body,                                                              // body
                Vec::<String>::new(),                                              // actions
                std::collections::HashMap::<String, zbus::zvariant::Value>::new(), // hints
                5000i32, // timeout (5 seconds)
            ),
        )
        .await
        .map_err(|e| pocketdesk_core::error::PocketdeskError::Other {
            detail: format!("D-Bus call failed: {e}"),
        })?;

    Ok(())
}

/// Show a desktop notification (notify-send / a Windows toast), falling back
/// to native D-Bus on Linux.
async fn show_desktop_notification(app: &str, title: &str, text: &str) -> Result<()> {
    let summary = format!("{}: {}", app, title);

    let (toast_summary, toast_text) = (summary.clone(), text.to_string());
    let shown = tokio::task::spawn_blocking(move || {
        crate::platform::notify(
            &toast_summary,
            &toast_text,
            std::time::Duration::from_secs(5),
        )
    })
    .await
    .unwrap_or(false);
    if shown {
        tracing::info!("Desktop notification shown: {}", title);
        return Ok(());
    }
    tracing::debug!("desktop notification failed, trying D-Bus");

    // Fall back to native D-Bus
    #[cfg(target_os = "linux")]
    if let Err(e) = send_notification_dbus(&summary, text).await {
        tracing::warn!("Failed to send notification via D-Bus: {e}");
    } else {
        tracing::info!("Notification sent via D-Bus: {}", title);
    }
    #[cfg(not(target_os = "linux"))]
    tracing::warn!("Desktop notification could not be shown; dropped: {title}");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn at_most_twenty_a_minute_are_shown_from_one_phone() {
        let mut guard = FloodGuard::default();
        let start = Instant::now();
        let shown = (0..30).filter(|_| guard.admit("phone", start)).count();
        assert_eq!(shown, PER_MINUTE);
        assert!(
            guard.admit("other", start),
            "another phone has its own count"
        );
        assert!(
            guard.admit("phone", start + Duration::from_secs(61)),
            "a minute later it may show again"
        );
    }

    #[test]
    fn a_cancel_or_an_empty_notification_is_not_shown() {
        assert_eq!(
            to_show(&json!({"app": "Chat", "title": "Ana", "text": "hi"})),
            Some(("Chat".into(), "Ana".into(), "hi".into()))
        );
        assert_eq!(to_show(&json!({"id": "k", "isCancel": true})), None);
        assert_eq!(
            to_show(&json!({"app": "Chat", "title": " ", "text": ""})),
            None
        );
        assert_eq!(
            to_show(&json!({"text": "body only"})),
            Some(("Phone".into(), "Notification".into(), "body only".into()))
        );
    }
}
