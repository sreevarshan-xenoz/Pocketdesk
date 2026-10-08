use linux_link_core::error::Result;
use linux_link_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};

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
                let body = &packet.body;
                let title = body
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Notification");
                let text = body.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let app = body.get("app").and_then(|v| v.as_str()).unwrap_or("Remote");

                if let Err(e) = show_desktop_notification(app, title, text).await {
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

    let conn =
        Connection::session()
            .await
            .map_err(|e| linux_link_core::error::LinuxLinkError::Other {
                detail: format!("D-Bus session failed: {e}"),
            })?;
    let proxy = zbus::Proxy::new(
        &conn,
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
    )
    .await
    .map_err(|e| linux_link_core::error::LinuxLinkError::Other {
        detail: format!("D-Bus proxy failed: {e}"),
    })?;

    proxy
        .call::<_, _, u32>(
            "Notify",
            &(
                "linux-link",                                                      // app_name
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
        .map_err(|e| linux_link_core::error::LinuxLinkError::Other {
            detail: format!("D-Bus call failed: {e}"),
        })?;

    Ok(())
}

/// Show a desktop notification via `notify-send` first, falling back to native D-Bus.
async fn show_desktop_notification(app: &str, title: &str, text: &str) -> Result<()> {
    let summary = format!("{}: {}", app, title);

    // Try notify-send first
    let result = tokio::process::Command::new("notify-send")
        .args([
            &summary,
            text,
            "--app-name",
            "Linux Link",
            "--expire",
            "5000",
        ])
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => {
            tracing::info!("Notification sent via notify-send: {}", title);
            return Ok(());
        }
        _ => {
            tracing::debug!("notify-send failed, trying D-Bus");
        }
    }

    // Fall back to native D-Bus
    #[cfg(target_os = "linux")]
    if let Err(e) = send_notification_dbus(&summary, text).await {
        tracing::warn!("Failed to send notification via D-Bus: {e}");
    } else {
        tracing::info!("Notification sent via D-Bus: {}", title);
    }
    #[cfg(not(target_os = "linux"))]
    tracing::warn!("No desktop notification backend on this OS; dropped: {title}");

    Ok(())
}
