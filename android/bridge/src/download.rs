//! Downloading a file from the desktop (roadmap F7b): a file the phone
//! browsed to, or one the desktop's owner offered it. The phone listens on
//! a fresh port with a fresh key, asks over the control channel, and the
//! desktop dials back, presents the key, and runs the resumable exchange
//! (`core::protocol::transfer`): a download that broke continues from the
//! part this phone kept (T6a).

// The descriptor entry point exists on Android only; on a desktop host the
// rest is reached from the tests alone.
#![cfg_attr(not(unix), allow(dead_code))]

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, TcpDeviceSender};
use pocketdesk_core::protocol::partials::{Limits, Record, Store};
use pocketdesk_core::protocol::transfer::{self, Have, Pace, Partial, TransferError};
use tokio::io::AsyncBufReadExt;
use tokio::sync::{Mutex, broadcast};

use crate::api::{TRANSFER_SENT, TRANSFER_TOTAL, client_identity};
use crate::{CONTROL_PEER, CONTROL_WRITER};
use pocketdesk_core::protocol::clipboard::{CLIPBOARD_IMAGE_FETCH, MAX_CLIPBOARD_IMAGE};

/// Ask the desktop for a file (F7b): `{path | offer, port, key}`.
const FETCH: &str = "kdeconnect.pocketdesk.file_fetch";
/// The desktop will not send it: `{key, reason}`.
const FETCH_REFUSED: &str = "kdeconnect.pocketdesk.file_fetch_refused";
const FOLDER_FETCH: &str = "kdeconnect.pocketdesk.folder_fetch";
pub(crate) const FOLDER_FETCH_REPLY: &str = "kdeconnect.pocketdesk.folder_fetch_reply";
/// The desktop's owner sent this phone a file: `{id, name, size}`.
pub(crate) const FILE_OFFER: &str = "kdeconnect.pocketdesk.file_offer";
/// The phone will not fetch an offer: `{id, reason}`.
const DECLINE: &str = "kdeconnect.pocketdesk.file_offer_declined";
/// The largest file the phone takes, as the desktop.
const MAX_DOWNLOAD_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// A download that receives nothing for this long is abandoned.
const DOWNLOAD_IDLE: Duration = Duration::from_secs(30);

/// The unfinished downloads this phone keeps, for a retry to continue
/// (T6a): beside the certificates, in the app's own storage.
fn partials() -> Option<Store> {
    let certs = crate::CERT_DIR.lock().ok()?.clone()?;
    Some(Store::new(certs.parent()?.join("partials")))
}

/// Which partial a download continues and records into: the computer, the
/// file's source id, its name, and where its bytes are (the URI of the
/// pending MediaStore row, or a file path), which must be the same place.
pub struct Resume {
    pub source: String,
    pub name: String,
    pub location: String,
}

/// Where the bytes of `address`'s file `source` were kept, if a partial of
/// it is: the caller opens that place again to continue it.
pub fn partial_location(address: &str, source: &str) -> Option<String> {
    partials()?.get(address, source).map(|r| r.location)
}

/// Drop unfinished downloads older than `max_age_hours` (or beyond 4, or
/// 4 GiB in all) and return where their bytes are, for the caller to delete.
pub fn sweep_partials(max_age_hours: u32) -> Vec<String> {
    let Some(store) = partials() else {
        return Vec::new();
    };
    let limits = Limits {
        max_age_secs: u64::from(max_age_hours) * 3600,
        per_device: 4,
        total_bytes: 4 * 1024 * 1024 * 1024,
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    store
        .sweep(now, limits, |_| false)
        .into_iter()
        .map(|r| r.location)
        .collect()
}

/// What to download: a file browsed to, one the desktop offered, or the
/// image on the desktop's clipboard (N4), by the id its reply gave.
pub enum FetchTarget {
    Path(String),
    Offer(String),
    ClipboardImage(String),
}

impl FetchTarget {
    /// The request that asks for it, and the most bytes it may be.
    fn request(&self, body: &mut serde_json::Value) -> (&'static str, u64) {
        match self {
            FetchTarget::Path(path) => {
                body["path"] = serde_json::json!(path);
                (FETCH, MAX_DOWNLOAD_BYTES)
            }
            FetchTarget::Offer(id) => {
                body["offer"] = serde_json::json!(id);
                (FETCH, MAX_DOWNLOAD_BYTES)
            }
            FetchTarget::ClipboardImage(id) => {
                body["id"] = serde_json::json!(id);
                (CLIPBOARD_IMAGE_FETCH, MAX_CLIPBOARD_IMAGE)
            }
        }
    }
}

/// The offers the desktop has pushed since the last call, as JSON.
pub fn take_file_offers() -> String {
    crate::take_file_offers_json()
}

/// Download `target` from the desktop into `fd` (opened for reading and
/// writing), which this takes over and closes, and return the file's size.
/// With `resume`, it continues the part an earlier attempt kept there, and
/// keeps what arrives if this attempt breaks too.
#[cfg(unix)]
pub async fn fetch_file_fd(
    address: String,
    port: u16,
    target: FetchTarget,
    fd: i32,
    resume: Option<Resume>,
) -> Result<u64, String> {
    use std::os::fd::FromRawFd;
    if fd < 0 {
        return Err("no file descriptor".to_string());
    }
    // SAFETY: the caller gives up `fd` (ParcelFileDescriptor.detachFd), so
    // this File is its only owner and closes it.
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let mut out = tokio::fs::File::from_std(file);
    fetch_into(&address, port, target, &mut out, resume).await
}

#[cfg(not(unix))]
pub async fn fetch_file_fd(
    _address: String,
    _port: u16,
    _target: FetchTarget,
    _fd: i32,
    _resume: Option<Resume>,
) -> Result<u64, String> {
    Err("file descriptors are only passed on Android".to_string())
}

/// Listen with a fresh key, ask the desktop for `target`, and take the
/// file from the connection that presents the key. A refusal from the
/// desktop ends the wait at once.
async fn fetch_into<P: Partial>(
    address: &str,
    port: u16,
    target: FetchTarget,
    out: &mut P,
    resume: Option<Resume>,
) -> Result<u64, String> {
    // A clipboard image (N4) leaves a file transfer's progress alone.
    let counted = !matches!(target, FetchTarget::ClipboardImage(_));
    if counted {
        TRANSFER_TOTAL.store(0, Ordering::Relaxed);
        TRANSFER_SENT.store(0, Ordering::Relaxed);
    }
    let key = uuid::Uuid::new_v4().simple().to_string();
    // At home the desktop dials a listener; by the internet road it opens a
    // stream on this phone's link, and the request names no port (2d).
    let arrival = crate::link::Arrival::new(address, port, &key).await?;
    let mut body = crate::link::transfer_info(&arrival, &key);
    let (packet_type, limit) = target.request(&mut body);
    let request = NetworkPacket::new(packet_type).with_body(body);
    let lines = send_watching_replies(address, port, &request).await?;
    let refused = refusal_on(lines, key.clone());
    let store = partials();
    let kept = match (&resume, &store) {
        (Some(resume), Some(store)) => Some(Kept {
            store,
            device: address,
            resume,
        }),
        _ => None,
    };
    await_and_receive_limited(arrival, address, &key, refused, out, limit, counted, kept).await
}

/// Download the image on the desktop's clipboard, named `id` by its reply
/// (N4), into a new file at `path`; a failed download leaves no file.
pub async fn fetch_clipboard_image(
    address: String,
    port: u16,
    id: String,
    path: String,
) -> Result<u64, String> {
    let mut out = tokio::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .await
        .map_err(|e| format!("Could not create {path}: {e}"))?;
    let got = fetch_into(
        &address,
        port,
        FetchTarget::ClipboardImage(id),
        &mut out,
        None,
    )
    .await;
    drop(out);
    if got.is_err() {
        let _ = tokio::fs::remove_file(&path).await;
    }
    got
}

/// The read half of a connection opened for one request.
pub(crate) type OwnReader = Box<dyn tokio::io::AsyncRead + Send + Unpin>;

/// Where the desktop's replies to a request arrive: the live control
/// connection's broadcast, or a connection opened for this request alone,
/// kept open while its lines are read.
pub(crate) enum Replies {
    Shared(broadcast::Receiver<String>),
    Own {
        lines: tokio::io::Lines<tokio::io::BufReader<OwnReader>>,
        // Held so the connection stays open while its replies are read.
        _writer: Box<dyn std::any::Any + Send>,
    },
}

impl Replies {
    async fn next_line(&mut self) -> Option<String> {
        match self {
            Replies::Shared(rx) => loop {
                match rx.recv().await {
                    Ok(line) => return Some(line),
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            },
            Replies::Own { lines, .. } => lines.next_line().await.ok().flatten(),
        }
    }
}

/// Send `request` to the desktop at `address:port` and return where its
/// replies arrive. The live control connection carries it when it is to
/// this desktop, so pushes keep their usual target; otherwise a connection
/// of its own does.
pub(crate) async fn send_watching_replies(
    address: &str,
    port: u16,
    request: &NetworkPacket,
) -> Result<Replies, String> {
    let shared = {
        let guard = (*CONTROL_WRITER).lock().await;
        let peer = (*CONTROL_PEER).lock().await;
        let same_desktop = peer
            .as_ref()
            .is_some_and(|(host, live_port)| host == address && *live_port == port);
        if same_desktop {
            guard.as_ref().cloned()
        } else {
            None
        }
    };
    let rx = {
        let guard = crate::INCOMING_PACKETS.lock().await;
        guard.as_ref().map(|tx| tx.subscribe())
    };
    if let (Some(writer), Some(rx)) = (shared, rx) {
        TcpDeviceSender::from_arc(writer, address.to_string())
            .send_packet(request)
            .await
            .map_err(|e| format!("Failed to ask for the file: {e}"))?;
        return Ok(Replies::Shared(rx));
    }
    let stream = crate::api::control_manager(Duration::from_secs(10))?
        .connect(address, port, &client_identity())
        .await
        .map_err(|e| e.to_string())?;
    let (reader, writer) = tokio::io::split(stream);
    let writer = Arc::new(Mutex::new(writer));
    TcpDeviceSender::from_arc(writer.clone(), address.to_string())
        .send_packet(request)
        .await
        .map_err(|e| format!("Failed to ask for the file: {e}"))?;
    Ok(Replies::Own {
        lines: tokio::io::BufReader::new(Box::new(reader) as OwnReader).lines(),
        _writer: Box::new(writer),
    })
}

/// Tell the desktop the phone will not fetch offer `id`, and why (F7b), so
/// its sent list does not wait on it.
/// Ask the desktop for the whole folder at `path` from the remote browser
/// (`folder_fetch`). Its files then arrive as offers on this session's
/// control connection, which must be the one that asks: a one-off
/// connection would close and lose them.
pub async fn fetch_folder(address: String, port: u16, path: String) -> Result<(), String> {
    // An answer left from an earlier request for this folder is not this one's.
    let _ = crate::take_folder_reply(&path);
    crate::api::send_on_live(&address, port, &folder_fetch_packet(&path)).await?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        // Armed before looking, so an answer between the two is not missed.
        let arrived = crate::FOLDER_REPLY.notified();
        tokio::pin!(arrived);
        arrived.as_mut().enable();
        if let Some(answer) = crate::take_folder_reply(&path) {
            return folder_answer(&answer);
        }
        if tokio::time::timeout_at(deadline, arrived).await.is_err() {
            return Err(crate::api::NoReply.to_string());
        }
    }
}

fn folder_fetch_packet(path: &str) -> NetworkPacket {
    NetworkPacket::new(FOLDER_FETCH).with_body(serde_json::json!({ "path": path }))
}

/// The desktop's answer to a folder request: queued, or why not.
fn folder_answer(body: &serde_json::Value) -> Result<(), String> {
    if body.get("queued").and_then(|v| v.as_bool()) == Some(true) {
        return Ok(());
    }
    let reason = body
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("no reason given");
    Err(format!("The computer refused the folder: {reason}"))
}

pub async fn decline_offer(
    address: String,
    port: u16,
    id: String,
    reason: String,
) -> Result<(), String> {
    crate::api::push_packet(&address, port, &decline_packet(&id, &reason)).await
}

fn decline_packet(id: &str, reason: &str) -> NetworkPacket {
    NetworkPacket::new(DECLINE).with_body(serde_json::json!({ "id": id, "reason": reason }))
}

/// Resolves with the desktop's reason if it refuses the download keyed
/// `key`; never resolves otherwise. Offers seen on a connection of our own
/// are latched as the control reader would.
pub(crate) async fn refusal_on(mut replies: Replies, key: String) -> String {
    while let Some(line) = replies.next_line().await {
        if let Some(reason) = refusal_for(&line, &key) {
            return reason;
        }
        if let (Replies::Own { .. }, Ok(packet)) = (&replies, NetworkPacket::from_wire(&line))
            && packet.packet_type == FILE_OFFER
        {
            crate::queue_file_offer(&packet.body);
        }
    }
    std::future::pending().await
}

/// The reason in `line`, if it is the refusal of the download keyed `key`.
fn refusal_for(line: &str, key: &str) -> Option<String> {
    let packet = NetworkPacket::from_wire(line.trim()).ok()?;
    if packet.packet_type != FETCH_REFUSED || packet.body.get("key")?.as_str()? != key {
        return None;
    }
    Some(
        packet
            .body
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("the computer refused")
            .to_string(),
    )
}

/// A partial a download continues and records into.
struct Kept<'a> {
    store: &'a Store,
    /// The computer it comes from.
    device: &'a str,
    resume: &'a Resume,
}

/// Wait up to 30 s for the desktop to connect inside TLS with `key`
/// (dropping anything else that connects), then receive the file into
/// `out`, unless `refused` resolves first.
#[cfg(test)]
async fn await_and_receive<P, F>(
    listener: tokio::net::TcpListener,
    key: &str,
    tls: &tokio_rustls::TlsAcceptor,
    refused: F,
    out: &mut P,
) -> Result<u64, String>
where
    P: Partial,
    F: std::future::Future<Output = String>,
{
    await_and_receive_limited(
        crate::link::Arrival::Listen(listener, Some(tls.clone())),
        "",
        key,
        refused,
        out,
        MAX_DOWNLOAD_BYTES,
        true,
        None,
    )
    .await
}

/// [`await_and_receive`], refusing more than `limit` bytes, counting into
/// the transfer progress only when `counted`, and continuing `kept`.
#[allow(clippy::too_many_arguments)]
async fn await_and_receive_limited<P, F>(
    arrival: crate::link::Arrival,
    address: &str,
    key: &str,
    refused: F,
    out: &mut P,
    limit: u64,
    counted: bool,
    kept: Option<Kept<'_>>,
) -> Result<u64, String>
where
    P: Partial,
    F: std::future::Future<Output = String>,
{
    let by_link = arrival.port().is_none();
    // Only the desktop this phone pinned for `address` may send (phase C);
    // by the internet road, the stream on the link.
    let accept = tokio::time::timeout(Duration::from_secs(30), arrival.take(address, key));
    let mut stream = tokio::select! {
        accepted = accept => accepted.map_err(|_| "The computer did not send the file".to_string())??,
        reason = refused => return Err(crate::api::refusal_text("download", &reason, by_link)),
    };
    receive_into(&mut stream, out, limit, counted, kept).await
}

/// Receive one file from `stream` into `out` with the resumable exchange,
/// continuing what `kept` recorded, then tell the desktop it was saved.
/// Returns the file's size.
async fn receive_into<S, P>(
    stream: &mut S,
    out: &mut P,
    limit: u64,
    counted: bool,
    kept: Option<Kept<'_>>,
) -> Result<u64, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    P: Partial,
{
    let record = kept.as_ref().and_then(|k| {
        k.store
            .get(k.device, &k.resume.source)
            .filter(|r| r.location == k.resume.location)
    });
    let have = record.as_ref().map(|r| Have {
        size: r.size,
        committed: r.committed,
    });
    let created_unix = record.as_ref().map_or_else(
        || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs())
        },
        |r| r.created_unix,
    );
    if counted {
        TRANSFER_TOTAL.store(0, Ordering::Relaxed);
        TRANSFER_SENT.store(0, Ordering::Relaxed);
    }
    let mut total = 0;
    let pace = Pace {
        idle: DOWNLOAD_IDLE,
        ..Pace::DEFAULT
    };
    let outcome = transfer::receive(stream, out, have, None, limit, pace, |size, committed| {
        total = size;
        if counted {
            TRANSFER_TOTAL.store(size, Ordering::Relaxed);
            TRANSFER_SENT.store(committed, Ordering::Relaxed);
        }
        if let Some(k) = &kept {
            let record = Record {
                device: k.device.to_string(),
                source: k.resume.source.clone(),
                name: k.resume.name.clone(),
                size,
                committed,
                created_unix,
                location: k.resume.location.clone(),
            };
            if let Err(e) = k.store.put(&record) {
                tracing::warn!("Could not record the unfinished download: {e}");
            }
        }
    })
    .await;
    match outcome {
        Ok(offset) => {
            if let Some(k) = &kept {
                k.store.remove(k.device, &k.resume.source);
            }
            if offset > 0 {
                tracing::info!("Download continued from {offset} of {total} bytes");
            }
            transfer::finish(stream, true, pace).await;
            Ok(total)
        }
        Err(e) => {
            // Only a broken connection is worth continuing.
            if let Some(k) = &kept
                && !matches!(e, TransferError::Io(_))
            {
                k.store.remove(k.device, &k.resume.source);
            }
            Err(match e {
                TransferError::Size(0) => "the computer sent an empty file".to_string(),
                TransferError::Size(size) => format!("a size of {size} bytes was not accepted"),
                TransferError::Io(e) => format!("The connection failed: {e}"),
                other => format!("The download failed: {other}"),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FETCH_REFUSED, FOLDER_FETCH, FOLDER_FETCH_REPLY, Kept, Resume, await_and_receive,
        folder_answer, folder_fetch_packet, receive_into, refusal_for,
    };
    use crate::api::transfer_progress;
    use pocketdesk_core::protocol::kdeconnect::NetworkPacket;
    use pocketdesk_core::protocol::partials::Store;
    use pocketdesk_core::protocol::transfer::{self, Memory, Pace, Partial};
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    const KEY: &str = "0123456789abcdef0123456789abcdef";

    use pocketdesk_core::streaming::transport::CertManager;

    /// This phone, the desktop it pinned, and another computer.
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

    /// The phone's side: only the pinned desktop.
    fn phone_tls() -> tokio_rustls::TlsAcceptor {
        let (phone, desktop, _) = certs();
        tokio_rustls::TlsAcceptor::from(
            phone
                .transfer_server_config(desktop.cert_fingerprint())
                .unwrap(),
        )
    }

    /// Dial `to` inside TLS as `from` (a desktop), expecting this phone.
    async fn dial_as(
        from: &CertManager,
        to: std::net::SocketAddr,
    ) -> std::io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
        let (phone, _, _) = certs();
        let tcp = TcpStream::connect(to).await?;
        tokio_rustls::TlsConnector::from(from.transfer_client_config(phone.cert_fingerprint()))
            .connect(
                tokio_rustls::rustls::pki_types::ServerName::IpAddress(to.ip().into()),
                tcp,
            )
            .await
    }

    /// The desktop's side of a download: dial, present `key`, and send the
    /// first `data.len()` bytes of a `size`-byte file with the resumable
    /// exchange. Some(1) when the phone said it saved it, Some(0) when it
    /// said it did not, None when it said nothing.
    async fn desktop_sending(
        to: std::net::SocketAddr,
        key: &str,
        size: u64,
        data: Vec<u8>,
    ) -> Option<u8> {
        let mut stream = dial_as(&certs().1, to).await.unwrap();
        stream
            .write_all(format!("{key}\n").as_bytes())
            .await
            .unwrap();
        let mut reader: &[u8] = &data;
        match transfer::send(&mut reader, size, &mut stream, Pace::DEFAULT, |_| {}).await {
            Ok(_) => Some(1),
            Err(transfer::TransferError::NotSaved) => Some(0),
            Err(_) => None,
        }
    }

    async fn listener() -> (TcpListener, std::net::SocketAddr) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        (listener, addr)
    }

    #[tokio::test]
    async fn a_download_lands_in_the_writer_and_the_desktop_hears_it_saved() {
        let _counters = crate::api::TRANSFER_TEST_LOCK.lock().await;
        let (listener, addr) = listener().await;
        let data: Vec<u8> = (0..200_000u32).map(|i| (i * 3) as u8).collect();
        let desktop = tokio::spawn(desktop_sending(addr, KEY, data.len() as u64, data.clone()));
        let mut out = Memory::holding(&[]);
        let got = await_and_receive(
            listener,
            KEY,
            &phone_tls(),
            std::future::pending(),
            &mut out,
        )
        .await;
        assert_eq!(got, Ok(data.len() as u64));
        assert_eq!(out.bytes(), &data[..]);
        assert_eq!(desktop.await.unwrap(), Some(1));
        let (done, total) = transfer_progress();
        assert!(done <= total, "progress stays within its total");
    }

    #[tokio::test]
    async fn a_connection_without_the_key_is_dropped_and_the_right_one_served() {
        let _counters = crate::api::TRANSFER_TEST_LOCK.lock().await;
        let (listener, addr) = listener().await;
        let wrong = tokio::spawn(async move {
            let mut stream = dial_as(&certs().1, addr).await.unwrap();
            stream
                .write_all(b"ffffffffffffffffffffffffffffffff\n")
                .await
                .unwrap();
            stream.flush().await.unwrap();
            let mut rest = Vec::new();
            let _ = stream.read_to_end(&mut rest).await;
            rest
        });
        let receiving = tokio::spawn(async move {
            let mut out = Memory::holding(&[]);
            let got = await_and_receive(
                listener,
                KEY,
                &phone_tls(),
                std::future::pending(),
                &mut out,
            )
            .await;
            (got, out.into_bytes())
        });
        assert!(
            wrong.await.unwrap().is_empty(),
            "the wrong key gets nothing"
        );
        let status = desktop_sending(addr, KEY, 3, b"abc".to_vec()).await;
        assert_eq!(status, Some(1));
        assert_eq!(receiving.await.unwrap(), (Ok(3), b"abc".to_vec()));
    }

    #[tokio::test]
    async fn a_computer_the_phone_did_not_pin_gets_nothing() {
        let _counters = crate::api::TRANSFER_TEST_LOCK.lock().await;
        let (listener, addr) = listener().await;
        let receiving = tokio::spawn(async move {
            let mut out = Memory::holding(&[]);
            let got = await_and_receive(
                listener,
                KEY,
                &phone_tls(),
                std::future::pending(),
                &mut out,
            )
            .await;
            (got, out.into_bytes())
        });
        // Another computer, even one that knows the key, is turned away in
        // the handshake.
        let other = async {
            let mut stream = dial_as(&certs().2, addr).await?;
            stream.write_all(format!("{KEY}\n").as_bytes()).await?;
            stream.flush().await?;
            let mut rest = Vec::new();
            stream.read_to_end(&mut rest).await?;
            Ok::<_, std::io::Error>(rest)
        }
        .await;
        assert!(other.is_err() || other.unwrap().is_empty());
        let status = desktop_sending(addr, KEY, 3, b"xyz".to_vec()).await;
        assert_eq!(status, Some(1));
        assert_eq!(receiving.await.unwrap(), (Ok(3), b"xyz".to_vec()));
    }

    #[tokio::test]
    async fn silent_connections_do_not_hold_up_the_desktop() {
        let _counters = crate::api::TRANSFER_TEST_LOCK.lock().await;
        let (listener, addr) = listener().await;
        // Connections that never start a handshake, ahead of the desktop.
        let mut silent = Vec::new();
        for _ in 0..4 {
            silent.push(TcpStream::connect(addr).await.unwrap());
        }
        let receiving = tokio::spawn(async move {
            let mut out = Memory::holding(&[]);
            let got = await_and_receive(
                listener,
                KEY,
                &phone_tls(),
                std::future::pending(),
                &mut out,
            )
            .await;
            (got, out.into_bytes())
        });
        let sent = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            desktop_sending(addr, KEY, 3, b"abc".to_vec()),
        )
        .await;
        assert_eq!(sent, Ok(Some(1)), "served while the others sit silent");
        assert_eq!(receiving.await.unwrap(), (Ok(3), b"abc".to_vec()));
        drop(silent);
    }

    #[tokio::test]
    async fn a_short_download_fails_and_says_so() {
        let _counters = crate::api::TRANSFER_TEST_LOCK.lock().await;
        let (listener, addr) = listener().await;
        let desktop = tokio::spawn(desktop_sending(addr, KEY, 10, b"abcd".to_vec()));
        let mut out = Memory::holding(&[]);
        assert!(
            await_and_receive(
                listener,
                KEY,
                &phone_tls(),
                std::future::pending(),
                &mut out
            )
            .await
            .is_err()
        );
        assert_ne!(desktop.await.unwrap(), Some(1), "not saved");
    }

    #[tokio::test]
    async fn an_oversized_download_is_refused_before_any_byte() {
        let _counters = crate::api::TRANSFER_TEST_LOCK.lock().await;
        let (listener, addr) = listener().await;
        let desktop = tokio::spawn(desktop_sending(addr, KEY, 9 << 30, Vec::new()));
        let mut out = Memory::holding(&[]);
        assert!(
            await_and_receive(
                listener,
                KEY,
                &phone_tls(),
                std::future::pending(),
                &mut out
            )
            .await
            .is_err()
        );
        assert_ne!(desktop.await.unwrap(), Some(1), "not saved");
        assert!(out.bytes().is_empty());
    }

    /// Storage that fails, as a full phone does.
    struct Full;

    impl tokio::io::AsyncRead for Full {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl tokio::io::AsyncWrite for Full {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            Poll::Ready(Err(std::io::Error::other("no space left")))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl tokio::io::AsyncSeek for Full {
        fn start_seek(self: Pin<&mut Self>, _: std::io::SeekFrom) -> std::io::Result<()> {
            Ok(())
        }
        fn poll_complete(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<u64>> {
            Poll::Ready(Ok(0))
        }
    }

    impl Partial for Full {
        async fn truncate(&mut self, _: u64) -> std::io::Result<()> {
            Ok(())
        }
        async fn sync(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_failed_write_answers_zero() {
        let _counters = crate::api::TRANSFER_TEST_LOCK.lock().await;
        let (listener, addr) = listener().await;
        let desktop = tokio::spawn(desktop_sending(addr, KEY, 3, b"abc".to_vec()));
        let got = await_and_receive(
            listener,
            KEY,
            &phone_tls(),
            std::future::pending(),
            &mut Full,
        )
        .await;
        assert!(got.is_err());
        assert_ne!(desktop.await.unwrap(), Some(1), "not saved");
    }

    /// A download that broke continues from what this phone kept, and only
    /// in the place it was kept.
    #[tokio::test]
    async fn a_broken_download_is_continued_by_the_retry() {
        let _counters = crate::api::TRANSFER_TEST_LOCK.lock().await;
        let dir = std::env::temp_dir().join(format!("pd-dl-partials-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::new(&dir);
        let resume = Resume {
            source: "00112233445566778899aabbccddeeff".into(),
            name: "a.bin".into(),
            location: "content://media/1".into(),
        };
        let data: Vec<u8> = (0..300_000u32).map(|i| (i * 11) as u8).collect();
        let mut out = Memory::holding(&[]);

        // The first attempt stops part way.
        let (mut desktop, mut phone) = tokio::io::duplex(1 << 20);
        let cut = data[..140_000].to_vec();
        let sender = tokio::spawn(async move {
            let mut reader: &[u8] = &cut;
            let sent =
                transfer::send(&mut reader, 300_000, &mut desktop, Pace::DEFAULT, |_| {}).await;
            drop(desktop);
            sent
        });
        let kept = Kept {
            store: &store,
            device: "192.0.2.4",
            resume: &resume,
        };
        assert!(
            receive_into(&mut phone, &mut out, 1 << 30, false, Some(kept))
                .await
                .is_err()
        );
        assert!(sender.await.unwrap().is_err());
        assert_eq!(
            store.get("192.0.2.4", &resume.source).unwrap().committed,
            140_000
        );

        // The retry sends only the rest.
        let (mut desktop, mut phone) = tokio::io::duplex(1 << 20);
        let all = data.clone();
        let sender = tokio::spawn(async move {
            let mut reader: &[u8] = &all;
            let sent =
                transfer::send(&mut reader, 300_000, &mut desktop, Pace::DEFAULT, |_| {}).await;
            drop(desktop);
            sent
        });
        let kept = Kept {
            store: &store,
            device: "192.0.2.4",
            resume: &resume,
        };
        assert_eq!(
            receive_into(&mut phone, &mut out, 1 << 30, false, Some(kept)).await,
            Ok(300_000)
        );
        assert_eq!(sender.await.unwrap().unwrap(), 140_000, "continued");
        assert_eq!(out.bytes(), &data[..]);
        assert!(
            store.get("192.0.2.4", &resume.source).is_none(),
            "the record is gone"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_refusal_ends_the_wait_with_its_reason() {
        let (listener, _) = listener().await;
        let refused = async { "the offer expired".to_string() };
        let mut out = Memory::holding(&[]);
        let got = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            await_and_receive(listener, KEY, &phone_tls(), refused, &mut out),
        )
        .await
        .expect("no wait for the listener's timeout");
        assert_eq!(
            got,
            Err("The computer refused the download: the offer expired".to_string())
        );
    }

    #[test]
    fn only_the_refusal_for_this_download_counts() {
        let line = |key: &str| {
            NetworkPacket::new(FETCH_REFUSED)
                .with_body(serde_json::json!({ "key": key, "reason": "no" }))
                .to_wire()
                .map(|bytes| String::from_utf8(bytes).unwrap())
                .unwrap()
        };
        assert_eq!(refusal_for(&line(KEY), KEY), Some("no".to_string()));
        assert_eq!(refusal_for(&line("ffff"), KEY), None);
        assert_eq!(refusal_for("not a packet", KEY), None);
    }

    #[tokio::test]
    async fn an_offer_on_a_transfers_own_connection_is_kept() {
        let _offers = crate::OFFERS_TEST_LOCK.lock().await;
        let (listener, addr) = listener().await;
        let desktop = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let offer = NetworkPacket::new(super::FILE_OFFER)
                .with_body(serde_json::json!({ "id": "own-conn", "name": "a.pdf", "size": 3 }))
                .to_wire()
                .unwrap();
            socket.write_all(&offer).await.unwrap();
            socket
        });
        let (reader, writer) = TcpStream::connect(addr).await.unwrap().into_split();
        let replies = super::Replies::Own {
            lines: tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(
                Box::new(reader) as super::OwnReader
            )),
            _writer: Box::new(writer),
        };
        // Waiting for a refusal also keeps any offer that arrives meanwhile.
        let reading = tokio::spawn(super::refusal_on(replies, KEY.to_string()));
        let _socket = desktop.await.unwrap();
        let mut kept = String::new();
        for _ in 0..50 {
            kept = crate::take_file_offers_json();
            if kept.contains("own-conn") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        reading.abort();
        assert!(kept.contains("own-conn"), "{kept}");
    }

    #[test]
    fn a_folder_request_is_queued_or_refused_with_the_desktops_reason() {
        let packet = folder_fetch_packet("C:\\Users\\me\\Docs");
        assert_eq!(packet.packet_type, FOLDER_FETCH);
        assert_eq!(packet.body["path"], "C:\\Users\\me\\Docs");
        assert_eq!(
            folder_answer(&serde_json::json!({ "path": "x", "queued": true, "reason": null })),
            Ok(())
        );
        assert_eq!(
            folder_answer(&serde_json::json!({ "queued": false, "reason": "busy: two folders" })),
            Err("The computer refused the folder: busy: two folders".to_string())
        );
        assert!(
            folder_answer(&serde_json::json!({})).is_err(),
            "no answer is no folder"
        );
    }

    #[test]
    fn each_folder_request_gets_its_own_answer() {
        let _offers = crate::OFFERS_TEST_LOCK.blocking_lock();
        crate::latch_folder_reply(&serde_json::json!({ "path": "A", "queued": true }));
        crate::latch_folder_reply(
            &serde_json::json!({ "path": "B", "queued": false, "reason": "busy" }),
        );
        assert_eq!(crate::take_folder_reply("B").unwrap()["queued"], false);
        assert_eq!(crate::take_folder_reply("A").unwrap()["queued"], true);
        assert!(crate::take_folder_reply("A").is_none(), "taken once");
    }

    #[test]
    fn a_burst_of_offers_never_reaches_the_pollers() {
        // A folder is offered file by file, thousands at once: kept out of
        // the shared broadcast, so no waiting answer is pushed out by them.
        assert!(crate::kept_from_pollers(super::FILE_OFFER));
        assert!(crate::kept_from_pollers(FOLDER_FETCH_REPLY));
        assert!(!crate::kept_from_pollers("kdeconnect.battery"));
    }

    #[test]
    fn a_decline_names_the_offer_and_why() {
        let packet = super::decline_packet("o-1", "Not enough space for a.pdf");
        assert_eq!(
            packet.packet_type,
            "kdeconnect.pocketdesk.file_offer_declined"
        );
        assert_eq!(packet.body["id"], "o-1");
        assert_eq!(packet.body["reason"], "Not enough space for a.pdf");
    }

    /// K1: an offer's date goes on to the app, and none without one.
    #[test]
    fn an_offer_keeps_its_date() {
        let _offers = crate::OFFERS_TEST_LOCK.blocking_lock();
        let _ = crate::take_file_offers_json();
        crate::queue_file_offer(&serde_json::json!({
            "id": "dated", "name": "a.jpg", "size": 3, "lastModified": 1_780_000_000_000u64,
        }));
        crate::queue_file_offer(
            &serde_json::json!({ "id": "undated", "name": "b.jpg", "size": 3 }),
        );
        let taken: Vec<serde_json::Value> =
            serde_json::from_str(&crate::take_file_offers_json()).unwrap();
        assert_eq!(taken[0]["lastModified"], 1_780_000_000_000u64);
        assert!(taken[1].get("lastModified").is_none());
    }

    #[test]
    fn a_folder_offer_keeps_its_place_only_when_every_name_is_safe() {
        let _offers = crate::OFFERS_TEST_LOCK.blocking_lock();
        let _ = crate::take_file_offers_json();
        let id = "00112233445566778899aabbccddeeff";
        crate::queue_file_offer(&serde_json::json!({
            "id": "safe", "name": "a.jpg", "size": 3,
            "folderId": id, "folder": "Trip", "path": ["day 2", "a.jpg"],
        }));
        crate::queue_file_offer(&serde_json::json!({
            "id": "climbs", "name": "x", "size": 3,
            "folderId": id, "folder": "Trip", "path": ["..", "x"],
        }));
        let taken: Vec<serde_json::Value> =
            serde_json::from_str(&crate::take_file_offers_json()).unwrap();
        assert_eq!(taken[0]["path"], serde_json::json!(["day 2", "a.jpg"]));
        assert_eq!(taken[0]["folder"], "Trip");
        assert!(
            taken[1].get("path").is_none(),
            "an unsafe place is never kept"
        );
        assert!(
            taken[1]["folderRefused"].is_string(),
            "and the offer is to be declined"
        );
    }

    #[test]
    fn offers_are_kept_once_each_and_bounded() {
        let _offers = crate::OFFERS_TEST_LOCK.blocking_lock();
        let _ = crate::take_file_offers_json();
        // A whole folder send (T6b: up to 10 000 files) arrives before the
        // app drains, plus a few single files: none may be lost.
        let total = 10_000 + 20;
        for n in 0..total {
            let body = serde_json::json!({ "id": format!("o{n}"), "name": "a.pdf", "size": 3 });
            crate::queue_file_offer(&body);
            crate::queue_file_offer(&body);
        }
        crate::queue_file_offer(&serde_json::json!({ "name": "no id" }));
        let taken: Vec<serde_json::Value> =
            serde_json::from_str(&crate::take_file_offers_json()).unwrap();
        assert_eq!(taken.len(), total, "each once, none dropped");
        assert_eq!(taken[0]["id"], "o0");
        assert_eq!(crate::take_file_offers_json(), "[]");

        // Past the bound, the oldest go first.
        for n in 0..crate::MAX_QUEUED_OFFERS + 3 {
            crate::queue_file_offer(
                &serde_json::json!({ "id": format!("p{n}"), "name": "a", "size": 1 }),
            );
        }
        let taken: Vec<serde_json::Value> =
            serde_json::from_str(&crate::take_file_offers_json()).unwrap();
        assert_eq!(taken.len(), crate::MAX_QUEUED_OFFERS);
        assert_eq!(taken[0]["id"], "p3");
        // A drained id may be offered again (a retry's new offer).
        crate::queue_file_offer(&serde_json::json!({ "id": "p3", "name": "a", "size": 1 }));
        assert_ne!(crate::take_file_offers_json(), "[]");
    }
}
