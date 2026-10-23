//! Files a paired KDE Connect device sends (3c), only with its Files
//! permission (off by default). The device offers each file in a
//! `kdeconnect.share.request`, with its size and a port it listens on
//! (1739–1764); this computer dials that port at the device's address, runs
//! TLS as the client with its KDE certificate, taking only the certificate
//! the device paired with, and reads exactly that many bytes into Downloads,
//! under a name no file there has yet, marked as downloaded. A text or link
//! share is not taken.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use pocketdesk_core::protocol::kdeconnect::NetworkPacket;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio_rustls::rustls;

use super::identity::CompatIdentity;

/// A device offering a file.
pub const SHARE: &str = "kdeconnect.share.request";

/// The ports KDE Connect offers a payload on.
pub const PORTS: std::ops::RangeInclusive<u16> = 1739..=1764;

/// The largest file taken, as from a phone.
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// Dialing the device's port, and its TLS handshake.
const STEP: Duration = Duration::from_secs(10);

/// How long a file waits for a turn.
const TURN_WAIT: Duration = Duration::from_secs(60);

/// Files from KDE Connect devices on the way at once.
static AT_ONCE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

/// One device's files at once, on the way or waiting for a turn (3c
/// review: each offer had been a task of its own, unbounded); more are
/// refused.
const PER_DEVICE: usize = 4;

/// How many files each device has on the way or waiting.
static TAKEN: std::sync::Mutex<std::collections::BTreeMap<String, usize>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// One of a device's places, given back when dropped.
pub struct Ticket(String);

impl Drop for Ticket {
    fn drop(&mut self) {
        let mut taken = TAKEN.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(count) = taken.get_mut(&self.0) {
            *count -= 1;
            if *count == 0 {
                taken.remove(&self.0);
            }
        }
    }
}

/// A place for one of `device_id`'s files, while it has fewer than
/// [`PER_DEVICE`].
pub fn admit(device_id: &str) -> Option<Ticket> {
    let mut taken = TAKEN.lock().unwrap_or_else(|e| e.into_inner());
    let count = taken.entry(device_id.to_string()).or_insert(0);
    if *count >= PER_DEVICE {
        return None;
    }
    *count += 1;
    Some(Ticket(device_id.to_string()))
}

/// How slowly a file may come: silent at most `quiet`, and on the whole at
/// least `min_rate` bytes a second after that (the phone road's floor), so
/// a device trickling bytes cannot hold a turn for good (3c review).
#[derive(Debug, Clone, Copy)]
pub struct Pace {
    pub quiet: Duration,
    pub min_rate: u64,
}

impl Pace {
    pub const DEFAULT: Pace = Pace {
        quiet: Duration::from_secs(30),
        min_rate: 32 * 1024,
    };

    /// How long `size` bytes may take in all.
    fn allowed(&self, size: u64) -> Duration {
        self.quiet + Duration::from_secs(size / self.min_rate.max(1))
    }
}

/// A file a device offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// Its name, made safe for Downloads.
    pub name: String,
    pub size: u64,
    pub port: u16,
    /// Its `lastModified`, set once it is kept (K1).
    pub modified: Option<std::time::SystemTime>,
}

/// The file a share request offers, held to the rules.
pub fn offer_of(packet: &NetworkPacket) -> Result<Offer, String> {
    let raw = packet
        .body
        .get("filename")
        .and_then(|v| v.as_str())
        .ok_or("not a file (a text or link share)")?;
    let size = packet
        .payload_size
        .ok_or("no size (a stream of no known length)")?;
    if size == 0 {
        return Err("an empty file".to_string());
    }
    if size > MAX_FILE_BYTES {
        return Err(format!("{size} bytes is more than this computer takes"));
    }
    let port = packet
        .payload_transfer_info
        .as_ref()
        .and_then(|info| info.get("port"))
        .and_then(|port| port.as_u64())
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| PORTS.contains(port))
        .ok_or("not a port KDE Connect offers on")?;
    Ok(Offer {
        name: crate::plugins::share::safe_file_name(raw),
        size,
        port,
        modified: crate::plugins::share::received_date(&packet.body),
    })
}

/// Fetch `offer` from `device_id` (`name`, at `address`) and keep it, as a
/// phone's file is kept; a failure is logged and told. `ticket` is the
/// device's place, held until it is done.
pub async fn fetch(
    dir: PathBuf,
    device_id: String,
    name: String,
    address: Option<IpAddr>,
    offer: Offer,
    ticket: Ticket,
) {
    let _ticket = ticket;
    let fetched = async {
        let address = address
            .filter(|ip| crate::plugins::share::dialable(*ip))
            .ok_or("its address cannot be dialed")?;
        let pin = super::link::pin_of(&dir, &device_id).ok_or("not paired")?;
        let identity = super::load_identity().map_err(|e| format!("no KDE identity: {e:#}"))?;
        let downloads =
            crate::plugins::share::download_dir().ok_or("no Downloads folder to put it in")?;
        // Still paired with the same certificate, and still allowed files
        // (3c review: a transfer had outlived both).
        let allowed = || {
            super::link::pin_of(&dir, &device_id) == Some(pin)
                && pocketdesk_core::local::kdeconnect_devices::permissions(&dir, &device_id)
                    .is_some_and(|p| p.files)
        };
        let places = Places {
            parts: dir.join(PARTS_DIR),
            downloads,
        };
        receive(
            &identity, &device_id, pin, address, &offer, &places, &allowed,
        )
        .await
    }
    .await;
    match fetched {
        Ok(path) => {
            tracing::info!(
                device_id,
                size = offer.size,
                "A file from a KDE Connect device was kept"
            );
            crate::plugins::share::on_received(&path, offer.size, &device_id, None);
        }
        Err(why) => {
            tracing::warn!(
                device_id,
                "A file from a KDE Connect device was not received: {why}"
            );
            super::link::notify(
                "A file from a KDE Connect device was not received",
                format!("{}, from {name}: {why}", offer.name),
            );
        }
    }
}

/// Dial the device's port, TLS as the client held to `pin`, and read the
/// file into `downloads`.
async fn receive(
    identity: &CompatIdentity,
    device_id: &str,
    pin: [u8; 32],
    address: IpAddr,
    offer: &Offer,
    places: &Places,
    allowed: &(dyn Fn() -> bool + Sync),
) -> Result<PathBuf, String> {
    let _turn = tokio::time::timeout(TURN_WAIT, AT_ONCE.acquire())
        .await
        .map_err(|_| "busy with other files".to_string())?
        .map_err(|e| e.to_string())?;
    let tcp = tokio::time::timeout(STEP, tokio::net::TcpStream::connect((address, offer.port)))
        .await
        .map_err(|_| "the device did not answer".to_string())?
        .map_err(|e| format!("the device did not answer: {e}"))?;
    let config = super::handshake::client_config(identity, device_id, Some(pin))?;
    // The device's certificate is judged by its subject and pin, not a name.
    let name = rustls::pki_types::ServerName::try_from("kdeconnect").map_err(|e| e.to_string())?;
    let mut tls = tokio::time::timeout(
        STEP,
        tokio_rustls::TlsConnector::from(config).connect(name, tcp),
    )
    .await
    .map_err(|_| "no TLS handshake in time".to_string())?
    .map_err(|e| format!("TLS: {e}"))?;
    receive_into(&mut tls, offer, places, Pace::DEFAULT, allowed).await
}

/// How long a device is given to come for a file once told of it (T6).
const COME_FOR_IT: Duration = Duration::from_secs(60);

/// Files this computer sends KDE Connect devices at once (T6).
static SENDING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

/// How long a file waits for the one before it to the same device (T6
/// review: a device fetches one file at a time, so a second one told at
/// once timed out while the first went).
const DEVICE_WAIT: Duration = Duration::from_secs(30 * 60);

/// Each device's turn: one file to it at a time.
fn device_turn(device_id: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    static TURNS: std::sync::LazyLock<
        std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::LazyLock::new(Default::default);
    let mut turns = TURNS.lock().unwrap_or_else(|e| e.into_inner());
    // Turns no one holds or waits for go.
    turns.retain(|_, turn| std::sync::Arc::strong_count(turn) > 1);
    turns.entry(device_id.to_string()).or_default().clone()
}

/// Send the file at `path` to the paired KDE Connect device `device_id`
/// (T6), as KDE Connect sends one: this computer listens on a port of
/// [`PORTS`], tells the device of the file on its link, and the device
/// comes for it from the link's address, TLS with this computer as the
/// server, held to the device's pinned certificate; the file's bytes, then
/// done. The device keeps it in its own Downloads (its owner may be asked).
pub async fn send(device_id: &str, path: &Path) -> Result<(), String> {
    let dir = crate::state::state_dir().map_err(|e| e.to_string())?;
    let pin = super::link::pin_of(&dir, device_id).ok_or("the device is not paired")?;
    let identity = super::load_identity().map_err(|e| format!("no KDE identity: {e:#}"))?;
    let turn = device_turn(device_id);
    let _device = tokio::time::timeout(DEVICE_WAIT, turn.lock())
        .await
        .map_err(|_| "the files before it to this device took too long".to_string())?;
    let _turn = tokio::time::timeout(TURN_WAIT, SENDING.acquire())
        .await
        .map_err(|_| "busy with other files".to_string())?
        .map_err(|e| e.to_string())?;
    // Its address as it is now, after the wait (T6 review).
    let address = super::link::address_of(device_id).ok_or("the device is not connected now")?;
    // This computer listens on IPv4 (T6 review: a device linked over IPv6
    // dialled a closed port and the send waited a minute).
    if !address.to_canonical().is_ipv4() {
        return Err("the device is linked over IPv6, where it cannot come for a file".to_string());
    }
    let listener = listen().await?;
    send_with(
        &identity,
        device_id,
        pin,
        address,
        listener,
        path,
        |offer| super::link::tell(device_id, offer),
    )
    .await
}

/// A listener on the first free port of [`PORTS`].
async fn listen() -> Result<tokio::net::TcpListener, String> {
    for port in PORTS {
        if let Ok(listener) = tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
            return Ok(listener);
        }
    }
    Err("no free port for KDE Connect files (1739 to 1764)".to_string())
}

/// [`send`]'s work: `tell` gives the link the offer.
async fn send_with(
    identity: &CompatIdentity,
    device_id: &str,
    pin: [u8; 32],
    address: IpAddr,
    listener: tokio::net::TcpListener,
    path: &Path,
    tell: impl FnOnce(super::link::Event) -> bool,
) -> Result<(), String> {
    let meta = tokio::fs::metadata(path)
        .await
        .map_err(|e| format!("it cannot be read: {e}"))?;
    if !meta.is_file() {
        return Err("not a file".to_string());
    }
    let size = meta.len();
    // As this computer takes a device's: a KDE device makes nothing of one.
    if size == 0 {
        return Err("an empty file".to_string());
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or("no file name")?;
    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64);
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let config = super::handshake::server_config(identity, device_id, pin)?;
    if !tell(super::link::Event::Share {
        name,
        size,
        port,
        modified_ms,
    }) {
        return Err("the device is not connected now".to_string());
    }
    // Only the device, from its link's address, may come for it.
    let deadline = tokio::time::Instant::now() + COME_FOR_IT;
    let tcp = loop {
        let (tcp, from) = tokio::time::timeout_at(deadline, listener.accept())
            .await
            .map_err(|_| "the device did not come for it (is KDE Connect open there?)".to_string())?
            .map_err(|e| e.to_string())?;
        if from.ip().to_canonical() == address.to_canonical() {
            break tcp;
        }
        tracing::warn!("A connection for a KDE Connect file from another address was refused");
    };
    drop(listener);
    let mut tls = tokio::time::timeout(STEP, tokio_rustls::TlsAcceptor::from(config).accept(tcp))
        .await
        .map_err(|_| "no TLS handshake in time".to_string())?
        .map_err(|e| format!("TLS: {e}"))?;
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| format!("it cannot be read: {e}"))?;
    // In pieces, each given the pace's quiet time (T6 review: a device that
    // stopped reading held the send for as long as the system kept the
    // connection).
    let pace = Pace::DEFAULT;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut sent = 0u64;
    while sent < size {
        let want = buffer.len().min((size - sent) as usize);
        let n = file
            .read(&mut buffer[..want])
            .await
            .map_err(|e| format!("it cannot be read: {e}"))?;
        if n == 0 {
            return Err("the file shrank while it was sent".to_string());
        }
        tokio::time::timeout(pace.quiet, tls.write_all(&buffer[..n]))
            .await
            .map_err(|_| "the device stopped taking it".to_string())?
            .map_err(|e| format!("it broke off: {e}"))?;
        sent += n as u64;
    }
    tokio::time::timeout(pace.quiet, tls.flush())
        .await
        .map_err(|_| "the device stopped taking it".to_string())?
        .map_err(|e| format!("it broke off: {e}"))?;
    let _ = tls.shutdown().await;
    Ok(())
}

/// Why a file stopped when its device was no longer allowed it.
const NO_LONGER: &str = "the device is no longer paired or allowed files";

/// A partial file, removed unless it was placed.
struct Part(PathBuf);

impl Drop for Part {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Where a file waits until it is whole, in the state folder (3c review:
/// in Downloads a synced folder or an indexer saw it grow, and a crash left
/// it there for good). Emptied at start.
pub const PARTS_DIR: &str = "kdeconnect_parts";

/// Empty [`PARTS_DIR`]: at start no file is on the way.
pub fn sweep_parts(state_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(state_dir.join(PARTS_DIR)) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "part") {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// How often a file on the way asks whether it is still allowed.
const CHECK_EVERY: u64 = 4 * 1024 * 1024;

/// Where a file waits, and where it is kept.
struct Places {
    parts: PathBuf,
    downloads: PathBuf,
}

/// Read exactly `offer.size` bytes from `stream`, at `pace`, into a part in
/// `places.parts`, then keep it in `places.downloads` named like
/// `offer.name`, while `allowed` says so (asked every [`CHECK_EVERY`]
/// bytes and before it is kept).
async fn receive_into<S: AsyncRead + Unpin>(
    stream: &mut S,
    offer: &Offer,
    places: &Places,
    pace: Pace,
    allowed: &(dyn Fn() -> bool + Sync),
) -> Result<PathBuf, String> {
    let deadline = tokio::time::Instant::now() + pace.allowed(offer.size);
    tokio::fs::create_dir_all(&places.parts)
        .await
        .map_err(|e| format!("the state folder: {e}"))?;
    let part = Part(
        places
            .parts
            .join(format!("{}.part", uuid::Uuid::new_v4().simple())),
    );
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part.0)
        .await
        .map_err(|e| format!("could not write its part: {e}"))?;
    let mut left = offer.size;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut since_check = 0u64;
    while left > 0 {
        if since_check >= CHECK_EVERY {
            since_check = 0;
            if !allowed() {
                return Err(NO_LONGER.to_string());
            }
        }
        let want = buffer
            .len()
            .min(usize::try_from(left).unwrap_or(usize::MAX));
        let wait = pace
            .quiet
            .min(deadline.saturating_duration_since(tokio::time::Instant::now()));
        let read = tokio::time::timeout(wait, stream.read(&mut buffer[..want]))
            .await
            .map_err(|_| {
                if tokio::time::Instant::now() >= deadline {
                    "the device sent it too slowly".to_string()
                } else {
                    "the device went quiet".to_string()
                }
            })?
            .map_err(|e| format!("the device's connection: {e}"))?;
        if read == 0 {
            return Err(format!("the device stopped {left} bytes before the end"));
        }
        file.write_all(&buffer[..read])
            .await
            .map_err(|e| format!("could not write its part: {e}"))?;
        left -= read as u64;
        since_check += read as u64;
    }
    if !allowed() {
        return Err(NO_LONGER.to_string());
    }
    file.flush()
        .await
        .map_err(|e| format!("could not write its part: {e}"))?;
    drop(file);
    crate::plugins::share::place(&part.0, &places.downloads, &offer.name, offer.modified)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pd-kde-share-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Places beside `downloads`: the parts in a folder of their own.
    fn places(downloads: &Path) -> Places {
        Places {
            parts: downloads.with_extension("parts"),
            downloads: downloads.to_path_buf(),
        }
    }

    /// T6: a file sent to a device: told on its link with the port, taken
    /// from that port by the device over TLS with each side's certificate,
    /// byte for byte; a device with another certificate is refused.
    #[tokio::test]
    async fn a_file_goes_to_the_device_that_comes_for_it() {
        let dir = temp("send");
        let ours = CompatIdentity::load_or_create(&dir.join("ours")).unwrap();
        let device = CompatIdentity::load_or_create(&dir.join("device")).unwrap();
        let stranger = CompatIdentity::load_or_create(&dir.join("stranger")).unwrap();
        let file = dir.join("Report.pdf");
        let bytes: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&file, &bytes).unwrap();
        let pin = super::super::handshake::cert_hash(&device.cert_der);
        let ours_pin = super::super::handshake::cert_hash(&ours.cert_der);

        let come_for_it = |who: CompatIdentity, port: u16| {
            let ours_id = ours.id.clone();
            tokio::spawn(async move {
                let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
                    .await
                    .unwrap();
                let config =
                    super::super::handshake::client_config(&who, &ours_id, Some(ours_pin)).unwrap();
                let name = rustls::pki_types::ServerName::try_from("kdeconnect").unwrap();
                let mut tls = tokio_rustls::TlsConnector::from(config)
                    .connect(name, tcp)
                    .await
                    .map_err(|e| e.to_string())?;
                let mut got = Vec::new();
                tls.read_to_end(&mut got).await.map_err(|e| e.to_string())?;
                Ok::<_, String>(got)
            })
        };

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        // The device comes for it as it is told; the listener is up first.
        let device_side = come_for_it(device.clone(), port);
        let (told, offered) = std::sync::mpsc::channel();
        send_with(
            &ours,
            &device.id,
            pin,
            "127.0.0.1".parse().unwrap(),
            listener,
            &file,
            |offer| told.send(offer).is_ok(),
        )
        .await
        .unwrap();
        let super::super::link::Event::Share {
            name,
            size,
            port: told_port,
            ..
        } = offered.try_recv().unwrap()
        else {
            panic!("not a share");
        };
        assert_eq!(
            (name.as_str(), size, told_port),
            ("Report.pdf", 300_000, port)
        );
        assert_eq!(device_side.await.unwrap().unwrap(), bytes);

        // Another certificate is refused: the send ends without the bytes.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let refused = tokio::join!(
            send_with(
                &ours,
                &device.id,
                pin,
                "127.0.0.1".parse().unwrap(),
                listener,
                &file,
                |_| true
            ),
            come_for_it(stranger, port)
        );
        assert!(refused.0.is_err(), "the stranger's certificate");
        assert!(refused.1.unwrap().is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parts_left_by_a_crash_are_swept_at_start() {
        let state = temp("sweep");
        let parts = state.join(PARTS_DIR);
        std::fs::create_dir_all(&parts).unwrap();
        std::fs::write(parts.join("left.part"), b"x").unwrap();
        std::fs::write(parts.join("other.txt"), b"x").unwrap();
        sweep_parts(&state);
        assert!(!parts.join("left.part").exists());
        assert!(parts.join("other.txt").exists(), "only parts");
        let _ = std::fs::remove_dir_all(&state);
    }

    fn offering(body: serde_json::Value, size: Option<u64>, port: Option<u64>) -> NetworkPacket {
        let mut packet = NetworkPacket::new(SHARE).with_body(body);
        packet.payload_size = size;
        packet.payload_transfer_info = port.map(|port| serde_json::json!({ "port": port }));
        packet
    }

    /// K1: a KDE device's `lastModified` is the kept file's date.
    #[test]
    fn a_file_offer_reads_its_date() {
        let offer = offer_of(&offering(
            serde_json::json!({ "filename": "a.jpg", "lastModified": 1_780_000_000_000u64 }),
            Some(5),
            Some(1739),
        ))
        .unwrap();
        // None only with "Keep files' dates" turned off in this computer's
        // config, which a test does not choose.
        if crate::events::current_config().keep_file_dates {
            assert_eq!(
                offer.modified,
                Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_780_000_000))
            );
        }
        let undated = offer_of(&offering(
            serde_json::json!({ "filename": "a.jpg" }),
            Some(5),
            Some(1739),
        ))
        .unwrap();
        assert_eq!(undated.modified, None);
    }

    #[test]
    fn a_file_offer_reads_and_its_name_is_made_safe() {
        let offer = offer_of(&offering(
            serde_json::json!({ "filename": "../../evil.jpg" }),
            Some(5),
            Some(1739),
        ))
        .unwrap();
        assert_eq!(offer.size, 5);
        assert_eq!(offer.port, 1739);
        assert!(
            !offer.name.contains('/') && !offer.name.contains(".."),
            "{}",
            offer.name
        );
        for (body, size, port) in [
            (serde_json::json!({ "text": "hello" }), Some(5), Some(1739)),
            (
                serde_json::json!({ "url": "https://example.org" }),
                None,
                None,
            ),
            (serde_json::json!({ "filename": "a" }), None, Some(1739)),
            (serde_json::json!({ "filename": "a" }), Some(0), Some(1739)),
            (serde_json::json!({ "filename": "a" }), Some(5), None),
            (serde_json::json!({ "filename": "a" }), Some(5), Some(22)),
            (
                serde_json::json!({ "filename": "a" }),
                Some(MAX_FILE_BYTES + 1),
                Some(1739),
            ),
        ] {
            assert!(
                offer_of(&offering(body.clone(), size, port)).is_err(),
                "{body} {size:?} {port:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_file_is_read_whole_into_downloads_and_a_short_one_leaves_nothing() {
        let downloads = temp("whole");
        let offer = Offer {
            name: "photo.jpg".to_string(),
            size: 200_000,
            port: 1739,
            modified: None,
        };
        let bytes: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let (mut device, mut ours) = tokio::io::duplex(8 * 1024);
        let sent = bytes.clone();
        tokio::spawn(async move { device.write_all(&sent).await });
        let path = receive_into(
            &mut ours,
            &offer,
            &places(&downloads),
            Pace::DEFAULT,
            &|| true,
        )
        .await
        .unwrap();
        assert_eq!(path, downloads.join("photo.jpg"));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);

        let (mut device, mut ours) = tokio::io::duplex(8 * 1024);
        tokio::spawn(async move {
            let _ = device.write_all(&[1; 1000]).await;
        });
        let short = receive_into(
            &mut ours,
            &offer,
            &places(&downloads),
            Pace::DEFAULT,
            &|| true,
        )
        .await;
        assert!(short.unwrap_err().contains("before the end"));
        let left: Vec<_> = std::fs::read_dir(&downloads).unwrap().flatten().collect();
        assert_eq!(left.len(), 1, "only the first file, no part left behind");
        let _ = std::fs::remove_dir_all(&downloads);
    }

    #[test]
    fn a_device_has_a_few_files_on_the_way_at_once() {
        let id = format!("intake{}", uuid::Uuid::new_v4().simple());
        let tickets: Vec<_> = std::iter::from_fn(|| admit(&id)).take(10).collect();
        assert_eq!(tickets.len(), PER_DEVICE);
        assert!(admit("another device").is_some(), "per device");
        drop(tickets);
        assert!(admit(&id).is_some(), "given back");
    }

    #[tokio::test]
    async fn a_file_stops_once_its_device_is_allowed_it_no_more() {
        let downloads = temp("revoked");
        let size = 3 * CHECK_EVERY;
        let offer = Offer {
            name: "big.bin".to_string(),
            size,
            port: 1739,
            modified: None,
        };
        let (mut device, mut ours) = tokio::io::duplex(256 * 1024);
        tokio::spawn(async move {
            let chunk = vec![7u8; 64 * 1024];
            for _ in 0..(size / chunk.len() as u64) {
                if device.write_all(&chunk).await.is_err() {
                    break;
                }
            }
        });
        let asked = std::sync::atomic::AtomicUsize::new(0);
        // Allowed at the first look, not at the second.
        let allowed = || asked.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0;
        let stopped = receive_into(
            &mut ours,
            &offer,
            &places(&downloads),
            Pace::DEFAULT,
            &allowed,
        )
        .await;
        assert_eq!(stopped.unwrap_err(), NO_LONGER);
        assert_eq!(
            std::fs::read_dir(&downloads).unwrap().count(),
            0,
            "nothing kept, no part left"
        );
        let _ = std::fs::remove_dir_all(&downloads);
    }

    #[tokio::test(start_paused = true)]
    async fn a_file_that_trickles_in_is_given_up() {
        let downloads = temp("slow");
        let offer = Offer {
            name: "slow.bin".to_string(),
            size: 100,
            port: 1739,
            modified: None,
        };
        let (mut device, mut ours) = tokio::io::duplex(1024);
        tokio::spawn(async move {
            // A byte every 2 s: never quiet for long, far under the rate.
            for _ in 0..100 {
                if device.write_all(&[1]).await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
        let pace = Pace {
            quiet: Duration::from_secs(5),
            min_rate: 10,
        };
        let slow = receive_into(&mut ours, &offer, &places(&downloads), pace, &|| true).await;
        assert!(slow.unwrap_err().contains("too slowly"));
        let _ = std::fs::remove_dir_all(&downloads);
    }

    /// A device offering `bytes` on a port of its own, TLS as the server
    /// with `cert_of`'s certificate; the port.
    /// The certificate a client presented, which the device requires (as
    /// KDE Connect's sender does).
    type Presented = Arc<std::sync::Mutex<Option<Vec<u8>>>>;

    #[derive(Debug)]
    struct RequiresOne(Presented);

    impl rustls::server::danger::ClientCertVerifier for RequiresOne {
        fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
            &[]
        }
        fn client_auth_mandatory(&self) -> bool {
            true
        }
        fn verify_client_cert(
            &self,
            end_entity: &rustls::pki_types::CertificateDer<'_>,
            _: &[rustls::pki_types::CertificateDer<'_>],
            _: rustls::pki_types::UnixTime,
        ) -> Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
            *self.0.lock().unwrap() = Some(end_entity.as_ref().to_vec());
            Ok(rustls::server::danger::ClientCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            m: &[u8],
            c: &rustls::pki_types::CertificateDer<'_>,
            d: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls12_signature(
                m,
                c,
                d,
                &rustls::crypto::ring::default_provider().signature_verification_algorithms,
            )
        }
        fn verify_tls13_signature(
            &self,
            m: &[u8],
            c: &rustls::pki_types::CertificateDer<'_>,
            d: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls13_signature(
                m,
                c,
                d,
                &rustls::crypto::ring::default_provider().signature_verification_algorithms,
            )
        }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            rustls::crypto::ring::default_provider()
                .signature_verification_algorithms
                .supported_schemes()
        }
    }

    async fn device_offering(cert_of: &CompatIdentity, bytes: Vec<u8>) -> (u16, Presented) {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let presented = Presented::default();
        let config = rustls::ServerConfig::builder()
            .with_client_cert_verifier(Arc::new(RequiresOne(presented.clone())))
            .with_single_cert(
                vec![rustls::pki_types::CertificateDer::from(
                    cert_of.cert_der.clone(),
                )],
                rustls::pki_types::PrivateKeyDer::try_from(cert_of.key_der.clone()).unwrap(),
            )
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            if let Ok(mut tls) = tokio_rustls::TlsAcceptor::from(Arc::new(config))
                .accept(tcp)
                .await
            {
                let _ = tls.write_all(&bytes).await;
                let _ = tls.shutdown().await;
            }
        });
        (port, presented)
    }

    #[tokio::test]
    async fn a_file_comes_over_tls_only_from_the_paired_certificate() {
        let ours = CompatIdentity::load_or_create(&temp("ours")).unwrap();
        let phone = CompatIdentity::load_or_create(&temp("phone")).unwrap();
        let pin = super::super::handshake::cert_hash(&phone.cert_der);
        let downloads = temp("tls");
        let localhost: IpAddr = "127.0.0.1".parse().unwrap();

        let (port, presented) = device_offering(&phone, b"hello".to_vec()).await;
        let offer = Offer {
            name: "note.txt".to_string(),
            size: 5,
            port,
            modified: None,
        };
        let path = receive(
            &ours,
            &phone.id,
            pin,
            localhost,
            &offer,
            &places(&downloads),
            &|| true,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"hello");
        assert_eq!(
            presented.lock().unwrap().as_deref(),
            Some(ours.cert_der.as_slice()),
            "this computer's KDE certificate, as the device requires"
        );

        let (port, _) = device_offering(&phone, b"hello".to_vec()).await;
        let offer = Offer { port, ..offer };
        let refused = receive(
            &ours,
            &phone.id,
            [7; 32],
            localhost,
            &offer,
            &places(&downloads),
            &|| true,
        )
        .await;
        assert!(refused.unwrap_err().contains("TLS"), "another certificate");
        let _ = std::fs::remove_dir_all(&downloads);
    }
}
