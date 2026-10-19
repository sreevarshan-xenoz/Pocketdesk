//! The drive over real phones (D8): listings and fetches through T10g's
//! packets on each phone's live link, the fetches waiting here for their
//! shares, and the listener that follows `phone_drive`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use pocketdesk_core::local::drive::{self as store, DriveInfo};
use pocketdesk_core::local::paths::DRIVE_CACHE_DIR;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, TrustStore};
use pocketdesk_core::protocol::permissions::Capability;
use pocketdesk_core::protocol::phone_files::{FETCH, LIST, Listing, request_body};
use tokio::sync::oneshot;

use super::{Drive, Fail, Phones};

/// How long a listing may take: the phone answers on its next look.
pub const LIST_WAIT: Duration = Duration::from_secs(15);

/// How long a file may take to arrive whole.
pub const FETCH_WAIT: Duration = Duration::from_secs(10 * 60);

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

    async fn list(
        &self,
        device: &str,
        root: Option<u32>,
        path: &[String],
    ) -> Result<Listing, Fail> {
        let phone = self.phone(device).await?;
        let body = request_body(root.unwrap_or(0), path).ok_or(Fail::NotFound)?;
        // Listening before asking, so the answer cannot be missed.
        let mut listings = LISTINGS.subscribe();
        phone
            .send_packet(&NetworkPacket::new(LIST).with_body(body))
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
                    && listing.root == Some(root)
                    && listing.path == parent
                    && listing.error.is_some()
                {
                    return;
                }
            }
        };
        tokio::select! {
            arrived = tokio::time::timeout(FETCH_WAIT, arrives) => match arrived {
                Ok(Ok(file)) => Ok(file),
                _ => Err(Fail::Timeout),
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
    let mut serving: Option<tokio_util::sync::CancellationToken> = None;
    loop {
        let on = pocketdesk_core::local::config::Config::load()
            .map(|c| c.phone_drive)
            .unwrap_or(false);
        match (on, serving.is_some()) {
            (true, false) => match tokio::net::TcpListener::bind("127.0.0.1:0").await {
                Ok(listener) => {
                    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
                    let info = DriveInfo::new(port);
                    let stop = tokio_util::sync::CancellationToken::new();
                    let drive = Arc::new(Drive::new(info.token.clone(), Arc::new(LivePhones)));
                    tokio::spawn(super::serve(listener, drive, stop.clone()));
                    match store::write(&dir, &info) {
                        Ok(()) => {
                            tracing::info!("Serving the phones' folders as a drive on port {port}");
                            serving = Some(stop);
                        }
                        Err(e) => {
                            tracing::warn!("The drive's address was not kept: {e}");
                            stop.cancel();
                        }
                    }
                }
                Err(e) => tracing::warn!("The drive could not listen: {e}"),
            },
            (false, true) => {
                if let Some(stop) = serving.take() {
                    stop.cancel();
                }
                let _ = store::clear(&dir);
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
}
