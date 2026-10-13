//! Downloading a file from the desktop (roadmap F7b): a file the phone
//! browsed to, or one the desktop's owner offered it. The phone listens on
//! a fresh port with a fresh key, asks over the control channel, and the
//! desktop dials back, presents the key, and sends the size and the bytes;
//! the phone answers with one status byte once the file is saved.

// The descriptor entry point exists on Android only; on a desktop host the
// rest is reached from the tests alone.
#![cfg_attr(not(unix), allow(dead_code))]

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use linux_link_core::protocol::connection::ConnectionManager;
use linux_link_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, TcpDeviceSender};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, broadcast};

use crate::api::{TRANSFER_SENT, TRANSFER_TOTAL, client_identity, presents_key};
use crate::{CONTROL_PEER, CONTROL_WRITER};

/// Ask the desktop for a file (F7b): `{path | offer, port, key}`.
const FETCH: &str = "kdeconnect.linuxlink.file_fetch";
/// The desktop will not send it: `{key, reason}`.
const FETCH_REFUSED: &str = "kdeconnect.linuxlink.file_fetch_refused";
/// The desktop's owner sent this phone a file: `{id, name, size}`.
pub(crate) const FILE_OFFER: &str = "kdeconnect.linuxlink.file_offer";
/// The largest file the phone takes, as the desktop.
const MAX_DOWNLOAD_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// A download that receives nothing for this long is abandoned.
const DOWNLOAD_IDLE: Duration = Duration::from_secs(30);

/// What to download: a file browsed to, or one the desktop offered.
pub enum FetchTarget {
    Path(String),
    Offer(String),
}

/// The offers the desktop has pushed since the last call, as JSON.
pub fn take_file_offers() -> String {
    crate::take_file_offers_json()
}

/// Download `target` from the desktop into `fd`, which this takes over and
/// closes, and return the bytes saved.
#[cfg(unix)]
pub async fn fetch_file_fd(
    address: String,
    port: u16,
    target: FetchTarget,
    fd: i32,
) -> Result<u64, String> {
    use std::os::fd::FromRawFd;
    if fd < 0 {
        return Err("no file descriptor".to_string());
    }
    // SAFETY: the caller gives up `fd` (ParcelFileDescriptor.detachFd), so
    // this File is its only owner and closes it.
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let mut out = tokio::fs::File::from_std(file);
    fetch_into(&address, port, target, &mut out).await
}

#[cfg(not(unix))]
pub async fn fetch_file_fd(
    _address: String,
    _port: u16,
    _target: FetchTarget,
    _fd: i32,
) -> Result<u64, String> {
    Err("file descriptors are only passed on Android".to_string())
}

/// Listen with a fresh key, ask the desktop for `target`, and take the
/// file from the connection that presents the key. A refusal from the
/// desktop ends the wait at once.
async fn fetch_into<W: tokio::io::AsyncWrite + Unpin>(
    address: &str,
    port: u16,
    target: FetchTarget,
    out: &mut W,
) -> Result<u64, String> {
    {
        TRANSFER_TOTAL.store(0, Ordering::Relaxed);
        TRANSFER_SENT.store(0, Ordering::Relaxed);
    }
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0")
        .await
        .map_err(|e| format!("Failed to bind port: {e}"))?;
    let transfer_port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let key = uuid::Uuid::new_v4().simple().to_string();
    let mut body = serde_json::json!({ "port": transfer_port, "key": key });
    match &target {
        FetchTarget::Path(path) => body["path"] = serde_json::json!(path),
        FetchTarget::Offer(id) => body["offer"] = serde_json::json!(id),
    }
    let request = NetworkPacket::new(FETCH).with_body(body);
    let lines = send_watching_replies(address, port, &request).await?;
    let refused = refusal_on(lines, key.clone());
    await_and_receive(listener, &key, refused, out).await
}

/// Where the desktop's replies to a request arrive: the live control
/// connection's broadcast, or a connection opened for this request alone,
/// kept open while its lines are read.
enum Replies {
    Shared(broadcast::Receiver<String>),
    Own {
        lines: tokio::io::Lines<tokio::io::BufReader<tokio::net::tcp::OwnedReadHalf>>,
        // Held so the connection stays open until the download ends.
        _writer: Arc<Mutex<tokio::net::tcp::OwnedWriteHalf>>,
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
async fn send_watching_replies(
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
    let stream = ConnectionManager::new(Duration::from_secs(10))
        .connect(address, port, &client_identity())
        .await
        .map_err(|e| e.to_string())?;
    let (reader, writer) = stream.into_split();
    let writer = Arc::new(Mutex::new(writer));
    TcpDeviceSender::from_arc(writer.clone(), address.to_string())
        .send_packet(request)
        .await
        .map_err(|e| format!("Failed to ask for the file: {e}"))?;
    Ok(Replies::Own {
        lines: tokio::io::BufReader::new(reader).lines(),
        _writer: writer,
    })
}

/// Resolves with the desktop's reason if it refuses the download keyed
/// `key`; never resolves otherwise. Offers seen on a connection of our own
/// are latched as the control reader would.
async fn refusal_on(mut replies: Replies, key: String) -> String {
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

/// Wait up to 30 s for the desktop to connect with `key` (dropping anything
/// else that connects), then receive the file into `out`, unless `refused`
/// resolves first.
async fn await_and_receive<W, F>(
    listener: tokio::net::TcpListener,
    key: &str,
    refused: F,
    out: &mut W,
) -> Result<u64, String>
where
    W: tokio::io::AsyncWrite + Unpin,
    F: std::future::Future<Output = String>,
{
    let accept = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let (mut stream, peer) = listener
                .accept()
                .await
                .map_err(|e| format!("Failed to accept connection: {e}"))?;
            if presents_key(&mut stream, key).await {
                return Ok::<_, String>(stream);
            }
            tracing::warn!("Download: dropped a connection from {peer} without the key");
        }
    });
    let mut stream = tokio::select! {
        accepted = accept => accepted.map_err(|_| "The computer did not send the file".to_string())??,
        reason = refused => return Err(reason),
    };
    receive_into(&mut stream, out).await
}

/// Read the size and exactly that many bytes from `stream` into `out`,
/// counting progress, then tell the desktop whether it was saved.
async fn receive_into<S, W>(stream: &mut S, out: &mut W) -> Result<u64, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let received = async {
        let size = tokio::time::timeout(DOWNLOAD_IDLE, stream.read_u64())
            .await
            .map_err(|_| "The computer stopped sending".to_string())?
            .map_err(|e| format!("The computer stopped sending: {e}"))?;
        if size > MAX_DOWNLOAD_BYTES {
            return Err("The file is larger than 8 GB".to_string());
        }
        TRANSFER_TOTAL.store(size, Ordering::Relaxed);
        TRANSFER_SENT.store(0, Ordering::Relaxed);
        let mut buffer = vec![0u8; 64 * 1024];
        let mut done: u64 = 0;
        while done < size {
            let want = buffer
                .len()
                .min(usize::try_from(size - done).unwrap_or(usize::MAX));
            let n = tokio::time::timeout(DOWNLOAD_IDLE, stream.read(&mut buffer[..want]))
                .await
                .map_err(|_| "The computer stopped sending".to_string())?
                .map_err(|e| format!("The connection failed: {e}"))?;
            if n == 0 {
                return Err(format!("The file ended after {done} of {size} bytes"));
            }
            out.write_all(&buffer[..n])
                .await
                .map_err(|e| format!("Could not save the file: {e}"))?;
            done += n as u64;
            TRANSFER_SENT.store(done, Ordering::Relaxed);
        }
        out.flush()
            .await
            .map_err(|e| format!("Could not save the file: {e}"))?;
        Ok(size)
    }
    .await;
    let status = if received.is_ok() { 1 } else { 0 };
    let _ = tokio::time::timeout(Duration::from_secs(5), stream.write_all(&[status])).await;
    received
}

#[cfg(test)]
mod tests {
    use super::{FETCH_REFUSED, await_and_receive, refusal_for};
    use crate::api::transfer_progress;
    use linux_link_core::protocol::kdeconnect::NetworkPacket;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    const KEY: &str = "0123456789abcdef0123456789abcdef";

    /// The desktop's side of a download: dial, present `key`, send the
    /// size and `data` (all of it unless `short`), and read the status.
    async fn desktop_sending(
        to: std::net::SocketAddr,
        key: &str,
        size: u64,
        data: Vec<u8>,
    ) -> Option<u8> {
        let mut stream = TcpStream::connect(to).await.unwrap();
        stream
            .write_all(format!("{key}\n").as_bytes())
            .await
            .unwrap();
        stream.write_u64(size).await.unwrap();
        stream.write_all(&data).await.unwrap();
        stream.shutdown().await.unwrap();
        let mut status = [0u8; 1];
        stream.read_exact(&mut status).await.ok().map(|_| status[0])
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
        let mut out = Vec::new();
        let got = await_and_receive(listener, KEY, std::future::pending(), &mut out).await;
        assert_eq!(got, Ok(data.len() as u64));
        assert_eq!(out, data);
        assert_eq!(desktop.await.unwrap(), Some(1));
        let (done, total) = transfer_progress();
        assert!(done <= total, "progress stays within its total");
    }

    #[tokio::test]
    async fn a_connection_without_the_key_is_dropped_and_the_right_one_served() {
        let _counters = crate::api::TRANSFER_TEST_LOCK.lock().await;
        let (listener, addr) = listener().await;
        let wrong = tokio::spawn(async move {
            let mut stream = TcpStream::connect(addr).await.unwrap();
            stream
                .write_all(b"ffffffffffffffffffffffffffffffff\n")
                .await
                .unwrap();
            let mut rest = Vec::new();
            let _ = stream.read_to_end(&mut rest).await;
            rest
        });
        let receiving = tokio::spawn(async move {
            let mut out = Vec::new();
            let got = await_and_receive(listener, KEY, std::future::pending(), &mut out).await;
            (got, out)
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
    async fn a_short_download_fails_and_says_so() {
        let _counters = crate::api::TRANSFER_TEST_LOCK.lock().await;
        let (listener, addr) = listener().await;
        let desktop = tokio::spawn(desktop_sending(addr, KEY, 10, b"abcd".to_vec()));
        let mut out = Vec::new();
        assert!(
            await_and_receive(listener, KEY, std::future::pending(), &mut out)
                .await
                .is_err()
        );
        assert_eq!(desktop.await.unwrap(), Some(0));
    }

    #[tokio::test]
    async fn an_oversized_download_is_refused_before_any_byte() {
        let _counters = crate::api::TRANSFER_TEST_LOCK.lock().await;
        let (listener, addr) = listener().await;
        let desktop = tokio::spawn(desktop_sending(addr, KEY, 9 << 30, Vec::new()));
        let mut out = Vec::new();
        assert!(
            await_and_receive(listener, KEY, std::future::pending(), &mut out)
                .await
                .is_err()
        );
        assert_eq!(desktop.await.unwrap(), Some(0));
        assert!(out.is_empty());
    }

    /// Storage that fails, as a full phone does.
    struct Full;

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

    #[tokio::test]
    async fn a_failed_write_answers_zero() {
        let _counters = crate::api::TRANSFER_TEST_LOCK.lock().await;
        let (listener, addr) = listener().await;
        let desktop = tokio::spawn(desktop_sending(addr, KEY, 3, b"abc".to_vec()));
        let got = await_and_receive(listener, KEY, std::future::pending(), &mut Full).await;
        assert!(got.is_err());
        assert_eq!(desktop.await.unwrap(), Some(0));
    }

    #[tokio::test]
    async fn a_refusal_ends_the_wait_with_its_reason() {
        let (listener, _) = listener().await;
        let refused = async { "the offer expired".to_string() };
        let mut out = Vec::new();
        let got = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            await_and_receive(listener, KEY, refused, &mut out),
        )
        .await
        .expect("no wait for the listener's timeout");
        assert_eq!(got, Err("the offer expired".to_string()));
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

    #[test]
    fn offers_are_kept_once_each_and_bounded() {
        let _ = crate::take_file_offers_json();
        for n in 0..20 {
            let body = serde_json::json!({ "id": format!("o{n}"), "name": "a.pdf", "size": 3 });
            crate::queue_file_offer(&body);
            crate::queue_file_offer(&body);
        }
        crate::queue_file_offer(&serde_json::json!({ "name": "no id" }));
        let taken: Vec<serde_json::Value> =
            serde_json::from_str(&crate::take_file_offers_json()).unwrap();
        assert_eq!(taken.len(), 16);
        assert_eq!(taken[0]["id"], "o4", "the oldest go first");
        assert_eq!(crate::take_file_offers_json(), "[]");
    }
}
