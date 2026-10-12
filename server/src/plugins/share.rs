use linux_link_core::error::Result;
use linux_link_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

#[derive(Debug)]
pub struct SharePlugin {
    download_dir: PathBuf,
}

impl Default for SharePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl SharePlugin {
    pub fn new() -> Self {
        Self {
            download_dir: default_download_dir(),
        }
    }

    #[allow(dead_code)]
    pub fn with_download_dir(dir: PathBuf) -> Self {
        Self { download_dir: dir }
    }
}

#[async_trait::async_trait]
impl Plugin for SharePlugin {
    fn name(&self) -> &'static str {
        "share"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.share.request"]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.share.request"]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type.as_str() == "kdeconnect.share.request" {
            let body = &packet.body;

            // Extract transfer metadata
            let filename = body
                .get("filename")
                .and_then(|v| v.as_str())
                .unwrap_or("shared_file")
                .to_string();

            let total_size = packet.payload_size.unwrap_or(0);

            // Check if there's port info for direct TCP transfer
            if let Some(transfer_info) = body.get("payloadTransferInfo") {
                if let Some(port) = transfer_info.get("port").and_then(|v| v.as_u64()) {
                    let port = port as u16;
                    let filepath = self.download_dir.join(&filename);

                    // Spawn a task to receive the file
                    tokio::spawn(async move {
                        match receive_file(filepath, port, total_size).await {
                            Ok(bytes_received) => {
                                tracing::info!(
                                    "File received: {} ({} bytes)",
                                    filename,
                                    bytes_received
                                );
                            }
                            Err(e) => {
                                tracing::warn!("File transfer failed: {}", e);
                            }
                        }
                    });
                }
            } else if let Some(url) = body.get("url").and_then(|v| v.as_str()) {
                // URL share - just log it (could be enhanced to auto-download)
                tracing::info!("URL shared: {}", url);
                // Send a notification about the URL
                let notification =
                    NetworkPacket::new("kdeconnect.notification").with_body(serde_json::json!({
                        "title": "URL Received",
                        "text": url,
                        "app": "Pocketdesk",
                    }));
                sender.send_packet(&notification).await?;
            }
        }
        Ok(())
    }
}

/// Receive a file from a remote peer over TCP.
async fn receive_file(filepath: PathBuf, port: u16, expected_size: u64) -> Result<u64> {
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    // Ensure download directory exists
    if let Some(parent) = filepath.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            linux_link_core::error::LinuxLinkError::Io {
                operation: "create_dir_all",
                detail: e.to_string(),
            }
        })?;
    }

    let listener = TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .map_err(|e| linux_link_core::error::LinuxLinkError::Io {
            operation: "bind",
            detail: e.to_string(),
        })?;

    let (mut stream, addr) =
        listener
            .accept()
            .await
            .map_err(|e| linux_link_core::error::LinuxLinkError::Io {
                operation: "accept",
                detail: e.to_string(),
            })?;

    tracing::info!(
        "Receiving file from {} ({} bytes expected)",
        addr,
        expected_size
    );

    let mut file = tokio::fs::File::create(&filepath).await.map_err(|e| {
        linux_link_core::error::LinuxLinkError::Io {
            operation: "create_file",
            detail: e.to_string(),
        }
    })?;

    let mut buffer = vec![0u8; 64 * 1024]; // 64KB chunks
    let mut received: u64 = 0;

    loop {
        let n = stream.read(&mut buffer).await.map_err(|e| {
            linux_link_core::error::LinuxLinkError::Io {
                operation: "read",
                detail: e.to_string(),
            }
        })?;

        if n == 0 {
            break;
        }

        file.write_all(&buffer[..n]).await.map_err(|e| {
            linux_link_core::error::LinuxLinkError::Io {
                operation: "write",
                detail: e.to_string(),
            }
        })?;

        received += n as u64;

        // Progress logging every MB
        if received.is_multiple_of(1024 * 1024) {
            tracing::debug!("Received {} MB", received / (1024 * 1024));
        }

        // Safety valve: stop if we've received more than expected
        if expected_size > 0 && received >= expected_size {
            break;
        }
    }

    tracing::info!("File saved to {:?} ({} bytes)", filepath, received);
    Ok(received)
}

fn default_download_dir() -> PathBuf {
    dirs::download_dir().unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()))
}
