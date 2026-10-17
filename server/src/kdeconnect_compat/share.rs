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
}

/// The file a share request offers, held to the rules.
pub fn offer_of(packet: &NetworkPacket) -> Result<Offer, String> {
    let raw = packet
        .body
        .get("filename")
        .and_then(|v| v.as_str())
        .ok_or("not a file (a text or link share)")?;
    let size = packet.payload_size.ok_or("no size")?;
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
        receive(
            &identity, &device_id, pin, address, &offer, &downloads, &allowed,
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
    downloads: &Path,
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
    receive_into(&mut tls, offer, downloads, Pace::DEFAULT, allowed).await
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

/// How often a file on the way asks whether it is still allowed.
const CHECK_EVERY: u64 = 4 * 1024 * 1024;

/// Read exactly `offer.size` bytes from `stream`, at `pace`, into a new
/// file in `downloads`, named like `offer.name`, while `allowed` says so
/// (asked every [`CHECK_EVERY`] bytes and before it is kept).
async fn receive_into<S: AsyncRead + Unpin>(
    stream: &mut S,
    offer: &Offer,
    downloads: &Path,
    pace: Pace,
    allowed: &(dyn Fn() -> bool + Sync),
) -> Result<PathBuf, String> {
    let deadline = tokio::time::Instant::now() + pace.allowed(offer.size);
    tokio::fs::create_dir_all(downloads)
        .await
        .map_err(|e| format!("Downloads: {e}"))?;
    let part = Part(downloads.join(format!(
        ".pocketdesk-kde-{}.part",
        uuid::Uuid::new_v4().simple()
    )));
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part.0)
        .await
        .map_err(|e| format!("could not write in Downloads: {e}"))?;
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
            .map_err(|e| format!("could not write in Downloads: {e}"))?;
        left -= read as u64;
        since_check += read as u64;
    }
    if !allowed() {
        return Err(NO_LONGER.to_string());
    }
    file.flush()
        .await
        .map_err(|e| format!("could not write in Downloads: {e}"))?;
    drop(file);
    crate::plugins::share::place(&part.0, downloads, &offer.name)
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

    fn offering(body: serde_json::Value, size: Option<u64>, port: Option<u64>) -> NetworkPacket {
        let mut packet = NetworkPacket::new(SHARE).with_body(body);
        packet.payload_size = size;
        packet.payload_transfer_info = port.map(|port| serde_json::json!({ "port": port }));
        packet
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
        };
        let bytes: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let (mut device, mut ours) = tokio::io::duplex(8 * 1024);
        let sent = bytes.clone();
        tokio::spawn(async move { device.write_all(&sent).await });
        let path = receive_into(&mut ours, &offer, &downloads, Pace::DEFAULT, &|| true)
            .await
            .unwrap();
        assert_eq!(path, downloads.join("photo.jpg"));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);

        let (mut device, mut ours) = tokio::io::duplex(8 * 1024);
        tokio::spawn(async move {
            let _ = device.write_all(&[1; 1000]).await;
        });
        let short = receive_into(&mut ours, &offer, &downloads, Pace::DEFAULT, &|| true).await;
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
        let stopped = receive_into(&mut ours, &offer, &downloads, Pace::DEFAULT, &allowed).await;
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
        let slow = receive_into(&mut ours, &offer, &downloads, pace, &|| true).await;
        assert!(slow.unwrap_err().contains("too slowly"));
        let _ = std::fs::remove_dir_all(&downloads);
    }

    /// A device offering `bytes` on a port of its own, TLS as the server
    /// with `cert_of`'s certificate; the port.
    async fn device_offering(cert_of: &CompatIdentity, bytes: Vec<u8>) -> u16 {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
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
        port
    }

    #[tokio::test]
    async fn a_file_comes_over_tls_only_from_the_paired_certificate() {
        let ours = CompatIdentity::load_or_create(&temp("ours")).unwrap();
        let phone = CompatIdentity::load_or_create(&temp("phone")).unwrap();
        let pin = super::super::handshake::cert_hash(&phone.cert_der);
        let downloads = temp("tls");
        let localhost: IpAddr = "127.0.0.1".parse().unwrap();

        let port = device_offering(&phone, b"hello".to_vec()).await;
        let offer = Offer {
            name: "note.txt".to_string(),
            size: 5,
            port,
        };
        let path = receive(
            &ours,
            &phone.id,
            pin,
            localhost,
            &offer,
            &downloads,
            &|| true,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"hello");

        let port = device_offering(&phone, b"hello".to_vec()).await;
        let offer = Offer { port, ..offer };
        let refused = receive(
            &ours,
            &phone.id,
            [7; 32],
            localhost,
            &offer,
            &downloads,
            &|| true,
        )
        .await;
        assert!(refused.unwrap_err().contains("TLS"), "another certificate");
        let _ = std::fs::remove_dir_all(&downloads);
    }
}
