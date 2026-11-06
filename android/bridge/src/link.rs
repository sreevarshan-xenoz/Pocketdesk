//! Transfers on the internet road, the phone's side (2d).
//!
//! At home the computer dials the phone back for each transfer. By the
//! internet road it cannot (the phone's router drops it), so it opens a
//! bidirectional stream on the phone's own control connection instead
//! (2c), starting with the transfer's key line. The phone accepts those
//! streams beside the control stream, reads the key and hands the stream to
//! the transfer waiting for it; a request asks for this by naming no port.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use crate::Road;
use pocketdesk_core::protocol::kdeconnect::TransferIo;
use tokio::io::AsyncReadExt;

/// A stream's key line, 32 hex digits and a newline, must come this soon.
const KEY_WAIT: Duration = Duration::from_secs(5);

/// Whether files may move by the internet road: the phone's Settings "Files
/// over the internet", on by default.
pub(crate) static INTERNET_FILES: AtomicBool = AtomicBool::new(true);

type Arrived = tokio::sync::oneshot::Sender<Box<dyn TransferIo>>;

/// The transfers waiting for their stream, by key.
static WAITERS: LazyLock<Mutex<HashMap<String, Arrived>>> = LazyLock::new(Default::default);

/// Accept the streams the computer opens on an internet control connection
/// until it closes, each handed to the transfer its key names.
pub(crate) fn spawn_acceptor(acceptor: pocketdesk_core::streaming::LinkAcceptor) {
    tokio::spawn(async move {
        while let Some(stream) = acceptor.accept().await {
            tokio::spawn(hand_over(stream));
        }
    });
}

/// Read `stream`'s key line and give it to its waiter; drop one with no key
/// in time, or a key no transfer waits for.
async fn hand_over(mut stream: Box<dyn TransferIo>) {
    let mut line = [0u8; 33];
    match tokio::time::timeout(KEY_WAIT, stream.read_exact(&mut line)).await {
        Ok(Ok(_)) if line[32] == b'\n' => {}
        _ => {
            tracing::warn!("File transfer: dropped a stream without a key line");
            return;
        }
    }
    let Ok(key) = std::str::from_utf8(&line[..32]) else {
        return;
    };
    let waiter = WAITERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(key);
    match waiter {
        Some(waiter) => {
            let _ = waiter.send(stream);
        }
        None => tracing::warn!("File transfer: dropped a stream for no transfer waiting"),
    }
}

/// A transfer waiting for its stream on the link. Dropped, it stops
/// waiting.
pub(crate) struct Waiter {
    key: String,
    arrived: tokio::sync::oneshot::Receiver<Box<dyn TransferIo>>,
}

impl Waiter {
    /// Wait for the stream named by `key`, from now: register before the
    /// request goes out, so a fast computer's stream is not missed.
    pub(crate) fn for_key(key: &str) -> Self {
        let (tx, rx) = tokio::sync::oneshot::channel();
        WAITERS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_string(), tx);
        Self {
            key: key.to_string(),
            arrived: rx,
        }
    }

    /// The stream, its key already read.
    pub(crate) async fn arrived(mut self) -> Result<Box<dyn TransferIo>, String> {
        (&mut self.arrived)
            .await
            .map_err(|_| "the link closed before the computer sent the file".to_string())
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        WAITERS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.key);
    }
}

/// Whether a transfer with the computer at `address`, `port` rides the
/// link: the live control connection is to it, by the internet road with
/// files allowed that way, or by Bluetooth, where nothing else reaches it
/// and "Files over the internet" does not hold (the computer refuses a
/// file too big for the road itself).
pub(crate) async fn ready(address: &str, port: u16) -> bool {
    match road_to(address, port).await {
        Some(Road::Internet) => INTERNET_FILES.load(Ordering::Relaxed),
        Some(Road::Bluetooth) => true,
        Some(Road::Home) | None => false,
    }
}

/// The road of the live control connection, when it is to `address`,
/// `port`.
async fn road_to(address: &str, port: u16) -> Option<Road> {
    let road = crate::control_road();
    (*crate::CONTROL_PEER)
        .lock()
        .await
        .as_ref()
        .is_some_and(|(host, live)| host == address && *live == port)
        .then_some(road)
}

/// Whether the live control connection to the computer at `address` came by
/// the internet road (V8) or by Bluetooth: its home address most likely
/// does not answer. How the link was dialed, not a reading of now: a phone
/// back home keeps it. By host alone, as a session names the stream port,
/// not the control one.
pub(crate) async fn home_unreachable(address: &str) -> bool {
    crate::control_road() != Road::Home
        && (*crate::CONTROL_PEER)
            .lock()
            .await
            .as_ref()
            .is_some_and(|(host, _)| host == address)
}

/// What a computer from before 2c answers a request without a port; the
/// phone says what it means.
pub(crate) fn too_old(reason: &str) -> bool {
    // A share, a download, a clipboard image.
    [
        "no usable port",
        "no port to send to",
        "no port to connect to",
    ]
    .iter()
    .any(|old| reason.contains(old))
}

/// Where the computer's connection for one transfer comes from.
pub(crate) enum Arrival {
    /// At home: it dials this listener, inside the transfer's TLS (the
    /// desktop pinned for its address, unless one is given).
    Listen(tokio::net::TcpListener, Option<tokio_rustls::TlsAcceptor>),
    /// By the internet road: a stream on the link.
    Link(Waiter),
}

impl Arrival {
    /// The arrival for a transfer named `key` with the computer at
    /// `address`, `port` (its control port).
    pub(crate) async fn new(address: &str, port: u16, key: &str) -> Result<Self, String> {
        if ready(address, port).await {
            return Ok(Self::Link(Waiter::for_key(key)));
        }
        // Linked by the internet road but with the switch off: nothing else
        // can reach that computer, so say why now.
        if !INTERNET_FILES.load(Ordering::Relaxed)
            && road_to(address, port).await == Some(Road::Internet)
        {
            return Err("Files over the internet are off in this phone's Settings".to_string());
        }
        tokio::net::TcpListener::bind("0.0.0.0:0")
            .await
            .map(|listener| Self::Listen(listener, None))
            .map_err(|e| format!("Failed to bind port: {e}"))
    }

    /// The port the request names: none on the link.
    pub(crate) fn port(&self) -> Option<u16> {
        match self {
            Self::Listen(listener, _) => listener.local_addr().ok().map(|a| a.port()),
            Self::Link(_) => None,
        }
    }

    /// The computer's connection, its key checked: at home inside TLS with
    /// the desktop pinned for `address`, looked up now (the request's
    /// connection may have pinned it just before).
    pub(crate) async fn take(
        self,
        address: &str,
        key: &str,
    ) -> Result<Box<dyn TransferIo>, String> {
        match self {
            Self::Listen(listener, tls) => {
                let tls = match tls {
                    Some(tls) => tls,
                    None => crate::api::transfer_acceptor(address)?,
                };
                let stream = crate::api::accept_transfer(&listener, key, &tls).await?;
                Ok(Box::new(stream))
            }
            Self::Link(waiter) => waiter.arrived().await,
        }
    }
}

/// A request's `payloadTransferInfo` (or fetch body) for `arrival`: the
/// key, and the port at home only.
pub(crate) fn transfer_info(arrival: &Arrival, key: &str) -> serde_json::Value {
    match arrival.port() {
        Some(port) => serde_json::json!({ "port": port, "key": key }),
        None => serde_json::json!({ "key": key }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    const KEY: &str = "0123456789abcdef0123456789abcdef";

    #[tokio::test]
    async fn a_stream_with_a_waiting_key_goes_to_its_transfer() {
        let waiter = Waiter::for_key(KEY);
        let (mut computer, phone) = tokio::io::duplex(1024);
        computer
            .write_all(format!("{KEY}\nrest").as_bytes())
            .await
            .unwrap();
        tokio::spawn(hand_over(Box::new(phone)));
        let mut stream = waiter.arrived().await.unwrap();
        let mut rest = [0u8; 4];
        stream.read_exact(&mut rest).await.unwrap();
        assert_eq!(&rest, b"rest", "the key line read, nothing after it");
        assert!(!WAITERS.lock().unwrap().contains_key(KEY));
    }

    #[tokio::test]
    async fn a_stream_for_no_transfer_or_without_a_key_is_dropped() {
        let (mut computer, phone) = tokio::io::duplex(1024);
        computer
            .write_all(b"ffffffffffffffffffffffffffffffff\n")
            .await
            .unwrap();
        hand_over(Box::new(phone)).await;
        let mut byte = [0u8; 1];
        assert_eq!(
            computer.read(&mut byte).await.unwrap(),
            0,
            "closed: no transfer waits for that key"
        );
        let (computer, phone) = tokio::io::duplex(1024);
        drop(computer);
        hand_over(Box::new(phone)).await;
    }

    #[test]
    fn a_waiter_that_gives_up_is_forgotten() {
        let key = "fedcba9876543210fedcba9876543210";
        drop(Waiter::for_key(key));
        assert!(!WAITERS.lock().unwrap().contains_key(key));
    }

    /// What each road means for a transfer with the computer linked by it:
    /// home dials back; the internet road rides the link while "Files over
    /// the internet" is on; Bluetooth rides the link whatever that says.
    /// Home does not answer on either of the other two.
    #[tokio::test]
    async fn each_road_says_how_a_transfer_arrives() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        let was_peer = crate::CONTROL_PEER.lock().await.clone();
        let was_road = crate::control_road();
        let was_files = INTERNET_FILES.load(Ordering::Relaxed);
        *crate::CONTROL_PEER.lock().await = Some(("192.0.2.10".to_string(), 1716));
        let mut seen = Vec::new();
        for road in [Road::Home, Road::Internet, Road::Bluetooth] {
            crate::set_control_road(road);
            for files in [true, false] {
                INTERNET_FILES.store(files, Ordering::Relaxed);
                seen.push((
                    road,
                    files,
                    ready("192.0.2.10", 1716).await,
                    home_unreachable("192.0.2.10").await,
                ));
            }
            assert!(!ready("192.0.2.11", 1716).await, "another computer");
            assert!(!ready("192.0.2.10", 1717).await, "another port");
            assert!(!home_unreachable("192.0.2.11").await);
        }
        crate::set_control_road(Road::Internet);
        INTERNET_FILES.store(false, Ordering::Relaxed);
        let internet_off = Arrival::new("192.0.2.10", 1716, KEY).await.err();
        crate::set_control_road(Road::Bluetooth);
        let bluetooth_off = Arrival::new("192.0.2.10", 1716, KEY).await;
        *crate::CONTROL_PEER.lock().await = was_peer;
        crate::set_control_road(was_road);
        INTERNET_FILES.store(was_files, Ordering::Relaxed);
        assert_eq!(
            seen,
            [
                (Road::Home, true, false, false),
                (Road::Home, false, false, false),
                (Road::Internet, true, true, true),
                (Road::Internet, false, false, true),
                (Road::Bluetooth, true, true, true),
                (Road::Bluetooth, false, true, true),
            ]
        );
        assert!(internet_off.is_some_and(|e| e.contains("Files over the internet")));
        assert!(
            bluetooth_off.is_ok_and(|arrival| arrival.port().is_none()),
            "on the link, asking with no port"
        );
    }

    #[test]
    fn a_computer_from_before_2c_is_known_by_its_refusal() {
        assert!(too_old("no usable port"));
        assert!(too_old("no port to send to"));
        assert!(too_old("no port to connect to"));
        assert!(!too_old("the phone may not send files"));
    }
}
