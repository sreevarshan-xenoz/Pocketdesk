#[cfg(not(windows))]
use anyhow::{Context, Result};
use tokio::sync::broadcast;

/// A captured desktop notification ready for forwarding.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ForwardedNotification {
    pub app_name: String,
    pub summary: String,
    pub body: String,
    pub urgency: String,
}

impl ForwardedNotification {
    /// Shown by this server itself: a phone's notification (N5a), a pairing
    /// PIN, a received file. Never relayed to phones: a phone's notification
    /// would come back to it and reach every other paired phone, and a PIN
    /// must stay on the desk.
    pub fn is_own(&self) -> bool {
        self.app_name.trim().eq_ignore_ascii_case("pocketdesk")
    }

    /// Serialize to a JSON-encoded KDE Connect notification packet.
    #[allow(dead_code)]
    pub fn to_kdeconnect_payload(&self) -> String {
        let urgency_val = match self.urgency.as_str() {
            "critical" => 2,
            "high" => 1,
            _ => 0,
        };
        serde_json::json!({
            "type": "kdeconnect.notification",
            "body": {
                "id": format!("{}|{}", self.app_name, self.summary),
                "app": self.app_name,
                "title": self.summary,
                "text": self.body,
                "ticker": &self.summary,
                "urgency": urgency_val,
                "isClearable": true,
            }
        })
        .to_string()
    }
}

/// Start the notification monitor task.
///
/// Spawns a `dbus-monitor` subprocess that watches for
/// `org.freedesktop.Notifications.Notify` method calls on the session bus.
/// Captured notifications are forwarded on the returned broadcast sender.
pub fn start_notification_monitor() -> broadcast::Sender<ForwardedNotification> {
    let (tx, _rx) = broadcast::channel::<ForwardedNotification>(128);

    let monitor_tx = tx.clone();
    #[cfg(windows)]
    crate::notification_listener_windows::spawn(monitor_tx);
    #[cfg(not(windows))]
    tokio::spawn(async move {
        if let Err(e) = run_dbus_monitor(monitor_tx).await {
            tracing::warn!("Notification monitor exited: {e}");
        }
    });

    tx
}

#[cfg(not(windows))]
async fn run_dbus_monitor(tx: broadcast::Sender<ForwardedNotification>) -> Result<()> {
    // Try dbus-monitor first (most reliable)
    if let Err(e) = try_dbus_monitor_subprocess(&tx).await {
        tracing::debug!("dbus-monitor failed: {e}, trying gdbus");
        // Fall back to gdbus
        try_gdbus_monitor_subprocess(&tx).await?;
    }
    Ok(())
}

#[cfg(not(windows))]
async fn try_dbus_monitor_subprocess(tx: &broadcast::Sender<ForwardedNotification>) -> Result<()> {
    use tokio::io::AsyncBufReadExt;

    let mut child = tokio::process::Command::new("stdbuf")
        .args([
            "-oL",
            "dbus-monitor",
            "--session",
            "interface=org.freedesktop.Notifications,member=Notify",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("failed to start dbus-monitor with stdbuf")?;

    let stdout = child.stdout.take().context("no stdout from dbus-monitor")?;
    let reader = tokio::io::BufReader::new(stdout);
    let mut lines = reader.lines();

    tracing::info!("Notification monitor started (dbus-monitor)");

    let mut current_app = String::new();
    let mut current_summary = String::new();
    let mut current_body = String::new();
    let mut current_urgency = String::new();
    let mut in_notify = false;
    let mut string_count = 0;

    while let Some(line) = lines.next_line().await.transpose() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("dbus-monitor read error: {e}");
                break;
            }
        };

        // Parse dbus-monitor output for Notify method calls
        let trimmed = line.trim();

        if trimmed.contains("method call") && trimmed.contains("Notify") {
            in_notify = true;
            string_count = 0;
            current_app.clear();
            current_summary.clear();
            current_body.clear();
            current_urgency.clear();
            continue;
        }

        if in_notify {
            if let Some(s) = trimmed.strip_prefix("string \"") {
                let value = unescape_dbus_string(s.trim_end_matches('"'));
                match string_count {
                    0 => current_app = value,
                    // index 1 is app_icon string
                    2 => current_summary = value,
                    3 => current_body = value,
                    _ => {}
                }
                string_count += 1;
            } else if trimmed.starts_with("byte") {
                // Urgency level (0=low, 1=normal, 2=critical)
                let val = trimmed
                    .strip_prefix("byte")
                    .map(|s| s.trim())
                    .unwrap_or("0");
                current_urgency = val.to_string();
            } else if trimmed.starts_with("array [") || trimmed.starts_with("dict entry(") {
                // End of the Notify method call parameters
                if !current_summary.is_empty() {
                    let notification = ForwardedNotification {
                        app_name: std::mem::take(&mut current_app),
                        summary: std::mem::take(&mut current_summary),
                        body: std::mem::take(&mut current_body),
                        urgency: std::mem::take(&mut current_urgency),
                    };
                    let _ = tx.send(notification);
                    in_notify = false;
                }
            }
        }
    }

    // Wait for child to exit
    let _ = child.wait().await;
    Ok(())
}

#[cfg(not(windows))]
async fn try_gdbus_monitor_subprocess(tx: &broadcast::Sender<ForwardedNotification>) -> Result<()> {
    use anyhow::Context;
    use tokio::io::AsyncBufReadExt;

    let mut child = tokio::process::Command::new("gdbus")
        .args([
            "monitor",
            "--session",
            "--dest",
            "org.freedesktop.Notifications",
            "--object-path",
            "/org/freedesktop/Notifications",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("failed to start gdbus")?;

    let stdout = child.stdout.take().context("no stdout from gdbus")?;
    let reader = tokio::io::BufReader::new(stdout);
    let mut lines = reader.lines();

    tracing::info!("Notification monitor started (gdbus)");

    let mut current_app = String::new();
    let mut current_summary = String::new();
    let mut current_body = String::new();
    let mut in_notify = false;
    let mut arg_index = 0usize;
    // Notification params mapping per D-Bus spec:
    // 0: app_name, 1: replaces_id, 2: app_icon, 3: summary, 4: body, 5: actions, 6: hints, 7: expire_timeout

    while let Some(line) = lines.next_line().await.transpose() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("gdbus read error: {e}");
                break;
            }
        };

        let trimmed = line.trim();

        if trimmed.contains("Notify")
            && (trimmed.contains("method call") || trimmed.contains(">>>"))
        {
            in_notify = true;
            arg_index = 0;
            current_app.clear();
            current_summary.clear();
            current_body.clear();
            continue;
        }

        if in_notify {
            // gdbus output format: variant type followed by value
            if let Some(val) = extract_gdbus_string(trimmed) {
                let unescaped = unescape_dbus_string(&val);
                match arg_index {
                    0 => current_app = unescaped,
                    3 => current_summary = unescaped,
                    4 => current_body = unescaped,
                    _ => {}
                }
                arg_index += 1;
            } else if trimmed.contains("}") || trimmed.contains("array") {
                // End of method call params
                if !current_summary.is_empty() {
                    let notification = ForwardedNotification {
                        app_name: std::mem::take(&mut current_app),
                        summary: std::mem::take(&mut current_summary),
                        body: std::mem::take(&mut current_body),
                        urgency: "0".to_string(),
                    };
                    let _ = tx.send(notification);
                    in_notify = false;
                }
            }
        }
    }

    let _ = child.wait().await;
    Ok(())
}

#[cfg(not(windows))]
/// Unescape D-Bus monitor string output.
///
/// dbus-monitor uses C-style escape sequences: \n, \r, \t, \\, \", etc.
/// This function converts them back to actual characters.
fn unescape_dbus_string(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('r') => result.push('\r'),
                Some('t') => result.push('\t'),
                Some('\\') => result.push('\\'),
                Some('"') => result.push('"'),
                Some('\'') => result.push('\''),
                Some('0') => result.push('\0'),
                Some('x') => {
                    // Hex escape \xNN
                    let hex: String = chars.by_ref().take(2).collect();
                    if let Ok(code) = u8::from_str_radix(&hex, 16) {
                        result.push(code as char);
                    }
                }
                Some(c) => {
                    // Unknown escape, keep as-is
                    result.push('\\');
                    result.push(c);
                }
                None => result.push('\\'),
            }
        } else {
            result.push(c);
        }
    }
    result
}

#[cfg(not(windows))]
/// Extract a string value from gdbus monitor output.
fn extract_gdbus_string(line: &str) -> Option<String> {
    // gdbus outputs: "string 'value'" or "string \"value\""
    if let Some(start) = line.find("string '") {
        let val = line[start + 8..].trim_end_matches('\'');
        if !val.is_empty() && !val.contains("'") {
            return Some(val.to_string());
        }
    }
    // Try double-quoted variant
    if let Some(start) = line.find("string \"") {
        let val = line[start + 8..].trim_end_matches('"');
        if !val.is_empty() && !val.contains("\"") {
            return Some(val.to_string());
        }
    }
    None
}

#[cfg(test)]
mod own_tests {
    use super::ForwardedNotification;

    fn from(app: &str) -> ForwardedNotification {
        ForwardedNotification {
            app_name: app.to_string(),
            summary: "s".into(),
            body: "b".into(),
            urgency: "normal".into(),
        }
    }

    #[test]
    fn what_this_server_shows_is_never_relayed_back_to_phones() {
        assert!(from("Pocketdesk").is_own());
        assert!(from(" pocketdesk ").is_own());
        assert!(!from("Mail").is_own());
    }
}
