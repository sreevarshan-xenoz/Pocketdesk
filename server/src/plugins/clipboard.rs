use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub struct ClipboardPlugin;

impl Default for ClipboardPlugin {
    fn default() -> Self {
        Self
    }
}

impl ClipboardPlugin {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl Plugin for ClipboardPlugin {
    fn name(&self) -> &'static str {
        "clipboard"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.clipboard", "kdeconnect.clipboard.connect"]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.clipboard"]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        match packet.packet_type.as_str() {
            "kdeconnect.clipboard" => {
                // Remote clipboard update - set local clipboard
                if let Some(content) = packet.body.get("content").and_then(|v| v.as_str())
                    && let Err(e) = set_clipboard(content).await
                {
                    tracing::warn!("Failed to set clipboard: {}", e);
                }
            }
            "kdeconnect.clipboard.connect" => {
                // Remote requested our clipboard - get and send it
                if let Ok(content) = get_clipboard().await {
                    let response = NetworkPacket::new("kdeconnect.clipboard").with_body(json!({
                        "content": content,
                        "timestamp": SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis(),
                    }));
                    sender.send_packet(&response).await?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Set the system clipboard: the Win32 clipboard on Windows.
#[cfg(windows)]
async fn set_clipboard(content: &str) -> Result<()> {
    let content = content.to_string();
    tokio::task::spawn_blocking(move || crate::platform::clipboard_windows::set_text(&content))
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))??;
    Ok(())
}

/// Get the system clipboard's text: the Win32 clipboard on Windows.
#[cfg(windows)]
async fn get_clipboard() -> Result<String> {
    Ok(
        tokio::task::spawn_blocking(crate::platform::clipboard_windows::get_text)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))??,
    )
}

/// Set system clipboard using `wl-copy`.
#[cfg(not(windows))]
async fn set_clipboard(content: &str) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    // wl-copy forks into the background to retain clipboard ownership, so it
    // must not inherit our stdio — a daemon holding the parent's stdout pipe
    // makes test harnesses (and log capture) wait on EOF forever.
    let mut child = tokio::process::Command::new("wl-copy")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| pocketdesk_core::error::PocketdeskError::Io {
            operation: "spawn wl-copy",
            detail: e.to_string(),
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(content.as_bytes()).await?;
        stdin.flush().await?;
    }

    child.wait().await?;
    Ok(())
}

/// Get system clipboard using `wl-paste`.
#[cfg(not(windows))]
async fn get_clipboard() -> Result<String> {
    let output = tokio::process::Command::new("wl-paste")
        .arg("--no-newline")
        .output()
        .await?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(pocketdesk_core::error::PocketdeskError::Io {
            operation: "wl-paste",
            detail: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }
}
