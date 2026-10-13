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
/// Longest file name kept, in bytes (Linux allows 255).
const MAX_NAME_BYTES: usize = 200;
/// Transfers running at once; more are refused, not queued.
const MAX_TRANSFERS: usize = 2;
/// The slowest transfer allowed, so a trickle cannot hold a slot forever.
const MIN_BYTES_PER_SEC: u64 = 32 * 1024;
/// The receiver's last word on a transfer, one byte after the data: the
/// file is saved under its real name, or it is not (F7b). Without it, the
/// sender could only say every byte was taken.
const STATUS_SAVED: u8 = 1;
const STATUS_FAILED: u8 = 0;

/// The phone asks for a file (F7b): `{path | offer, port, key}`. It listens
/// on `port`, and the desktop dials it, as for a share, so the bytes go to
/// the transport's peer and nowhere the phone names.
pub const FETCH: &str = "kdeconnect.linuxlink.file_fetch";
/// A download the desktop will not make, and why: `{key, reason}`, so the
/// phone stops waiting at once instead of at its listener's timeout.
pub const FETCH_REFUSED: &str = "kdeconnect.linuxlink.file_fetch_refused";

/// The transfer slots, shared by every connection.
fn transfer_slots() -> &'static std::sync::Arc<tokio::sync::Semaphore> {
    static SLOTS: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
        std::sync::OnceLock::new();
    SLOTS.get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_TRANSFERS)))
}

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
        &["kdeconnect.share.request", FETCH, crate::offers::DECLINE]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[
            "kdeconnect.share.request",
            FETCH_REFUSED,
            crate::offers::OFFER,
        ]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type == FETCH {
            handle_fetch(packet, sender).await;
            return Ok(());
        }
        if packet.packet_type == crate::offers::DECLINE {
            if let Some(id) = packet.body.get("id").and_then(|v| v.as_str()) {
                let reason = packet
                    .body
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                crate::offers::decline(id, &phone_of(sender), reason);
            }
            return Ok(());
        }
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
                // Connect: to the address the transport saw, never to one the
                // peer names (a v2 device id is the peer's own choice).
                let Some(phone_ip) = sender.peer_addr().map(|a| a.ip()) else {
                    tracing::warn!(
                        "File share refused: only the TCP control channel can carry one"
                    );
                    return Ok(());
                };
                if !dialable(phone_ip) {
                    tracing::warn!("File share refused: will not connect back to {phone_ip}");
                    return Ok(());
                }
                let Some(dir) = self.download_dir.clone() else {
                    tracing::warn!("File share refused: no downloads or home folder");
                    return Ok(());
                };
                let name =
                    safe_file_name(body.get("filename").and_then(|v| v.as_str()).unwrap_or(""));
                let size = packet.payload_size.unwrap_or(0);
                // The phone hands the bytes only to whoever presents the key
                // it put in this request, so they reach this desktop and no
                // one else on the network.
                let Some(key) = transfer_info
                    .get("key")
                    .and_then(|v| v.as_str())
                    .filter(|k| is_transfer_key(k))
                    .map(str::to_string)
                else {
                    tracing::warn!("File share of {name} refused: no transfer key");
                    return Ok(());
                };
                // The bytes travel inside TLS with the phone that asked
                // (phase C): the key its control connection proved.
                let tls = match transfer_tls(sender) {
                    Ok(tls) => tls,
                    Err(reason) => {
                        tracing::warn!("File share of {name} refused: {reason}");
                        return Ok(());
                    }
                };
                let Ok(slot) = transfer_slots().clone().try_acquire_owned() else {
                    tracing::warn!("File share of {name} refused: {MAX_TRANSFERS} already running");
                    return Ok(());
                };

                let from = sender.device_id().to_string();
                tokio::spawn(async move {
                    let _slot = slot;
                    match receive_file(
                        SocketAddr::new(phone_ip, port),
                        tls,
                        &key,
                        &dir,
                        &name,
                        size,
                    )
                    .await
                    {
                        Ok(path) => {
                            tracing::info!("File received: {} ({size} bytes)", path.display());
                            on_received(&path, size, &from);
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

/// A download the phone asked for: check it, then send the file from a task
/// of its own, or tell the phone why not.
async fn handle_fetch(packet: &NetworkPacket, sender: &dyn DeviceSender) {
    // Without a key there is nothing to address a refusal to.
    let Some(key) = packet
        .body
        .get("key")
        .and_then(|v| v.as_str())
        .filter(|k| is_transfer_key(k))
        .map(str::to_string)
    else {
        tracing::warn!("Download refused: no transfer key");
        return;
    };
    match plan_fetch(&packet.body, sender) {
        Ok(fetch) => {
            tracing::info!("Sending {} to {}", fetch.path.display(), fetch.to);
            tokio::spawn(async move {
                let _slot = fetch.slot;
                let sent = send_file_to(fetch.to, fetch.tls, &key, &fetch.path).await;
                match &sent {
                    Ok(size) => {
                        tracing::info!("File sent: {} ({size} bytes)", fetch.path.display())
                    }
                    Err(e) => tracing::warn!("Sending {} failed: {e}", fetch.path.display()),
                }
                if let Some(id) = fetch.offer {
                    crate::offers::finish(&id, sent.map(|_| ()).map_err(|e| e.to_string()));
                }
            });
        }
        Err(reason) => {
            tracing::warn!("Download refused: {reason}");
            refuse(sender, &key, &reason).await;
        }
    }
}

/// A download that passed every check, holding its transfer slot.
struct Fetch {
    to: SocketAddr,
    /// The transfer's TLS, with the phone that asked.
    tls: std::sync::Arc<tokio_rustls::rustls::ClientConfig>,
    path: PathBuf,
    /// The desktop app's send this download answers, if any.
    offer: Option<String>,
    slot: tokio::sync::OwnedSemaphorePermit,
}

fn plan_fetch(
    body: &serde_json::Value,
    sender: &dyn DeviceSender,
) -> std::result::Result<Fetch, String> {
    let port = body
        .get("port")
        .and_then(|v| v.as_u64())
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p != 0)
        .ok_or("no port to send to")?;
    let ip = sender
        .peer_addr()
        .map(|a| a.ip())
        .ok_or("downloads need the local network connection")?;
    if !dialable(ip) {
        return Err(format!("will not connect back to {ip}"));
    }
    // Taken before an offer is redeemed, so a busy desktop does not use up
    // the phone's one try.
    let slot = transfer_slots()
        .clone()
        .try_acquire_owned()
        .map_err(|_| format!("{MAX_TRANSFERS} transfers are already running"))?;
    let (path, size, offer) = if let Some(path) = body.get("path").and_then(|v| v.as_str()) {
        let (path, size) = crate::plugins::file_browse::resolve_file(path)?;
        (path, size, None)
    } else if let Some(id) = body.get("offer").and_then(|v| v.as_str()) {
        let path = crate::offers::redeem(id, &phone_of(sender))?;
        let Some(size) = std::fs::metadata(&path)
            .ok()
            .filter(|m| m.is_file())
            .map(|m| m.len())
        else {
            let reason = "the file is no longer there";
            crate::offers::finish(id, Err(reason.to_string()));
            return Err(reason.to_string());
        };
        (path, size, Some(id.to_string()))
    } else {
        return Err("no file named".to_string());
    };
    if size > MAX_FILE_BYTES {
        let reason = "the file is larger than 8 GB";
        if let Some(id) = &offer {
            crate::offers::finish(id, Err(reason.to_string()));
        }
        return Err(reason.to_string());
    }
    // Last, after every check that can refuse: it reads this desktop's
    // certificate.
    let tls = match transfer_tls(sender) {
        Ok(tls) => tls,
        Err(reason) => {
            if let Some(id) = &offer {
                crate::offers::finish(id, Err(reason.clone()));
            }
            return Err(reason);
        }
    };
    Ok(Fetch {
        to: SocketAddr::new(ip, port),
        tls,
        path,
        offer,
        slot,
    })
}

/// This desktop's side of a transfer's TLS with the phone behind `sender`
/// (phase C): this desktop's certificate, and the phone must present the
/// key its control connection proved. A connection that proved none gets
/// no transfer.
fn transfer_tls(
    sender: &dyn DeviceSender,
) -> std::result::Result<std::sync::Arc<tokio_rustls::rustls::ClientConfig>, String> {
    let Some(key @ linux_link_core::streaming::PeerKey::Tls(phone)) = sender.peer_key() else {
        return Err("the phone's connection proved no key".to_string());
    };
    crate::plugins::pair::may_transfer(sender.device_id(), &key)?;
    Ok(desktop_certs()?.transfer_client_config(phone))
}

/// This desktop's identity, read once: the one its control channel and
/// stream present (loaded at start by the service from the same folder).
fn desktop_certs()
-> std::result::Result<std::sync::Arc<linux_link_core::streaming::transport::CertManager>, String> {
    use linux_link_core::streaming::transport::CertManager;
    static CERTS: std::sync::Mutex<Option<std::sync::Arc<CertManager>>> =
        std::sync::Mutex::new(None);
    let mut cached = CERTS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(certs) = cached.as_ref() {
        return Ok(certs.clone());
    }
    let dir = crate::state::state_dir()
        .map_err(|e| e.to_string())?
        .join("certs");
    let certs = std::sync::Arc::new(
        CertManager::load_or_create(&dir)
            .map_err(|e| format!("this desktop's certificate: {e}"))?,
    );
    *cached = Some(certs.clone());
    Ok(certs)
}

/// Dial the phone at `to` and open the transfer's TLS.
async fn dial_transfer(
    to: SocketAddr,
    tls: std::sync::Arc<tokio_rustls::rustls::ClientConfig>,
) -> Result<tokio_rustls::client::TlsStream<tokio::net::TcpStream>> {
    let tcp = tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(to))
        .await
        .map_err(|_| io_error("connect", "timed out"))?
        .map_err(|e| io_error("connect", e))?;
    let name = tokio_rustls::rustls::pki_types::ServerName::IpAddress(to.ip().into());
    let stream = tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio_rustls::TlsConnector::from(tls).connect(name, tcp),
    )
    .await
    .map_err(|_| io_error("tls", "timed out"))?
    .map_err(|e| io_error("tls", e))?;
    if stream.get_ref().1.alpn_protocol() != Some(linux_link_core::protocol::ALPN_TRANSFER) {
        return Err(io_error(
            "tls",
            "the phone does not speak the transfer protocol",
        ));
    }
    Ok(stream)
}

/// The phone behind `sender`: a connection goes by its device's id (phase B).
fn phone_of(sender: &dyn DeviceSender) -> String {
    sender.device_id().to_string()
}

async fn refuse(sender: &dyn DeviceSender, key: &str, reason: &str) {
    let packet = NetworkPacket::new(FETCH_REFUSED)
        .with_body(serde_json::json!({ "key": key, "reason": reason }));
    if let Err(e) = sender.send_packet(&packet).await {
        tracing::debug!("Could not tell the phone the download was refused: {e}");
    }
}

/// Send the file at `path` to the phone listening at `to`, and return its
/// size once the phone says it saved it.
pub(crate) async fn send_file_to(
    to: SocketAddr,
    tls: std::sync::Arc<tokio_rustls::rustls::ClientConfig>,
    key: &str,
    path: &Path,
) -> Result<u64> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| io_error("open", e))?;
    let size = file
        .metadata()
        .await
        .map_err(|e| io_error("open", e))?
        .len();
    if size > MAX_FILE_BYTES {
        return Err(io_error("send", "larger than 8 GB"));
    }
    send_reader(to, tls, key, &mut file, size).await
}

/// Dial `to`, present `key`, write `size` (u64, big-endian) and exactly
/// that many bytes from `reader`, then wait for the phone's status byte.
/// A reader that ends early (the file shrank) fails the send.
async fn send_reader<R: tokio::io::AsyncRead + Unpin>(
    to: SocketAddr,
    tls: std::sync::Arc<tokio_rustls::rustls::ClientConfig>,
    key: &str,
    reader: &mut R,
    size: u64,
) -> Result<u64> {
    let send = async {
        let mut stream = dial_transfer(to, tls).await?;
        stream
            .write_all(format!("{key}\n").as_bytes())
            .await
            .map_err(|e| io_error("write", e))?;
        stream
            .write_u64(size)
            .await
            .map_err(|e| io_error("write", e))?;
        let mut buffer = vec![0u8; 64 * 1024];
        let mut sent: u64 = 0;
        while sent < size {
            let want = buffer
                .len()
                .min(usize::try_from(size - sent).unwrap_or(usize::MAX));
            let n = reader
                .read(&mut buffer[..want])
                .await
                .map_err(|e| io_error("read", e))?;
            if n == 0 {
                return Err(io_error(
                    "read",
                    format!("the file changed while it was sent ({sent} of {size} bytes)"),
                ));
            }
            tokio::time::timeout(IDLE_TIMEOUT, stream.write_all(&buffer[..n]))
                .await
                .map_err(|_| io_error("write", "stalled"))?
                .map_err(|e| io_error("write", e))?;
            sent += n as u64;
        }
        stream.flush().await.map_err(|e| io_error("write", e))?;
        let mut status = [0u8; 1];
        tokio::time::timeout(IDLE_TIMEOUT, stream.read_exact(&mut status))
            .await
            .map_err(|_| io_error("read", "the phone did not confirm the file"))?
            .map_err(|_| io_error("read", "the phone did not confirm the file"))?;
        if status[0] != STATUS_SAVED {
            return Err(io_error("send", "the phone could not save the file"));
        }
        Ok(size)
    };
    tokio::time::timeout(transfer_deadline(size), send)
        .await
        .map_err(|_| io_error("write", "too slow"))?
}

/// Tell the owner a file arrived: a desktop notification, and a row in the
/// received-files list the desktop app shows (roadmap F7).
fn on_received(path: &Path, size: u64, from: &str) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Ok(dir) = crate::state::state_dir() {
        record_received_in(&dir, path, &name, size, from);
    }
    let folder = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let body = format!("{name} ({}) is in {folder}.", human_size(size));
    std::thread::spawn(move || {
        let summary = "Pocketdesk: file received";
        if !crate::platform::notify(summary, &body, Duration::from_secs(8)) {
            tracing::info!("{summary} — {body}");
        }
    });
}

fn record_received_in(state_dir: &Path, path: &Path, name: &str, size: u64, from: &str) {
    // Transfers run concurrently; the list is read, changed and replaced.
    static LIST: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LIST.lock().unwrap_or_else(|e| e.into_inner());
    let entry = linux_link_core::local::received::ReceivedFile {
        name: name.to_string(),
        path: path.display().to_string(),
        size,
        received_unix: linux_link_core::local::pin::now_unix(),
        from: from.to_string(),
    };
    if let Err(e) = linux_link_core::local::received::record(state_dir, entry) {
        tracing::warn!("Could not record the received file: {e}");
    }
}

/// `2_100_000` -> "2.0 MB".
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// A transfer key as the phone makes it: 32 hex digits (a random UUID).
fn is_transfer_key(key: &str) -> bool {
    key.len() == 32 && key.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether the desktop may connect back to a phone at `ip`: not itself
/// (a loopback peer means a local proxy such as `adb reverse`, and the
/// phone's port would name a service on this machine), and not an address
/// that is no single host or is the link-local range (cloud metadata).
fn dialable(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            !(v4.is_loopback()
                || v4.is_unspecified()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_broadcast())
        }
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => dialable(IpAddr::V4(v4)),
            None => {
                !(v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_multicast()
                    || v6.is_unicast_link_local())
            }
        },
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
        .filter(|c| {
            !c.is_control()
                && !is_invisible_format(*c)
                && !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*')
        })
        .collect();
    let mut name = fit_name(trim_name(&cleaned));
    if name.is_empty() {
        name = "shared_file".to_string();
    }
    if is_reserved_name(&name) {
        name.insert(0, '_');
    }
    name
}

/// Leading dots would hide the file (or make "." and ".."); trailing dots
/// and spaces are dropped by Windows, which could make two names one.
fn trim_name(name: &str) -> &str {
    name.trim_start_matches(['.', ' '])
        .trim_end_matches(['.', ' '])
}

/// Characters that change how a name reads without being seen: a
/// right-to-left override makes "invoice<RLO>fdp.exe" read as
/// "invoiceexe.pdf".
fn is_invisible_format(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}'
    )
}

/// At most [`MAX_NAME_BYTES`], keeping a short extension: a long name loses
/// the end of its stem, not what opens it.
fn fit_name(name: &str) -> String {
    if name.len() <= MAX_NAME_BYTES {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 && name.len() - i <= 16 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    let budget = MAX_NAME_BYTES - ext.len();
    let mut cut = String::new();
    for c in stem.chars() {
        if cut.len() + c.len_utf8() > budget {
            break;
        }
        cut.push(c);
    }
    format!("{}{ext}", trim_name(&cut))
}

/// Windows device names, which name a device in any folder and with any
/// extension: CON, PRN, AUX, NUL, CONIN$, CONOUT$, COM0-9 and LPT0-9
/// (superscript digits too), ignoring case and spaces before the dot.
pub(crate) fn is_reserved_name(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ')
        .to_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }
    let mut chars = stem.chars();
    let prefix: String = chars.by_ref().take(3).collect();
    let digit = chars.next();
    (prefix == "COM" || prefix == "LPT")
        && chars.next().is_none()
        && digit.is_some_and(|d| d.is_ascii_digit() || matches!(d, '¹' | '²' | '³'))
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

/// A transfer in progress: its reserved final name and the `.part` file the
/// bytes go to. Unless committed, both go when this is dropped, also when
/// the task is cancelled, so no half file is ever left under a real name.
struct Incoming {
    final_path: PathBuf,
    part_path: PathBuf,
    committed: bool,
}

impl Drop for Incoming {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.part_path);
            let _ = std::fs::remove_file(&self.final_path);
        }
    }
}

/// The whole transfer must finish within this, so a peer cannot hold a
/// slot open by sending a byte now and then.
fn transfer_deadline(size: u64) -> Duration {
    Duration::from_secs(30 + size / MIN_BYTES_PER_SEC)
}

/// Mark a received file as coming from another computer, so Windows warns
/// before running it, as it does for a browser download.
#[cfg(windows)]
fn mark_of_the_web(path: &Path) {
    let mut stream = path.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    if let Err(e) = std::fs::write(stream, "[ZoneTransfer]\r\nZoneId=3\r\n") {
        tracing::warn!("Could not mark {} as downloaded: {e}", path.display());
    }
}

#[cfg(not(windows))]
fn mark_of_the_web(_path: &Path) {}

/// Connect to the phone at `from`, read exactly `size` bytes into a new file
/// in `dir`, and return its path. A transfer that ends short, stalls, runs
/// too slow, or declares no size or too large a size leaves no file behind.
async fn receive_file(
    from: SocketAddr,
    tls: std::sync::Arc<tokio_rustls::rustls::ClientConfig>,
    key: &str,
    dir: &Path,
    name: &str,
    size: u64,
) -> Result<PathBuf> {
    if size == 0 || size > MAX_FILE_BYTES {
        return Err(LinuxLinkError::Io {
            operation: "receive",
            detail: format!("refused a file of declared size {size}"),
        });
    }
    let mut stream = dial_transfer(from, tls).await?;
    stream
        .write_all(format!("{key}\n").as_bytes())
        .await
        .map_err(|e| io_error("write", e))?;
    stream.flush().await.map_err(|e| io_error("write", e))?;
    let saved = save_stream(&mut stream, from, dir, name, size).await;
    // Best effort: a phone that is already gone has nothing to hear.
    let status = if saved.is_ok() {
        STATUS_SAVED
    } else {
        STATUS_FAILED
    };
    // Flushed: inside TLS the byte otherwise waits in the session's buffer.
    let _ = tokio::time::timeout(CONNECT_TIMEOUT, async {
        stream.write_all(&[status]).await?;
        stream.flush().await
    })
    .await;
    saved
}

/// Read exactly `size` bytes from `stream` into a new file in `dir` named
/// like `name`, and return its path; on any failure no file is left.
async fn save_stream<S: tokio::io::AsyncRead + Unpin>(
    stream: &mut S,
    from: SocketAddr,
    dir: &Path,
    name: &str,
    size: u64,
) -> Result<PathBuf> {
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| io_error("create_dir", e))?;
    let (final_path, placeholder) = create_unique(dir, name).await?;
    drop(placeholder);
    let mut part_path = final_path.as_os_str().to_owned();
    part_path.push(".part");
    let mut incoming = Incoming {
        final_path,
        part_path: PathBuf::from(part_path),
        committed: false,
    };
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&incoming.part_path)
        .await
        .map_err(|e| io_error("create_file", e))?;
    tracing::info!(
        "Receiving {} from {from} ({size} bytes)",
        incoming.final_path.display()
    );

    let copy = async {
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
    };
    tokio::time::timeout(transfer_deadline(size), copy)
        .await
        .map_err(|_| io_error("read", "too slow"))??;
    drop(file);
    tokio::fs::rename(&incoming.part_path, &incoming.final_path)
        .await
        .map_err(|e| io_error("rename", e))?;
    incoming.committed = true;
    mark_of_the_web(&incoming.final_path);
    Ok(incoming.final_path.clone())
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
    fn the_desktop_connects_back_only_to_a_real_remote_host() {
        for ok in ["192.168.1.20", "10.0.0.2", "100.64.1.2", "2001:db8::1"] {
            assert!(dialable(ok.parse().unwrap()), "{ok}");
        }
        for no in [
            "127.0.0.1",
            "::1",
            "0.0.0.0",
            "169.254.169.254",
            "fe80::1",
            "224.0.0.1",
            "255.255.255.255",
            "::ffff:127.0.0.1",
        ] {
            assert!(!dialable(no.parse().unwrap()), "{no}");
        }
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
            ("con .txt", "_con .txt"),
            ("COM¹.log", "_COM¹.log"),
            ("CONIN$", "_CONIN$"),
            ("invoice\u{202E}fdp.exe", "invoicefdp.exe"),
            ("a\u{200B}b.txt", "ab.txt"),
            ("trailing. . .", "trailing"),
            ("", "shared_file"),
            ("..", "shared_file"),
            ("\u{0}\u{7}", "shared_file"),
            ("résumé 2026.pdf", "résumé 2026.pdf"),
        ] {
            assert_eq!(safe_file_name(raw), want, "{raw:?}");
        }
        assert_eq!(safe_file_name(&"a".repeat(500)).len(), MAX_NAME_BYTES);
        let long = format!("{}.pdf", "文".repeat(200));
        let fitted = safe_file_name(&long);
        assert!(
            fitted.len() <= MAX_NAME_BYTES && fitted.ends_with(".pdf"),
            "{fitted}"
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

    const KEY: &str = "0123456789abcdef0123456789abcdef";

    use linux_link_core::streaming::transport::CertManager;

    /// This desktop, the phone that asks, and another device.
    fn certs() -> &'static (CertManager, CertManager, CertManager) {
        static CERTS: std::sync::OnceLock<(CertManager, CertManager, CertManager)> =
            std::sync::OnceLock::new();
        CERTS.get_or_init(|| {
            (
                CertManager::new().unwrap(),
                CertManager::new().unwrap(),
                CertManager::new().unwrap(),
            )
        })
    }

    /// The desktop's side of a transfer with the phone that asked.
    fn desktop_tls() -> std::sync::Arc<tokio_rustls::rustls::ClientConfig> {
        let (desktop, phone, _) = certs();
        desktop.transfer_client_config(phone.cert_fingerprint())
    }

    /// Accept one connection as `device` (the phone that asked, or another),
    /// expecting this desktop.
    async fn accept_as(
        device: &CertManager,
        listener: &TcpListener,
    ) -> std::io::Result<tokio_rustls::server::TlsStream<tokio::net::TcpStream>> {
        let (desktop, _, _) = certs();
        let (tcp, _) = listener.accept().await?;
        tokio_rustls::TlsAcceptor::from(
            device
                .transfer_server_config(desktop.cert_fingerprint())
                .unwrap(),
        )
        .accept(tcp)
        .await
    }

    /// A phone-side listener that sends `data` to whoever presents [`KEY`],
    /// then reads the desktop's status byte (`None`: it sent none).
    async fn phone_sending(data: Vec<u8>) -> (SocketAddr, tokio::task::JoinHandle<Option<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let status = tokio::spawn(async move {
            let mut socket = accept_as(&certs().1, &listener).await.unwrap();
            let mut presented = [0u8; 33];
            socket.read_exact(&mut presented).await.unwrap();
            assert_eq!(&presented, format!("{KEY}\n").as_bytes());
            socket.write_all(&data).await.unwrap();
            // TLS's close_notify, then the socket's write side; the status
            // still comes back.
            socket.shutdown().await.unwrap();
            let mut status = [0u8; 1];
            socket.read_exact(&mut status).await.ok().map(|_| status[0])
        });
        (addr, status)
    }

    #[test]
    fn a_received_file_is_listed_for_the_desktop_app() {
        let state = temp_dir("list");
        let path = state.join("photo.jpg");
        record_received_in(&state, &path, "photo.jpg", 2_100_000, "192.0.2.7");
        let files = linux_link_core::local::received::read(&state);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "photo.jpg");
        assert_eq!(files[0].size, 2_100_000);
        assert_eq!(files[0].path, path.display().to_string());
        assert_eq!(human_size(2_100_000), "2.0 MB");
        assert_eq!(human_size(512), "512 B");
    }

    #[test]
    fn a_transfer_key_is_32_hex_digits() {
        assert!(is_transfer_key(KEY));
        assert!(!is_transfer_key(""));
        assert!(!is_transfer_key(&KEY[1..]));
        assert!(!is_transfer_key("0123456789abcdef0123456789abcdeg"));
    }

    #[tokio::test]
    async fn the_desktop_dials_the_phone_and_saves_exactly_the_declared_bytes() {
        let dir = temp_dir("receive");
        let data: Vec<u8> = (0..200_000u32).map(|i| i as u8).collect();
        let (from, status) = phone_sending(data.clone()).await;
        let path = receive_file(from, desktop_tls(), KEY, &dir, "big.bin", data.len() as u64)
            .await
            .unwrap();
        assert_eq!(path, dir.join("big.bin"));
        assert_eq!(
            status.await.unwrap(),
            Some(STATUS_SAVED),
            "told it was saved"
        );
        assert_eq!(std::fs::read(path).unwrap(), data);
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["big.bin"], "no .part left behind");
    }

    #[tokio::test]
    async fn a_short_transfer_leaves_no_file() {
        let dir = temp_dir("short");
        let (from, status) = phone_sending(vec![1; 10]).await;
        assert!(
            receive_file(from, desktop_tls(), KEY, &dir, "x.bin", 20)
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        assert_eq!(status.await.unwrap(), Some(STATUS_FAILED), "told it failed");
    }

    #[test]
    fn a_slow_trickle_has_a_deadline() {
        assert_eq!(transfer_deadline(1), Duration::from_secs(30));
        assert_eq!(
            transfer_deadline(MIN_BYTES_PER_SEC * 100),
            Duration::from_secs(130)
        );
    }

    #[tokio::test]
    async fn an_undeclared_or_huge_size_is_refused_before_connecting() {
        let dir = temp_dir("size");
        let unused: SocketAddr = "127.0.0.1:9".parse().unwrap();
        assert!(
            receive_file(unused, desktop_tls(), KEY, &dir, "x", 0)
                .await
                .is_err()
        );
        assert!(
            receive_file(unused, desktop_tls(), KEY, &dir, "x", MAX_FILE_BYTES + 1)
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }

    /// A phone-side listener for a download: checks the key, reads the
    /// size and the bytes, and answers `status`. Yields what it received.
    async fn phone_fetching(status: u8) -> (SocketAddr, tokio::task::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let got = tokio::spawn(async move {
            let mut socket = accept_as(&certs().1, &listener).await.unwrap();
            let mut presented = [0u8; 33];
            socket.read_exact(&mut presented).await.unwrap();
            assert_eq!(&presented, format!("{KEY}\n").as_bytes());
            let size = socket.read_u64().await.unwrap();
            let mut data = vec![0u8; size as usize];
            socket.read_exact(&mut data).await.unwrap();
            socket.write_all(&[status]).await.unwrap();
            socket.flush().await.unwrap();
            data
        });
        (addr, got)
    }

    #[tokio::test]
    async fn a_download_sends_the_size_then_the_bytes_and_waits_for_the_save() {
        let dir = temp_dir("fetch");
        let data: Vec<u8> = (0..150_000u32).map(|i| (i * 7) as u8).collect();
        let path = dir.join("a.bin");
        std::fs::write(&path, &data).unwrap();
        let (to, got) = phone_fetching(STATUS_SAVED).await;
        assert_eq!(
            send_file_to(to, desktop_tls(), KEY, &path).await.unwrap(),
            data.len() as u64
        );
        assert_eq!(got.await.unwrap(), data);
    }

    #[tokio::test]
    async fn a_download_the_phone_could_not_save_fails() {
        let dir = temp_dir("fetch-fail");
        let path = dir.join("a.bin");
        std::fs::write(&path, b"abc").unwrap();
        let (to, _) = phone_fetching(STATUS_FAILED).await;
        assert!(send_file_to(to, desktop_tls(), KEY, &path).await.is_err());
    }

    #[tokio::test]
    async fn a_file_that_shrinks_fails_the_send() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let to = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut socket = accept_as(&certs().1, &listener).await.unwrap();
            let mut sink = Vec::new();
            let _ = socket.read_to_end(&mut sink).await;
        });
        // Listed at 10 bytes, holding 4 by the time it is read.
        let sent = send_reader(to, desktop_tls(), KEY, &mut &b"abcd"[..], 10).await;
        assert!(sent.is_err(), "{sent:?}");
    }

    #[tokio::test]
    async fn a_device_other_than_the_phone_that_asked_gets_nothing() {
        let dir = temp_dir("fetch-impostor");
        let path = dir.join("a.bin");
        std::fs::write(&path, b"secret").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let to = listener.local_addr().unwrap();
        let impostor = tokio::spawn(async move {
            let Ok(mut socket) = accept_as(&certs().2, &listener).await else {
                return Vec::new();
            };
            let mut got = Vec::new();
            let _ = socket.read_to_end(&mut got).await;
            got
        });
        assert!(send_file_to(to, desktop_tls(), KEY, &path).await.is_err());
        assert!(impostor.await.unwrap().is_empty(), "not a byte of it");
    }

    /// The phone's control connection, as the plugin sees it.
    struct Phone {
        peer: SocketAddr,
        sent: std::sync::Mutex<Vec<NetworkPacket>>,
    }

    impl Phone {
        fn at(peer: &str) -> Self {
            Self {
                peer: peer.parse().unwrap(),
                sent: Default::default(),
            }
        }

        fn replies(&self) -> Vec<NetworkPacket> {
            self.sent.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl DeviceSender for Phone {
        fn device_id(&self) -> &str {
            "phone"
        }
        fn connection_id(&self) -> &str {
            "conn"
        }
        fn peer_addr(&self) -> Option<SocketAddr> {
            Some(self.peer)
        }
        fn peer_key(&self) -> Option<linux_link_core::streaming::PeerKey> {
            Some(linux_link_core::streaming::PeerKey::Tls(
                certs().1.cert_fingerprint(),
            ))
        }
        async fn send_packet(&self, packet: &NetworkPacket) -> Result<()> {
            self.sent.lock().unwrap().push(packet.clone());
            Ok(())
        }
    }

    fn fetch(body: serde_json::Value) -> NetworkPacket {
        NetworkPacket::new(FETCH).with_body(body)
    }

    fn refusal(phone: &Phone) -> Option<(String, String)> {
        let replies = phone.replies();
        let packet = replies.iter().find(|p| p.packet_type == FETCH_REFUSED)?;
        Some((
            packet.body["key"].as_str()?.to_string(),
            packet.body["reason"].as_str()?.to_string(),
        ))
    }

    /// A file that exists on this computer outside the home folder.
    fn outside_home() -> &'static str {
        if cfg!(windows) {
            r"C:\Windows\System32\drivers\etc\hosts"
        } else {
            "/etc/hosts"
        }
    }

    #[tokio::test]
    async fn a_download_outside_home_is_refused_with_its_key() {
        let plugin = SharePlugin::new();
        let phone = Phone::at("192.0.2.7:51000");
        let packet =
            fetch(serde_json::json!({ "path": outside_home(), "port": 40000, "key": KEY }));
        plugin.handle_packet(&packet, &phone).await.unwrap();
        let (key, reason) = refusal(&phone).expect("a refusal");
        assert_eq!(key, KEY);
        assert!(!reason.is_empty());
    }

    #[tokio::test]
    async fn an_offer_made_to_another_phone_is_refused() {
        let plugin = SharePlugin::new();
        let phone = Phone::at("192.0.2.7:51000");
        let file = temp_dir("offer").join("a.txt");
        std::fs::write(&file, "a").unwrap();
        crate::offers::offer_at("offer-for-x", file, "phone-x", std::time::Instant::now());
        let packet =
            fetch(serde_json::json!({ "offer": "offer-for-x", "port": 40000, "key": KEY }));
        plugin.handle_packet(&packet, &phone).await.unwrap();
        let (_, reason) = refusal(&phone).expect("a refusal");
        assert_eq!(reason, "the file was offered to another phone");
    }

    #[tokio::test]
    async fn the_phone_can_decline_an_offer_it_will_not_fetch() {
        let plugin = SharePlugin::new();
        let phone = Phone::at("192.0.2.7:51000");
        crate::offers::offer_at(
            "offer-declined",
            temp_dir("decline").join("a.txt"),
            "phone",
            std::time::Instant::now(),
        );
        let packet = NetworkPacket::new(crate::offers::DECLINE)
            .with_body(serde_json::json!({ "id": "offer-declined", "reason": "no space" }));
        plugin.handle_packet(&packet, &phone).await.unwrap();
        assert_eq!(
            crate::offers::redeem("offer-declined", "phone"),
            Err("the offer was used or is unknown"),
            "declining used it up"
        );
    }

    #[tokio::test]
    async fn a_download_without_a_usable_key_gets_no_answer() {
        let plugin = SharePlugin::new();
        let phone = Phone::at("192.0.2.7:51000");
        let packet =
            fetch(serde_json::json!({ "path": outside_home(), "port": 40000, "key": "nope" }));
        plugin.handle_packet(&packet, &phone).await.unwrap();
        assert!(phone.replies().is_empty());
    }

    #[tokio::test]
    async fn a_download_is_never_dialled_back_to_this_machine() {
        let plugin = SharePlugin::new();
        let phone = Phone::at("127.0.0.1:51000");
        let home = dirs::home_dir().unwrap();
        let packet = fetch(serde_json::json!({
            "path": home.display().to_string(), "port": 40000, "key": KEY,
        }));
        plugin.handle_packet(&packet, &phone).await.unwrap();
        assert_eq!(refusal(&phone).map(|(k, _)| k), Some(KEY.to_string()));
    }
}
