//! Files the desktop's owner sends to a phone (roadmap F7b).
//!
//! The desktop app writes a request to the outbox (`core::local::outbox`).
//! The watcher here takes it, checks the file and the phone, and pushes the
//! phone a `file_offer` `{id, name, size}`. The phone then fetches the file
//! by that id (`share::FETCH`), so the desktop still dials the phone and
//! the phone can name nothing but what the owner chose. An offer is single
//! use, expires after [`OFFER_TTL`], and is valid only from its phone (as
//! far as the phone's in-band device id can be trusted: see the F5 limit).
//! Each send and its outcome go to `sent.json` for the app's Files tab, and
//! every send reaches an end there: sent, refused, declined by the phone,
//! or swept when its time runs out.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use pocketdesk_core::local::outbox::{self, SendRequest};
use pocketdesk_core::local::pin::now_unix;
use pocketdesk_core::local::sent::{self, SentFile, SentState};
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, TrustStore};
use pocketdesk_core::protocol::permissions::Capability;

/// Pushed to the phone: a file is waiting for it, `{id, name, size}`.
pub const OFFER: &str = "kdeconnect.pocketdesk.file_offer";
/// The phone declines an offer it will not fetch: `{id, reason}` (no room,
/// too large, nowhere to save it).
pub const DECLINE: &str = "kdeconnect.pocketdesk.file_offer_declined";
/// How long the phone has to fetch an offered file. Long enough for a phone
/// that fetches a dropped batch one file after another.
pub const OFFER_TTL: Duration = Duration::from_secs(10 * 60);
/// What the sent list says of an offer whose time ran out.
const EXPIRED: &str = "the phone did not take it in time";
/// The largest file offered, as for every transfer.
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// How often the outbox is read.
const POLL: Duration = Duration::from_millis(250);
/// How many times a phone may try to fetch an offer (T6a): a failed
/// attempt leaves it for the phone's retry, which continues the file.
pub const MAX_ATTEMPTS: u8 = 4;

/// A file offered to one phone.
#[derive(Debug, Clone)]
struct Offer {
    path: PathBuf,
    device_id: String,
    expires: Instant,
    /// Fetches begun. One runs at a time: a retry replaces an attempt
    /// still running (`share::running`).
    attempts: u8,
}

/// Offers by id. The server has one ([`offers`]); a test that sweeps uses
/// its own, so a sweep at a far-future instant cannot take another test's.
type Table = Mutex<HashMap<String, Offer>>;

fn offers() -> &'static Table {
    static OFFERS: OnceLock<Table> = OnceLock::new();
    OFFERS.get_or_init(Default::default)
}

/// Offer `path` to `device_id` under `id` (the outbox request's id).
pub(crate) fn offer_at(id: &str, path: PathBuf, device_id: &str, now: Instant) {
    offer_in(offers(), id, path, device_id, now);
}

fn offer_in(table: &Table, id: &str, path: PathBuf, device_id: &str, now: Instant) {
    let mut offers = table.lock().unwrap_or_else(|e| e.into_inner());
    offers.insert(
        id.to_string(),
        Offer {
            path,
            device_id: device_id.to_string(),
            expires: now + OFFER_TTL,
            attempts: 0,
        },
    );
}

/// The file offered as `id`, for a fetch by `device_id`. The offer stays
/// until a fetch settles it ([`finish`]); an offer for another phone is
/// refused and left for its own phone.
pub fn redeem(id: &str, device_id: &str) -> Result<PathBuf, &'static str> {
    let redeemed = redeem_at(id, device_id, Instant::now());
    // An offer this ends records why, so the sent list never waits on it.
    match redeemed {
        Err(OFFER_EXPIRED) => finish(id, Err(EXPIRED.to_string())),
        Err(TOO_MANY) => finish(id, Err(TOO_MANY.to_string())),
        _ => {}
    }
    redeemed
}

const OFFER_EXPIRED: &str = "the offer expired";
const TOO_MANY: &str = "the phone tried too many times";

fn redeem_at(id: &str, device_id: &str, now: Instant) -> Result<PathBuf, &'static str> {
    redeem_in(offers(), id, device_id, now)
}

fn redeem_in(
    table: &Table,
    id: &str,
    device_id: &str,
    now: Instant,
) -> Result<PathBuf, &'static str> {
    let mut offers = table.lock().unwrap_or_else(|e| e.into_inner());
    let offer = offers
        .get_mut(id)
        .ok_or("the offer was used or is unknown")?;
    if offer.device_id != device_id {
        return Err("the file was offered to another phone");
    }
    if offer.expires <= now {
        offers.remove(id);
        return Err(OFFER_EXPIRED);
    }
    if offer.attempts >= MAX_ATTEMPTS {
        offers.remove(id);
        return Err(TOO_MANY);
    }
    offer.attempts += 1;
    Ok(offer.path.clone())
}

/// How a fetch of an offer settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Settled {
    /// Sent: the offer is gone.
    Done,
    /// Failed, and the phone may try again.
    Retry,
    /// Failed for the last time: the offer is gone.
    GaveUp,
}

fn settle_in(table: &Table, id: &str, sent: bool) -> Settled {
    let mut offers = table.lock().unwrap_or_else(|e| e.into_inner());
    let Some(offer) = offers.get_mut(id) else {
        return if sent { Settled::Done } else { Settled::GaveUp };
    };
    if sent {
        offers.remove(id);
        Settled::Done
    } else if offer.attempts >= MAX_ATTEMPTS {
        offers.remove(id);
        Settled::GaveUp
    } else {
        Settled::Retry
    }
}

fn remove_in(table: &Table, id: &str, device_id: &str) -> bool {
    let mut offers = table.lock().unwrap_or_else(|e| e.into_inner());
    if offers.get(id).is_some_and(|o| o.device_id == device_id) {
        offers.remove(id);
        true
    } else {
        false
    }
}

/// Take every offer whose time is up at `now`; their ids.
fn sweep_at(now: Instant) -> Vec<String> {
    sweep_in(offers(), now)
}

fn sweep_in(table: &Table, now: Instant) -> Vec<String> {
    let mut offers = table.lock().unwrap_or_else(|e| e.into_inner());
    let expired: Vec<String> = offers
        .iter()
        .filter(|(_, o)| o.expires <= now)
        .map(|(id, _)| id.clone())
        .collect();
    for id in &expired {
        offers.remove(id);
    }
    expired
}

/// The phone `device_id` will not fetch offer `id`, and says why. Only the
/// phone it was offered to can decline it.
pub fn decline(id: &str, device_id: &str, reason: &str) {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    decline_in(&dir, id, device_id, reason, Instant::now());
}

fn decline_in(dir: &Path, id: &str, device_id: &str, reason: &str, _now: Instant) {
    if !remove_in(offers(), id, device_id) {
        return;
    }
    // The phone's words go into the desktop app: short, and one line.
    let reason: String = reason
        .chars()
        .filter(|c| !c.is_control())
        .take(120)
        .collect();
    let reason = if reason.trim().is_empty() {
        "the phone declined it".to_string()
    } else {
        format!("the phone declined it: {}", reason.trim())
    };
    tracing::info!("Offer {id} declined by {device_id}: {reason}");
    update_sent(dir, id, SentState::Failed(reason));
}

/// Record how a fetch of offer `id` ended. A failure leaves the offer for
/// the phone's retry until its attempts run out.
pub fn finish(id: &str, outcome: Result<(), String>) {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    match settle_in(offers(), id, outcome.is_ok()) {
        Settled::Done | Settled::GaveUp => finish_in(&dir, id, outcome),
        Settled::Retry => {
            if let Err(reason) = outcome {
                tracing::info!("Offer {id} failed this time ({reason}); the phone may try again");
            }
        }
    }
}

/// End offer `id` with `reason`: nothing a retry could change (the file is
/// gone, or too large, or the phone may not take it).
pub fn fail(id: &str, reason: String) {
    offers()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(id);
    if let Ok(dir) = crate::state::state_dir() {
        finish_in(&dir, id, Err(reason));
    }
}

fn finish_in(dir: &Path, id: &str, outcome: Result<(), String>) {
    let state = match outcome {
        Ok(()) => SentState::Sent,
        Err(reason) => SentState::Failed(reason),
    };
    update_sent(dir, id, state);
}

/// The live connection that reaches `device_id` (a connection goes by its
/// device's id, phase B). Only a connection with an address can carry the
/// download that follows.
fn target_for(device_id: &str, clients: &[Arc<dyn DeviceSender>]) -> Option<Arc<dyn DeviceSender>> {
    clients
        .iter()
        .filter(|c| c.peer_addr().is_some())
        .find(|c| c.device_id() == device_id)
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
        for id in sweep_at(Instant::now()) {
            tracing::info!("Offer {id} expired before the phone took it");
            finish_in(&dir, &id, Err(EXPIRED.to_string()));
        }
        for request in outbox::take_all(&dir, now_unix()) {
            let clients = crate::state::clone_clients().await;
            let store = crate::state::trust_store_path()
                .ok()
                .and_then(|path| TrustStore::load_or_create(path).ok());
            deliver(&dir, request, &clients, store.as_ref()).await;
        }
    }
}

/// Check one request, offer the file to its phone, and record it. The
/// record is written before the offer goes out, so a phone that fetches at
/// once cannot finish a send the list does not hold yet.
async fn deliver(
    dir: &Path,
    request: SendRequest,
    clients: &[Arc<dyn DeviceSender>],
    store: Option<&TrustStore>,
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
    let checked = check(&request, size, clients, store);
    let file = SentFile {
        id: request.id.clone(),
        name: name.clone(),
        path: request.path.clone(),
        size: size.unwrap_or(0),
        device_id: request.device_id.clone(),
        sent_unix: now_unix(),
        state: match &checked {
            Ok(_) => SentState::Offered,
            Err(reason) => SentState::Failed(reason.to_string()),
        },
    };
    {
        let _guard = sent_lock().lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = sent::record(dir, file) {
            tracing::warn!("Could not record the sent file: {e}");
        }
    }
    let (target, size) = match checked {
        Ok(found) => found,
        Err(reason) => {
            tracing::warn!(
                "Not sending {} to {}: {reason}",
                path.display(),
                request.device_id
            );
            return;
        }
    };
    offer_at(
        &request.id,
        path.clone(),
        &request.device_id,
        Instant::now(),
    );
    let packet = NetworkPacket::new(OFFER).with_body(serde_json::json!({
        "id": request.id,
        "name": name,
        "size": size,
        "source": source_of(&path, size),
    }));
    if target.send_packet(&packet).await.is_err() {
        offers()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&request.id);
        update_sent(
            dir,
            &request.id,
            SentState::Failed("phone not connected".to_string()),
        );
        return;
    }
    tracing::info!("Offered {} to {}", path.display(), request.device_id);
}

/// The file's source id (T6a): 32 hex digits naming this file at this
/// size and modified time, so the phone continues a download it began of
/// the same file and never one of another.
pub(crate) fn source_of(path: &Path, size: u64) -> String {
    let modified = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis());
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let digest = ring::digest::digest(
        &ring::digest::SHA256,
        format!(
            "{}
{size}
{modified}",
            canonical.display()
        )
        .as_bytes(),
    );
    digest.as_ref()[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Whether `request` may be offered: the file, the phone's permission, and
/// a live connection to it. The connection and the file's size.
fn check(
    request: &SendRequest,
    size: Option<u64>,
    clients: &[Arc<dyn DeviceSender>],
    store: Option<&TrustStore>,
) -> Result<(Arc<dyn DeviceSender>, u64), &'static str> {
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
    let target = target_for(&request.device_id, clients).ok_or("phone not connected")?;
    Ok((target, size))
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
    use pocketdesk_core::error::Result;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-offers-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_offer_is_fetched_by_its_phone_until_one_attempt_succeeds() {
        let table = Table::default();
        let now = Instant::now();
        offer_in(&table, "o-1", PathBuf::from("/a.pdf"), "phone-a", now);
        assert_eq!(
            redeem_in(&table, "o-1", "phone-b", now),
            Err("the file was offered to another phone")
        );
        assert_eq!(
            redeem_in(&table, "o-1", "phone-a", now),
            Ok(PathBuf::from("/a.pdf")),
            "another phone's attempt does not use it up"
        );
        assert_eq!(settle_in(&table, "o-1", false), Settled::Retry);
        assert!(
            redeem_in(&table, "o-1", "phone-a", now).is_ok(),
            "a failed attempt leaves it"
        );
        assert_eq!(settle_in(&table, "o-1", true), Settled::Done);
        assert_eq!(
            redeem_in(&table, "o-1", "phone-a", now),
            Err("the offer was used or is unknown"),
            "a sent file's offer is gone"
        );
    }

    #[test]
    fn a_source_id_names_the_file_as_it_is() {
        let dir = temp_dir("source");
        let path = dir.join("a.bin");
        std::fs::write(&path, b"abc").unwrap();
        let id = source_of(&path, 3);
        assert!(pocketdesk_core::protocol::partials::valid_source(&id));
        assert_eq!(id, source_of(&path, 3));
        assert_ne!(id, source_of(&path, 4), "another size");
        assert_ne!(id, source_of(&dir.join("b.bin"), 3), "another file");
    }

    #[test]
    fn an_offer_gives_up_after_four_failed_attempts() {
        let table = Table::default();
        let now = Instant::now();
        offer_in(&table, "o-4", PathBuf::from("/a.pdf"), "phone-a", now);
        for attempt in 1..=MAX_ATTEMPTS {
            assert!(
                redeem_in(&table, "o-4", "phone-a", now).is_ok(),
                "{attempt}"
            );
            let settled = settle_in(&table, "o-4", false);
            if attempt < MAX_ATTEMPTS {
                assert_eq!(settled, Settled::Retry);
            } else {
                assert_eq!(settled, Settled::GaveUp);
            }
        }
        assert!(redeem_in(&table, "o-4", "phone-a", now).is_err());
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

    #[test]
    fn an_offer_whose_time_is_up_is_swept() {
        // Its own table: a sweep this far ahead would take any offer the
        // tests beside it just made in the server's.
        let table = Table::default();
        let now = Instant::now();
        offer_in(&table, "o-sweep", PathBuf::from("/a.pdf"), "phone-a", now);
        assert!(sweep_in(&table, now).is_empty());
        let later = now + OFFER_TTL + Duration::from_secs(1);
        assert_eq!(sweep_in(&table, later), ["o-sweep"]);
        assert_eq!(
            redeem_in(&table, "o-sweep", "phone-a", later),
            Err("the offer was used or is unknown")
        );
    }

    #[test]
    fn only_its_phone_can_decline_an_offer_and_the_reason_is_recorded() {
        let dir = temp_dir("decline");
        let now = Instant::now();
        let file = |id: &str| SentFile {
            id: id.to_string(),
            name: "a.pdf".to_string(),
            path: "/a.pdf".to_string(),
            size: 1,
            device_id: "phone-a".to_string(),
            sent_unix: 1,
            state: SentState::Offered,
        };
        sent::record(&dir, file("o-decline")).unwrap();
        offer_at("o-decline", PathBuf::from("/a.pdf"), "phone-a", now);
        decline_in(&dir, "o-decline", "phone-b", "no", now);
        assert_eq!(
            sent::read(&dir)[0].state,
            SentState::Offered,
            "not its phone"
        );
        decline_in(
            &dir,
            "o-decline",
            "phone-a",
            "Not enough space\nfor a.pdf",
            now,
        );
        assert_eq!(
            sent::read(&dir)[0].state,
            SentState::Failed("the phone declined it: Not enough spacefor a.pdf".to_string())
        );
        assert!(redeem_at("o-decline", "phone-a", now).is_err(), "used up");
    }

    struct Conn {
        id: &'static str,
        tcp: bool,
        sent: Mutex<Vec<NetworkPacket>>,
        /// The sent list's states when each packet went out.
        listed: Mutex<Vec<Vec<SentState>>>,
        dir: Mutex<Option<PathBuf>>,
    }

    impl Conn {
        fn new(id: &'static str, tcp: bool) -> Arc<Self> {
            Arc::new(Self {
                id,
                tcp,
                sent: Default::default(),
                listed: Default::default(),
                dir: Default::default(),
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
            if let Some(dir) = self.dir.lock().unwrap().as_ref() {
                let states = sent::read(dir).into_iter().map(|f| f.state).collect();
                self.listed.lock().unwrap().push(states);
            }
            Ok(())
        }
    }

    #[test]
    fn a_phone_is_reached_through_the_address_it_connected_from() {
        let quic: Arc<dyn DeviceSender> = Conn::new("phone-a", false);
        let tcp: Arc<dyn DeviceSender> = Conn::new("phone-a", true);
        let other: Arc<dyn DeviceSender> = Conn::new("phone-c", true);
        let clients = [quic, other, tcp];
        let found = target_for("phone-a", &clients).expect("found");
        assert_eq!(found.device_id(), "phone-a");
        assert!(target_for("phone-b", &clients).is_none());
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
        let tcp = Conn::new("phone-a", true);
        *tcp.dir.lock().unwrap() = Some(dir.clone());
        let clients: [Arc<dyn DeviceSender>; 1] = [tcp.clone()];
        let sent_request = request(&dir, &file);
        deliver(&dir, sent_request.clone(), &clients, Some(&store)).await;

        let pushed = tcp.sent.lock().unwrap().clone();
        assert_eq!(pushed.len(), 1);
        assert_eq!(pushed[0].packet_type, OFFER);
        assert_eq!(pushed[0].body["id"], sent_request.id);
        assert_eq!(pushed[0].body["name"], "report.pdf");
        assert_eq!(pushed[0].body["size"], 5);
        assert_eq!(
            tcp.listed.lock().unwrap()[0],
            [SentState::Offered],
            "listed before the phone could fetch it"
        );
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
        let tcp = Conn::new("phone-a", true);
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
            deliver(&dir, request(&dir, &path), clients, store).await;
            assert_eq!(
                sent::read(&dir)[0].state,
                SentState::Failed(why.to_string()),
                "{}",
                path.display()
            );
        }
        let denied = trusting(&temp_dir("refuse-denied"), "phone-a", false);
        let _ = std::fs::remove_file(dir.join("sent.json"));
        deliver(&dir, request(&dir, &file), &connected, Some(&denied)).await;
        assert_eq!(
            sent::read(&dir)[0].state,
            SentState::Failed("files are off for this phone".to_string())
        );
        assert!(tcp.sent.lock().unwrap().is_empty(), "nothing was offered");
    }
}
