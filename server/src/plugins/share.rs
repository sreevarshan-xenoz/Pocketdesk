use linux_link_core::error::{LinuxLinkError, Result};
use linux_link_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The largest file a phone may send: a bound on disk use, not a policy.
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// How long to wait for the phone to take the data connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// A transfer that sends nothing for this long is abandoned.
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest file name kept, in characters.
const MAX_NAME_CHARS: usize = 120;

#[derive(Debug)]
pub struct SharePlugin {
    /// Where received files go; `None` when this system has neither a
    /// downloads nor a home folder, and files are then refused.
    download_dir: Option<PathBuf>,
}

impl Default for SharePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl SharePlugin {
    pub fn new() -> Self {
        Self {
            download_dir: dirs::download_dir().or_else(dirs::home_dir),
        }
    }

    #[allow(dead_code)]
    pub fn with_download_dir(dir: PathBuf) -> Self {
        Self {
            download_dir: Some(dir),
        }
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

            // Check if there's port info for a file transfer
            if let Some(transfer_info) = body.get("payloadTransferInfo") {
                let Some(port) = transfer_info
                    .get("port")
                    .and_then(|v| v.as_u64())
                    .and_then(|p| u16::try_from(p).ok())
                    .filter(|p| *p != 0)
                else {
                    tracing::warn!("File share refused: no usable port");
                    return Ok(());
                };
                // The phone listens and the desktop connects, as in KDE
                // Connect. Over TCP the sender is known by its address; the v2
                // channel names a device id, which is not somewhere to dial.
                let Ok(phone_ip) = sender.device_id().parse::<IpAddr>() else {
                    tracing::warn!(
                        "File share refused: only the TCP control channel can carry one"
                    );
                    return Ok(());
                };
                let Some(dir) = self.download_dir.clone() else {
                    tracing::warn!("File share refused: no downloads or home folder");
                    return Ok(());
                };
                let name =
                    safe_file_name(body.get("filename").and_then(|v| v.as_str()).unwrap_or(""));
                let size = packet.payload_size.unwrap_or(0);

                tokio::spawn(async move {
                    match receive_file(SocketAddr::new(phone_ip, port), &dir, &name, size).await {
                        Ok(path) => {
                            tracing::info!("File received: {} ({size} bytes)", path.display())
                        }
                        Err(e) => tracing::warn!("File transfer of {name} failed: {e}"),
                    }
                });
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

/// A file name from the phone made safe to create in the downloads folder:
/// only its last path component, without characters Windows or the shell
/// treat specially, never a reserved device name, never hidden, and short.
/// The phone chooses this text, so it must never name a path.
pub fn safe_file_name(raw: &str) -> String {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = last
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        .collect();
    // Leading dots would hide the file (or make "." and ".."); trailing dots
    // and spaces are dropped by Windows, which could make two names one.
    let trimmed = cleaned
        .trim_start_matches(['.', ' '])
        .trim_end_matches(['.', ' ']);
    let mut name: String = trimmed.chars().take(MAX_NAME_CHARS).collect();
    if name.is_empty() {
        name = "shared_file".to_string();
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        name.insert(0, '_');
    }
    name
}

/// Create a new file for `name` in `dir` that did not exist before:
/// `name`, else `name (1)`, `name (2)`… before the extension. Created
/// exclusively, so an existing file is never overwritten.
async fn create_unique(dir: &Path, name: &str) -> Result<(PathBuf, tokio::fs::File)> {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 0..1000 {
        let candidate = if n == 0 {
            dir.join(name)
        } else {
            dir.join(format!("{stem} ({n}){ext}"))
        };
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
            .await
        {
            Ok(file) => return Ok((candidate, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(io_error("create_file", e)),
        }
    }
    Err(LinuxLinkError::Io {
        operation: "create_file",
        detail: format!("too many files named like {name}"),
    })
}

/// Connect to the phone at `from`, read exactly `size` bytes into a new file
/// in `dir`, and return its path. A transfer that ends short, stalls, or
/// declares no size or too large a size leaves no file behind.
async fn receive_file(from: SocketAddr, dir: &Path, name: &str, size: u64) -> Result<PathBuf> {
    if size == 0 || size > MAX_FILE_BYTES {
        return Err(LinuxLinkError::Io {
            operation: "receive",
            detail: format!("refused a file of declared size {size}"),
        });
    }
    let mut stream = tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(from))
        .await
        .map_err(|_| io_error("connect", "timed out"))?
        .map_err(|e| io_error("connect", e))?;
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| io_error("create_dir", e))?;
    let (path, mut file) = create_unique(dir, name).await?;
    tracing::info!("Receiving {} from {from} ({size} bytes)", path.display());

    let result = async {
        let mut buffer = vec![0u8; 64 * 1024];
        let mut received: u64 = 0;
        while received < size {
            let want = buffer
                .len()
                .min(usize::try_from(size - received).unwrap_or(usize::MAX));
            let n = tokio::time::timeout(IDLE_TIMEOUT, stream.read(&mut buffer[..want]))
                .await
                .map_err(|_| io_error("read", "stalled"))?
                .map_err(|e| io_error("read", e))?;
            if n == 0 {
                return Err(io_error(
                    "read",
                    format!("ended after {received} of {size} bytes"),
                ));
            }
            file.write_all(&buffer[..n])
                .await
                .map_err(|e| io_error("write", e))?;
            received += n as u64;
        }
        file.flush().await.map_err(|e| io_error("write", e))
    }
    .await;

    if let Err(e) = result {
        drop(file);
        let _ = tokio::fs::remove_file(&path).await;
        return Err(e);
    }
    Ok(path)
}

fn io_error(operation: &'static str, detail: impl std::fmt::Display) -> LinuxLinkError {
    LinuxLinkError::Io {
        operation,
        detail: detail.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-share-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_name_from_the_phone_never_names_a_path() {
        for (raw, want) in [
            ("photo.jpg", "photo.jpg"),
            ("../../.bashrc", "bashrc"),
            ("/home/u/.ssh/authorized_keys", "authorized_keys"),
            ("C:\\Windows\\System32\\evil.dll", "evil.dll"),
            ("..\\..\\x.txt", "x.txt"),
            ("a<b>c:d\"e|f?g*h.txt", "abcdefgh.txt"),
            ("CON", "_CON"),
            ("com1.txt", "_com1.txt"),
            ("COM10.txt", "COM10.txt"),
            ("trailing. . .", "trailing"),
            ("", "shared_file"),
            ("..", "shared_file"),
            ("\u{0}\u{7}", "shared_file"),
            ("résumé 2026.pdf", "résumé 2026.pdf"),
        ] {
            assert_eq!(safe_file_name(raw), want, "{raw:?}");
        }
        assert_eq!(
            safe_file_name(&"a".repeat(500)).chars().count(),
            MAX_NAME_CHARS
        );
    }

    #[tokio::test]
    async fn an_existing_file_is_never_overwritten() {
        let dir = temp_dir("unique");
        std::fs::write(dir.join("a.txt"), "keep").unwrap();
        std::fs::write(dir.join("a (1).txt"), "keep too").unwrap();
        let (path, _) = create_unique(&dir, "a.txt").await.unwrap();
        assert_eq!(path, dir.join("a (2).txt"));
        let (path, _) = create_unique(&dir, "noext").await.unwrap();
        assert_eq!(path, dir.join("noext"));
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "keep");
    }

    /// A phone-side listener that sends `data` to whoever connects.
    async fn phone_sending(data: Vec<u8>) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            socket.write_all(&data).await.unwrap();
        });
        addr
    }

    #[tokio::test]
    async fn the_desktop_dials_the_phone_and_saves_exactly_the_declared_bytes() {
        let dir = temp_dir("receive");
        let data: Vec<u8> = (0..200_000u32).map(|i| i as u8).collect();
        let from = phone_sending(data.clone()).await;
        let path = receive_file(from, &dir, "big.bin", data.len() as u64)
            .await
            .unwrap();
        assert_eq!(path, dir.join("big.bin"));
        assert_eq!(std::fs::read(path).unwrap(), data);
    }

    #[tokio::test]
    async fn a_short_transfer_leaves_no_file() {
        let dir = temp_dir("short");
        let from = phone_sending(vec![1; 10]).await;
        assert!(receive_file(from, &dir, "x.bin", 20).await.is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn an_undeclared_or_huge_size_is_refused_before_connecting() {
        let dir = temp_dir("size");
        let unused: SocketAddr = "127.0.0.1:9".parse().unwrap();
        assert!(receive_file(unused, &dir, "x", 0).await.is_err());
        assert!(
            receive_file(unused, &dir, "x", MAX_FILE_BYTES + 1)
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }
}
