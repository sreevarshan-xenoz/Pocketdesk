//! Files the desktop's owner sends to a phone (roadmap F7b).
//!
//! The desktop app writes a request to the outbox (`core::local::outbox`).
//! The watcher here takes it, checks the file and the phone, and pushes the
//! phone a `file_offer` `{id, name, size}`. The phone then fetches the file
//! by that id (`share::FETCH`), so the desktop still dials the phone and
//! the phone can name nothing but what the owner chose. An offer is single
//! use, expires after [`OFFER_TTL`], and is valid only from its phone.
//! Each send and its outcome go to `sent.json` for the app's Files tab.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use linux_link_core::local::outbox::{self, SendRequest};
use linux_link_core::local::pin::now_unix;
use linux_link_core::local::sent::{self, SentFile, SentState};
use linux_link_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, TrustStore};
use linux_link_core::protocol::permissions::Capability;

/// Pushed to the phone: a file is waiting for it, `{id, name, size}`.
pub const OFFER: &str = "kdeconnect.linuxlink.file_offer";
/// How long the phone has to fetch an offered file.
pub const OFFER_TTL: Duration = Duration::from_secs(120);
/// The largest file offered, as for every transfer.
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// How often the outbox is read.
const POLL: Duration = Duration::from_millis(250);

/// A file offered to one phone.
#[derive(Debug, Clone)]
struct Offer {
    path: PathBuf,
    device_id: String,
    expires: Instant,
}

fn offers() -> &'static Mutex<HashMap<String, Offer>> {
    static OFFERS: OnceLock<Mutex<HashMap<String, Offer>>> = OnceLock::new();
    OFFERS.get_or_init(Default::default)
}

/// Offer `path` to `device_id` under `id` (the outbox request's id).
pub(crate) fn offer_at(id: &str, path: PathBuf, device_id: &str, now: Instant) {
    let mut offers = offers().lock().unwrap_or_else(|e| e.into_inner());
    offers.retain(|_, o| o.expires > now);
    offers.insert(
        id.to_string(),
        Offer {
            path,
            device_id: device_id.to_string(),
            expires: now + OFFER_TTL,
        },
    );
}

/// The file offered as `id`, taken by `device_id`: the offer is used up.
/// An offer for another phone is refused and left for its own phone.
pub fn redeem(id: &str, device_id: &str) -> Result<PathBuf, &'static str> {
    redeem_at(id, device_id, Instant::now())
}

fn redeem_at(id: &str, device_id: &str, now: Instant) -> Result<PathBuf, &'static str> {
    let mut offers = offers().lock().unwrap_or_else(|e| e.into_inner());
    let offer = offers.get(id).ok_or("the offer was used or is unknown")?;
    if offer.device_id != device_id {
        return Err("the file was offered to another phone");
    }
    let offer = offers
        .remove(id)
        .ok_or("the offer was used or is unknown")?;
    if offer.expires <= now {
        return Err("the offer expired");
    }
    Ok(offer.path)
}

/// Record how the fetch of offer `id` ended.
pub fn finish(id: &str, outcome: Result<(), String>) {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let state = match outcome {
        Ok(()) => SentState::Sent,
        Err(reason) => SentState::Failed(reason),
    };
    update_sent(&dir, id, state);
}

/// The live connection that reaches `device_id`: on TCP a connection is
/// keyed by its address, so it is matched through the phone id it
/// announced (`phone_id_of`). Only a connection with an address can carry
/// the download that follows.
fn target_for(
    device_id: &str,
    clients: &[Arc<dyn DeviceSender>],
    phone_id_of: impl Fn(&str) -> Option<String>,
) -> Option<Arc<dyn DeviceSender>> {
    clients
        .iter()
        .filter(|c| c.peer_addr().is_some())
        .find(|c| {
            c.device_id() == device_id || phone_id_of(c.device_id()).as_deref() == Some(device_id)
        })
        .cloned()
}

/// Daemon task: offer every file the desktop app asks to send.
pub async fn run_watcher() {
    let Ok(dir) = crate::state::state_dir() else {
        tracing::warn!("No state directory: sending files to phones is off");
        return;
    };
    loop {
        tokio::time::sleep(POLL).await;
        for request in outbox::take_all(&dir, now_unix()) {
            let clients = crate::state::clone_clients().await;
            let store = crate::state::trust_store_path()
                .ok()
                .and_then(|path| TrustStore::load_or_create(path).ok());
            deliver(
                &dir,
                request,
                &clients,
                store.as_ref(),
                crate::plugins::pair::phone_id_of,
            )
            .await;
        }
    }
}

/// Check one request, offer the file to its phone, and record it.
async fn deliver(
    dir: &Path,
    request: SendRequest,
    clients: &[Arc<dyn DeviceSender>],
    store: Option<&TrustStore>,
    phone_id_of: impl Fn(&str) -> Option<String>,
) {
    let path = PathBuf::from(&request.path);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| request.path.clone());
    let size = std::fs::metadata(&path)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| m.len());
    let outcome = offer_to_phone(&request, &path, &name, size, clients, store, phone_id_of).await;
    match &outcome {
        Ok(()) => tracing::info!("Offered {} to {}", path.display(), request.device_id),
        Err(reason) => tracing::warn!(
            "Not sending {} to {}: {reason}",
            path.display(),
            request.device_id
        ),
    }
    let file = SentFile {
        id: request.id,
        name,
        path: request.path,
        size: size.unwrap_or(0),
        device_id: request.device_id,
        sent_unix: now_unix(),
        state: match outcome {
            Ok(()) => SentState::Offered,
            Err(reason) => SentState::Failed(reason.to_string()),
        },
    };
    let _guard = sent_lock().lock().unwrap_or_else(|e| e.into_inner());
    if let Err(e) = sent::record(dir, file) {
        tracing::warn!("Could not record the sent file: {e}");
    }
}

async fn offer_to_phone(
    request: &SendRequest,
    path: &Path,
    name: &str,
    size: Option<u64>,
    clients: &[Arc<dyn DeviceSender>],
    store: Option<&TrustStore>,
    phone_id_of: impl Fn(&str) -> Option<String>,
) -> Result<(), &'static str> {
    let size = size.ok_or("not a file")?;
    if size > MAX_FILE_BYTES {
        return Err("the file is larger than 8 GB");
    }
    // An unreadable store could be hiding a denial: refuse.
    let allowed = store.is_some_and(|s| {
        s.is_trusted(&request.device_id)
            && s.permissions(&request.device_id).allows(Capability::Files)
    });
    if !allowed {
        return Err("files are off for this phone");
    }
    let target =
        target_for(&request.device_id, clients, phone_id_of).ok_or("phone not connected")?;
    offer_at(
        &request.id,
        path.to_path_buf(),
        &request.device_id,
        Instant::now(),
    );
    let packet = NetworkPacket::new(OFFER).with_body(serde_json::json!({
        "id": request.id,
        "name": name,
        "size": size,
    }));
    if target.send_packet(&packet).await.is_err() {
        offers()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&request.id);
        return Err("phone not connected");
    }
    Ok(())
}

/// `sent.json` is read, changed and replaced; one writer at a time.
fn sent_lock() -> &'static Mutex<()> {
    static LOCK: Mutex<()> = Mutex::new(());
    &LOCK
}

fn update_sent(dir: &Path, id: &str, state: SentState) {
    let _guard = sent_lock().lock().unwrap_or_else(|e| e.into_inner());
    if let Err(e) = sent::update(dir, id, state) {
        tracing::warn!("Could not update the sent list: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use linux_link_core::error::Result;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-offers-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_offer_is_taken_once_and_only_by_its_phone() {
        let now = Instant::now();
        offer_at("o-1", PathBuf::from("/a.pdf"), "phone-a", now);
        assert_eq!(
            redeem_at("o-1", "phone-b", now),
            Err("the file was offered to another phone")
        );
        assert_eq!(
            redeem_at("o-1", "phone-a", now),
            Ok(PathBuf::from("/a.pdf")),
            "another phone's attempt does not use it up"
        );
        assert_eq!(
            redeem_at("o-1", "phone-a", now),
            Err("the offer was used or is unknown")
        );
    }

    #[test]
    fn an_offer_expires() {
        let now = Instant::now();
        offer_at("o-2", PathBuf::from("/a.pdf"), "phone-a", now);
        assert_eq!(
            redeem_at("o-2", "phone-a", now + OFFER_TTL + Duration::from_secs(1)),
            Err("the offer expired")
        );
    }

    struct Conn {
        id: &'static str,
        tcp: bool,
        sent: Mutex<Vec<NetworkPacket>>,
    }

    impl Conn {
        fn new(id: &'static str, tcp: bool) -> Arc<Self> {
            Arc::new(Self {
                id,
                tcp,
                sent: Default::default(),
            })
        }
    }

    #[async_trait::async_trait]
    impl DeviceSender for Conn {
        fn device_id(&self) -> &str {
            self.id
        }
        fn connection_id(&self) -> &str {
            self.id
        }
        fn peer_addr(&self) -> Option<std::net::SocketAddr> {
            self.tcp.then(|| "192.0.2.7:51000".parse().unwrap())
        }
        async fn send_packet(&self, packet: &NetworkPacket) -> Result<()> {
            self.sent.lock().unwrap().push(packet.clone());
            Ok(())
        }
    }

    fn ids(address: &str) -> Option<String> {
        (address == "192.0.2.7").then(|| "phone-a".to_string())
    }

    #[test]
    fn a_phone_is_reached_through_the_address_it_connected_from() {
        let quic: Arc<dyn DeviceSender> = Conn::new("phone-a", false);
        let tcp: Arc<dyn DeviceSender> = Conn::new("192.0.2.7", true);
        let other: Arc<dyn DeviceSender> = Conn::new("192.0.2.9", true);
        let clients = [quic, other, tcp];
        let found = target_for("phone-a", &clients, ids).expect("found");
        assert_eq!(found.device_id(), "192.0.2.7");
        assert!(target_for("phone-b", &clients, ids).is_none());
    }

    fn trusting(dir: &Path, device: &str, files: bool) -> TrustStore {
        let mut store = TrustStore::load_or_create(dir.join("trust.json")).unwrap();
        store.trust_device(device).unwrap();
        let mut permissions = store.permissions(device);
        permissions.set(Capability::Files, files);
        store.set_permissions(device, permissions).unwrap();
        store
    }

    fn request(dir: &Path, path: &Path) -> SendRequest {
        outbox::request(dir, "phone-a", path.to_str().unwrap(), now_unix()).unwrap()
    }

    #[tokio::test]
    async fn a_file_is_offered_to_a_phone_in_a_session() {
        let dir = temp_dir("deliver");
        let file = dir.join("report.pdf");
        std::fs::write(&file, b"12345").unwrap();
        let store = trusting(&dir, "phone-a", true);
        let tcp = Conn::new("192.0.2.7", true);
        let clients: [Arc<dyn DeviceSender>; 1] = [tcp.clone()];
        let sent_request = request(&dir, &file);
        deliver(&dir, sent_request.clone(), &clients, Some(&store), ids).await;

        let pushed = tcp.sent.lock().unwrap().clone();
        assert_eq!(pushed.len(), 1);
        assert_eq!(pushed[0].packet_type, OFFER);
        assert_eq!(pushed[0].body["id"], sent_request.id);
        assert_eq!(pushed[0].body["name"], "report.pdf");
        assert_eq!(pushed[0].body["size"], 5);
        let listed = sent::read(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].state, SentState::Offered);
        assert_eq!(listed[0].size, 5);
        assert_eq!(
            redeem(&sent_request.id, "phone-a"),
            Ok(file.clone()),
            "the phone can now fetch it"
        );
    }

    #[tokio::test]
    async fn a_send_that_cannot_be_offered_is_recorded_as_failed() {
        let dir = temp_dir("refuse");
        let file = dir.join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        let tcp = Conn::new("192.0.2.7", true);
        let connected: [Arc<dyn DeviceSender>; 1] = [tcp.clone()];
        let allowed = trusting(&dir, "phone-a", true);
        for (path, clients, store, why) in [
            (file.clone(), &[][..], Some(&allowed), "phone not connected"),
            (dir.clone(), &connected[..], Some(&allowed), "not a file"),
            (
                dir.join("gone"),
                &connected[..],
                Some(&allowed),
                "not a file",
            ),
            (
                file.clone(),
                &connected[..],
                None,
                "files are off for this phone",
            ),
        ] {
            let _ = std::fs::remove_file(dir.join("sent.json"));
            deliver(&dir, request(&dir, &path), clients, store, ids).await;
            assert_eq!(
                sent::read(&dir)[0].state,
                SentState::Failed(why.to_string()),
                "{}",
                path.display()
            );
        }
        let denied = trusting(&temp_dir("refuse-denied"), "phone-a", false);
        let _ = std::fs::remove_file(dir.join("sent.json"));
        deliver(&dir, request(&dir, &file), &connected, Some(&denied), ids).await;
        assert_eq!(
            sent::read(&dir)[0].state,
            SentState::Failed("files are off for this phone".to_string())
        );
        assert!(tcp.sent.lock().unwrap().is_empty(), "nothing was offered");
    }
}
