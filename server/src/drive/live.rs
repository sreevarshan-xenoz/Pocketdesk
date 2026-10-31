//! The drive over real phones (D8): listings and fetches through T10g's
//! packets on each phone's live link, the fetches waiting here for their
//! shares, and the listener that follows `phone_drive`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use pocketdesk_core::local::drive::{self as store, DriveInfo};
use pocketdesk_core::local::paths::DRIVE_CACHE_DIR;
use pocketdesk_core::local::paths::DRIVE_UPLOADS_DIR;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, TrustStore};
use pocketdesk_core::protocol::permissions::Capability;
use pocketdesk_core::protocol::phone_files::{
    FETCH, LIST, Listing, MKDIR, PUT, mkdir_body, put_body, request_body,
};
use tokio::sync::oneshot;

use super::{Drive, Fail, Phones, Put};

/// How long a listing may take: the phone answers on its next look.
pub const LIST_WAIT: Duration = Duration::from_secs(15);

/// How long a file may take to arrive whole.
pub const FETCH_WAIT: Duration = Duration::from_secs(10 * 60);

/// How a phone's refusal of a copied file reads (N6): not room for it, in
/// the words the phone says it with, or another reason.
fn refusal(why: String) -> Put {
    if why.contains(pocketdesk_core::protocol::phone_files::NO_ROOM) {
        Put::NoRoom(why)
    } else {
        Put::Refused(why)
    }
}

/// Say in a desktop notification that `name` did not reach the phone (N6).
fn late(name: &str, why: &str) {
    let body = format!("{name}: {why}");
    tokio::task::spawn_blocking(move || {
        crate::platform::notify(
            "A file did not reach the phone",
            &body,
            Duration::from_secs(10),
        )
    });
}

/// A waiting fetch's phone, and where to say its file arrived.
type Waiter = (String, oneshot::Sender<PathBuf>);

/// Fetches waiting for their share, by id.
static WAITING: LazyLock<Mutex<HashMap<String, Waiter>>> = LazyLock::new(Default::default);

/// Listings as phones send them, to whoever waits for one.
static LISTINGS: LazyLock<tokio::sync::broadcast::Sender<(String, Listing)>> =
    LazyLock::new(|| tokio::sync::broadcast::channel(64).0);

/// A phone's listing came (the T10g plugin's side).
pub(crate) fn listing_came(device: &str, listing: &Listing) {
    let _ = LISTINGS.send((device.to_string(), listing.clone()));
}

/// Where a share that says `drive: id` from `device` goes: the drive's
/// cache when that fetch waits for it, else nowhere (the share is refused).
pub(crate) fn waiting_dir(device: &str, id: &str) -> Option<PathBuf> {
    let waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    let (from, _) = waiting.get(id)?;
    if from != device {
        return None;
    }
    let dir = crate::state::state_dir().ok()?.join(DRIVE_CACHE_DIR);
    pocketdesk_core::secure_fs::create_private_dir(&dir).ok()?;
    Some(dir)
}

/// The share for fetch `id` is whole at `path`: it goes to its waiter, or,
/// with none waiting any more, away.
pub(crate) fn arrived(id: &str, path: &std::path::Path) {
    let waiter = WAITING.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    match waiter {
        Some((_, tell)) => {
            if let Err(path) = tell.send(path.to_path_buf()) {
                let _ = std::fs::remove_file(path);
            }
        }
        None => {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The share for fetch `id` was refused (D8 review): its request ends now,
/// not at its wait's end.
pub(crate) fn failed(id: &str) {
    WAITING.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
}

/// A waiting fetch, forgotten when its request ends however it ends.
struct Waiting(String);

impl Drop for Waiting {
    fn drop(&mut self) {
        WAITING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// The phones the server has.
pub struct LivePhones;

impl LivePhones {
    async fn phone(&self, device: &str) -> Result<Arc<dyn DeviceSender>, Fail> {
        if !crate::plugins::pair::allows(device, Capability::Files) {
            return Err(Fail::Unreachable);
        }
        crate::state::clone_clients()
            .await
            .into_iter()
            .find(|c| c.device_id() == device)
            .ok_or(Fail::Unreachable)
    }

    /// Send `packet_type` with `body` and wait for the listing it is
    /// answered with: of `path` in `root`, or with no root, any.
    async fn ask(
        &self,
        device: &str,
        phone: Arc<dyn DeviceSender>,
        packet_type: &str,
        body: serde_json::Value,
        root: Option<u32>,
        path: &[String],
    ) -> Result<Listing, Fail> {
        // Listening before asking, so the answer cannot be missed.
        let mut listings = LISTINGS.subscribe();
        phone
            .send_packet(&NetworkPacket::new(packet_type).with_body(body))
            .await
            .map_err(|_| Fail::Unreachable)?;
        let wait = async {
            loop {
                let Ok((from, listing)) = listings.recv().await else {
                    continue;
                };
                if from != device {
                    continue;
                }
                let Some(asked) = root else {
                    // Only its roots were wanted: any listing has them.
                    return Ok(listing);
                };
                if listing.root == Some(asked) && listing.path == path {
                    return Ok(listing);
                }
                // The phone answers with its first folder when the one
                // asked is shared no more.
                if !listing.roots.iter().any(|r| r.id == asked) {
                    return Err(Fail::NotFound);
                }
            }
        };
        tokio::time::timeout(LIST_WAIT, wait)
            .await
            .unwrap_or(Err(Fail::Timeout))
    }
}

#[async_trait::async_trait]
impl Phones for LivePhones {
    fn phones(&self) -> Vec<(String, String)> {
        let Some(store) = crate::state::trust_store_path()
            .ok()
            .and_then(|path| TrustStore::load_or_create(path).ok())
        else {
            return Vec::new();
        };
        store
            .paired_devices()
            .into_iter()
            .filter(|id| !store.is_computer(id) && store.permissions(id).allows(Capability::Files))
            .map(|id| {
                let name = store.name(&id).unwrap_or_default().to_string();
                (id, name)
            })
            .collect()
    }

    fn tell(&self, name: &str, why: &str) {
        late(name, why);
    }

    async fn mkdir(&self, device: &str, root: u32, path: &[String]) -> Result<Listing, Fail> {
        let phone = self.phone(device).await?;
        let body = mkdir_body(root, path).ok_or(Fail::NotFound)?;
        // Answered with the new folder's listing, or why not.
        self.ask(device, phone, MKDIR, body, Some(root), path).await
    }

    async fn put(
        &self,
        device: &str,
        root: u32,
        folder: &[String],
        name: &str,
        file: PathBuf,
        size: u64,
    ) -> Result<Put, Fail> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let (phone, body) = match (
            self.phone(device).await,
            put_body(root, folder, name, &id, size),
        ) {
            (Ok(phone), Some(body)) => (phone, body),
            (fail, _) => {
                super::remove_upload(&file);
                return Err(fail.err().unwrap_or(Fail::NotFound));
            }
        };
        // In the sent list as any send to the phone, named where it goes
        // (N6 review).
        if let Ok(dir) = crate::state::state_dir() {
            let mut shown = folder.to_vec();
            shown.push(name.to_string());
            let sent = pocketdesk_core::local::sent::SentFile {
                id: id.clone(),
                name: name.to_string(),
                path: format!("the phone's drive: {}", shown.join("/")),
                size,
                device_id: device.to_string(),
                sent_unix: pocketdesk_core::local::pin::now_unix(),
                state: pocketdesk_core::local::sent::SentState::Offered,
                note: None,
            };
            // Off the runtime: on Windows a replace can wait up to about
            // half a second for another program's handle on the file.
            let _ = tokio::task::spawn_blocking(move || {
                pocketdesk_core::local::sent::record(&dir, sent)
            })
            .await;
        }
        // Offered first, so the phone's fetch finds it; the file goes when
        // the offer ends, however it ends.
        let ended = crate::offers::watch(&id);
        crate::offers::offer_at(&id, file.clone(), device, std::time::Instant::now());
        let (said, heard) = oneshot::channel();
        let shown = name.to_string();
        tokio::spawn(async move {
            let outcome = ended
                .await
                .unwrap_or_else(|_| Err("it was not given".to_string()));
            super::remove_upload(&file);
            // No one waits any more (answered as under way): the owner
            // hears a failure in a notification.
            if let Err(outcome) = said.send(outcome)
                && let Err(why) = outcome
            {
                late(&shown, &why);
            }
        });
        if phone
            .send_packet(&NetworkPacket::new(PUT).with_body(body))
            .await
            .is_err()
        {
            crate::offers::fail(&id, "phone not connected".to_string());
            return Err(Fail::Unreachable);
        }
        Ok(match tokio::time::timeout(super::PUT_WAIT, heard).await {
            Ok(Ok(Ok(()))) => Put::Saved,
            Ok(Ok(Err(why))) => refusal(why),
            Ok(Err(_)) => return Err(Fail::Phone),
            Err(_) => Put::UnderWay,
        })
    }

    async fn list(
        &self,
        device: &str,
        root: Option<u32>,
        path: &[String],
    ) -> Result<Listing, Fail> {
        let phone = self.phone(device).await?;
        let body = request_body(root.unwrap_or(0), path).ok_or(Fail::NotFound)?;
        self.ask(device, phone, LIST, body, root, path).await
    }

    async fn fetch(&self, device: &str, root: u32, path: &[String]) -> Result<PathBuf, Fail> {
        let phone = self.phone(device).await?;
        let (_, parent) = path.split_last().ok_or(Fail::NotFound)?;
        let mut body = request_body(root, path).ok_or(Fail::NotFound)?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        body["drive"] = serde_json::json!(id);
        let (tell, arrives) = oneshot::channel();
        WAITING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), (device.to_string(), tell));
        let _waiting = Waiting(id);
        // A phone that cannot send it answers with its folder and why.
        let mut listings = LISTINGS.subscribe();
        phone
            .send_packet(&NetworkPacket::new(FETCH).with_body(body))
            .await
            .map_err(|_| Fail::Unreachable)?;
        let refused = async {
            loop {
                if let Ok((from, listing)) = listings.recv().await
                    && from == device
                    && listing.error.is_some()
                    // Its folder's listing with why; or, for a folder shared
                    // no more, the first one's (D8 review).
                    && ((listing.root == Some(root) && listing.path == parent)
                        || !listing.roots.iter().any(|r| r.id == root))
                {
                    return;
                }
            }
        };
        tokio::select! {
            arrived = tokio::time::timeout(FETCH_WAIT, arrives) => match arrived {
                Ok(Ok(file)) => Ok(file),
                // Refused here (its waiter dropped): the phone could not
                // give it.
                Ok(Err(_)) => Err(Fail::Phone),
                Err(_) => Err(Fail::Timeout),
            },
            _ = refused => Err(Fail::Phone),
        }
    }
}

/// How often the setting is read.
const TICK: Duration = Duration::from_secs(2);

/// Serve the drive while `phone_drive` is on, for the server's life: the
/// address in `drive.json` while it serves, gone when it stops.
pub async fn run() {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    // What an earlier run left: its address and its files.
    let _ = store::clear(&dir);
    let _ = std::fs::remove_dir_all(dir.join(DRIVE_CACHE_DIR));
    let _ = std::fs::remove_dir_all(dir.join(DRIVE_UPLOADS_DIR));
    let mut serving: Option<(tokio_util::sync::CancellationToken, Arc<Drive>)> = None;
    let mut said = false;
    loop {
        // A config that does not read now (written meanwhile) keeps what
        // was: a new address would break a drive the owner mapped (D8
        // review).
        let (on, write) = match pocketdesk_core::local::config::Config::load() {
            Ok(config) => (config.phone_drive, config.phone_drive_write),
            Err(_) => (
                serving.is_some(),
                serving.as_ref().is_some_and(|(_, d)| d.writes()),
            ),
        };
        // Followed live (N6).
        if let Some((_, drive)) = &serving {
            drive.set_write(write);
        }
        match (on, serving.is_some()) {
            (true, false) => match tokio::net::TcpListener::bind("127.0.0.1:0").await {
                Ok(listener) => {
                    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
                    let info = DriveInfo::new(port);
                    let stop = tokio_util::sync::CancellationToken::new();
                    let drive = Arc::new(
                        Drive::new(info.token.clone(), Arc::new(LivePhones))
                            .taking_files_into(dir.join(DRIVE_UPLOADS_DIR)),
                    );
                    drive.set_write(write);
                    tokio::spawn(super::serve(listener, drive.clone(), stop.clone()));
                    match store::write(&dir, &info) {
                        Ok(()) => {
                            tracing::info!("Serving the phones' folders as a drive on port {port}");
                            serving = Some((stop, drive));
                            said = false;
                        }
                        Err(e) => {
                            tracing::warn!("The drive's address was not kept: {e}");
                            stop.cancel();
                        }
                    }
                }
                // Said once, not every tick (D8 review).
                Err(e) => {
                    if !std::mem::replace(&mut said, true) {
                        tracing::warn!("The drive could not listen: {e}");
                    }
                }
            },
            (false, true) => {
                if let Some((stop, _)) = serving.take() {
                    stop.cancel();
                }
                let _ = store::clear(&dir);
                // Files kept for a re-read go with it.
                let _ = std::fs::remove_dir_all(dir.join(DRIVE_CACHE_DIR));
                tracing::info!("Stopped serving the phones' folders as a drive");
            }
            _ => {}
        }
        tokio::time::sleep(TICK).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D8: a share naming a drive fetch is taken only when that fetch waits
    /// for that phone; a file arriving for no one is removed.
    #[test]
    fn a_drive_share_is_taken_only_for_its_waiting_phone() {
        let id = "feedfacefeedfacefeedfacefeedface";
        assert!(waiting_dir("phone-a", id).is_none(), "nothing waits");
        let (tell, mut arrives) = oneshot::channel();
        WAITING
            .lock()
            .unwrap()
            .insert(id.to_string(), ("phone-a".to_string(), tell));
        let waiting = Waiting(id.to_string());
        assert!(waiting_dir("phone-b", id).is_none(), "another phone's");
        let dir = std::env::temp_dir().join(format!("pd-drive-arrived-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("f");
        std::fs::write(&file, b"x").unwrap();
        arrived(id, &file);
        assert_eq!(arrives.try_recv().unwrap(), file, "handed to its waiter");
        drop(waiting);
        // Arrived after its request ended: removed.
        arrived(id, &file);
        assert!(!file.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// D8 review: a refused share ends its fetch's wait at once.
    #[test]
    fn a_refused_share_ends_its_fetch() {
        let id = "0badc0de0badc0de0badc0de0badc0de";
        let (tell, mut arrives) = oneshot::channel();
        WAITING
            .lock()
            .unwrap()
            .insert(id.to_string(), ("phone-a".to_string(), tell));
        failed(id);
        assert!(matches!(
            arrives.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        ));
        assert!(waiting_dir("phone-a", id).is_none());
    }
}
