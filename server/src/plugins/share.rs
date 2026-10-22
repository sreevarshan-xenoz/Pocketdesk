use pocketdesk_core::error::{PocketdeskError, Result};
use pocketdesk_core::protocol::kdeconnect::{
    DeviceSender, NetworkPacket, OpenLinkStream, Plugin, TransferIo,
};
use pocketdesk_core::protocol::partials::{Record, Store};
use pocketdesk_core::protocol::transfer::{self, Have, Pace, TransferError};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// The largest file a phone may send: a bound on disk use, not a policy.
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// How long to wait for the phone to take the data connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest file name kept, in bytes (Linux allows 255).
const MAX_NAME_BYTES: usize = 200;
/// Transfers running at once; more are refused, not queued.
const MAX_TRANSFERS: usize = 4;
/// Unfinished files kept per phone, and in all (T6a). The age limit is the
/// `transfer_keep_hours` setting.
const PARTIALS_PER_PHONE: usize = 4;
const PARTIALS_TOTAL_BYTES: u64 = 16 * 1024 * 1024 * 1024;

/// Hours an unfinished file is kept (`transfer_keep_hours`); 0 keeps none.
static KEEP_HOURS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(24);

/// Set how long unfinished files are kept, from the config at start.
pub fn configure(keep_hours: u32) {
    KEEP_HOURS.store(keep_hours, std::sync::atomic::Ordering::Relaxed);
}

fn partial_limits() -> pocketdesk_core::protocol::partials::Limits {
    let hours = KEEP_HOURS.load(std::sync::atomic::Ordering::Relaxed);
    pocketdesk_core::protocol::partials::Limits {
        max_age_secs: u64::from(hours) * 3600,
        per_device: PARTIALS_PER_PHONE,
        total_bytes: PARTIALS_TOTAL_BYTES,
    }
}

/// Where unfinished files from phones wait for a retry: in the state
/// folder (owner-only), never in Downloads, so no half file sits where the
/// person looks.
pub(crate) fn partials() -> Option<Store> {
    crate::state::state_dir()
        .ok()
        .map(|dir| Store::new(dir.join("partials")))
}

/// The partial files a receive is writing now, by file name. The sweep
/// leaves these alone: a transfer without a record yet (its first commit
/// not reached, or nothing kept) is still using its file.
fn active_parts() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static ACTIVE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    ACTIVE.get_or_init(Default::default)
}

/// Marks a partial file in use for as long as it lives.
struct InUse(String);

impl InUse {
    fn new(location: &str) -> Self {
        active_parts()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(location.to_string());
        InUse(location.to_string())
    }
}

impl Drop for InUse {
    fn drop(&mut self) {
        active_parts()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

fn in_use(location: &str) -> bool {
    active_parts()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(location)
}

/// Drop unfinished files past `limits`, with their bytes, and any bytes
/// whose record is gone; never one a receive is using now.
pub(crate) fn sweep_partials(store: &Store, limits: pocketdesk_core::protocol::partials::Limits) {
    let now = pocketdesk_core::local::pin::now_unix();
    for record in store.sweep(now, limits, |r| in_use(&r.location)) {
        let _ = std::fs::remove_file(store.dir().join(&record.location));
    }
    let kept: std::collections::HashSet<String> =
        store.all().into_iter().map(|r| r.location).collect();
    if let Ok(entries) = std::fs::read_dir(store.dir()) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".part") && !kept.contains(&name) && !in_use(&name) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// Sweep the unfinished files now and every hour.
pub fn spawn_partials_sweep() {
    tokio::spawn(async {
        loop {
            if let Some(store) = partials() {
                let _ =
                    tokio::task::spawn_blocking(move || sweep_partials(&store, partial_limits()))
                        .await;
            }
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
}

/// The phone asks for a file (F7b): `{path | offer, port, key}`. It listens
/// on `port`, and the desktop dials it, as for a share, so the bytes go to
/// the transport's peer and nowhere the phone names.
pub const FETCH: &str = "kdeconnect.pocketdesk.file_fetch";
/// A download the desktop will not make, and why: `{key, reason}`, so the
/// phone stops waiting at once instead of at its listener's timeout.
pub const FETCH_REFUSED: &str = "kdeconnect.pocketdesk.file_fetch_refused";
/// The phone asks for a whole folder from the remote browser: `{path}`.
/// It is offered as the owner's folder sends are (`offers`), and always
/// answered (`offers::FOLDER_FETCH_REPLY`).
pub const FOLDER_FETCH: &str = "kdeconnect.pocketdesk.folder_fetch";

/// Transfers running at once from one phone, so one phone cannot hold every
/// slot and starve the others (R3). Two, not one: a phone sends its next
/// queued file as soon as it reads the last one's status byte, which can be
/// before the desktop has given that slot back, and an image sync or a
/// download should not wait on a long share.
const MAX_TRANSFERS_PER_PHONE: usize = 2;

/// The transfer slots: so many for the desktop, and fewer for one phone.
pub(crate) struct TransferSlots {
    total: std::sync::Arc<tokio::sync::Semaphore>,
    per_phone: usize,
    running: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>>,
}

/// One running transfer's place, given back (to the desktop and its phone)
/// when it is dropped.
pub(crate) struct TransferSlot {
    _permit: tokio::sync::OwnedSemaphorePermit,
    phone: String,
    running: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>>,
}

impl TransferSlots {
    fn new(total: usize, per_phone: usize) -> Self {
        Self {
            total: std::sync::Arc::new(tokio::sync::Semaphore::new(total)),
            per_phone,
            running: Default::default(),
        }
    }

    /// A slot for a transfer with `phone`, or why there is none.
    pub(crate) fn take(&self, phone: &str) -> std::result::Result<TransferSlot, String> {
        let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if running.get(phone).copied().unwrap_or(0) >= self.per_phone {
            return Err("this phone's last transfer is still running".to_string());
        }
        let permit = self
            .total
            .clone()
            .try_acquire_owned()
            .map_err(|_| "the desktop is busy with other transfers".to_string())?;
        *running.entry(phone.to_string()).or_insert(0) += 1;
        Ok(TransferSlot {
            _permit: permit,
            phone: phone.to_string(),
            running: self.running.clone(),
        })
    }
}

impl Drop for TransferSlot {
    fn drop(&mut self) {
        let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = running.get_mut(&self.phone) {
            *n -= 1;
            if *n == 0 {
                running.remove(&self.phone);
            }
        }
    }
}

/// The transfer slots, shared by every connection.
pub(crate) fn transfer_slots() -> &'static TransferSlots {
    static SLOTS: std::sync::OnceLock<TransferSlots> = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| TransferSlots::new(MAX_TRANSFERS, MAX_TRANSFERS_PER_PHONE))
}

/// A share request's folder fields (T6b): the send's id, the top folder's
/// name and the file's path under it, as a list of names. `None` for a
/// plain file.
fn folder_of(body: &serde_json::Value) -> Option<(String, String, Vec<String>)> {
    let folder_id = body.get("folderId")?.as_str()?.to_string();
    let folder = body.get("folder")?.as_str()?.to_string();
    let path = body
        .get("path")?
        .as_array()?
        .iter()
        .map(|p| p.as_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()
        // A path that is not all names is refused by the checks, not
        // quietly treated as a plain file.
        .unwrap_or_default();
    Some((folder_id, folder, path))
}

/// How long the top folder of one folder send is remembered after its last
/// file: its files arrive one after another, and a retry may come minutes
/// later.
const FOLDER_SEND_TTL: Duration = Duration::from_secs(60 * 60);

/// The top folder each folder send (T6b) put its files under, by the phone
/// and the send's id, so every file of one send lands in one new folder
/// and two sends never mix.
#[derive(Default)]
pub(crate) struct FolderTops {
    tops: std::sync::Mutex<
        std::collections::HashMap<(String, String), (PathBuf, std::time::Instant)>,
    >,
}

impl FolderTops {
    /// Where a file of `device`'s folder send `folder_id` goes: the folder
    /// for its `path` (components under the top, the file's name last),
    /// made if needed, and the file's name. The top is a new folder in
    /// `downloads` named like `folder` (`Name`, `Name (1)`…), made by the
    /// send's first file. A path that could leave the top is refused.
    pub(crate) fn place(
        &self,
        downloads: &Path,
        device: &str,
        folder_id: &str,
        folder: &str,
        path: &[String],
    ) -> std::result::Result<(PathBuf, String), String> {
        self.place_at(
            downloads,
            device,
            folder_id,
            folder,
            path,
            std::time::Instant::now(),
        )
    }

    fn place_at(
        &self,
        downloads: &Path,
        device: &str,
        folder_id: &str,
        folder: &str,
        path: &[String],
        now: std::time::Instant,
    ) -> std::result::Result<(PathBuf, String), String> {
        if !is_transfer_key(folder_id) {
            return Err("no usable folder id".to_string());
        }
        if let Some(problem) = pocketdesk_core::protocol::relpath::component_problem(folder) {
            return Err(format!("the folder's name: {problem}"));
        }
        if let Some(problem) = pocketdesk_core::protocol::relpath::path_problem(path) {
            return Err(format!("the file's place in the folder: {problem}"));
        }
        let (name, within) = path.split_last().ok_or("no file name")?;
        let top = {
            let mut tops = self.tops.lock().unwrap_or_else(|e| e.into_inner());
            tops.retain(|_, (_, at)| now.saturating_duration_since(*at) < FOLDER_SEND_TTL);
            let key = (device.to_string(), folder_id.to_string());
            match tops.get_mut(&key) {
                // Each file keeps its send remembered a while longer.
                Some((top, at)) => {
                    *at = now;
                    top.clone()
                }
                None => {
                    let top = new_folder(downloads, folder)?;
                    tops.insert(key, (top.clone(), now));
                    top
                }
            }
        };
        let dir = within.iter().fold(top.clone(), |dir, part| dir.join(part));
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("could not make {}: {e}", dir.display()))?;
        // Each component was checked, but the folder on disk has the last
        // word (a link planted inside the top would show here).
        let real_top = std::fs::canonicalize(&top).map_err(|e| e.to_string())?;
        let real_dir = std::fs::canonicalize(&dir).map_err(|e| e.to_string())?;
        if !real_dir.starts_with(&real_top) {
            return Err("the file's folder leads out of the sent folder".to_string());
        }
        Ok((dir, name.clone()))
    }
}

impl FolderTops {
    /// The top folder `device`'s folder send `folder_id` is saving into.
    pub(crate) fn top(&self, device: &str, folder_id: &str) -> Option<PathBuf> {
        let tops = self.tops.lock().unwrap_or_else(|e| e.into_inner());
        tops.get(&(device.to_string(), folder_id.to_string()))
            .map(|(top, _)| top.clone())
    }
}

/// A folder in `parent` named like `name` that did not exist before:
/// `name`, else `name (1)`, `name (2)`… Made exclusively, so a send never
/// merges into a folder that is already there.
fn new_folder(parent: &Path, name: &str) -> std::result::Result<PathBuf, String> {
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    for n in 0..1000 {
        let candidate = if n == 0 {
            parent.join(name)
        } else {
            parent.join(format!("{name} ({n})"))
        };
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("could not make {}: {e}", candidate.display())),
        }
    }
    Err(format!("too many folders named like {name}"))
}

/// The folder sends in progress, shared by every connection.
pub(crate) fn folder_tops() -> &'static FolderTops {
    static TOPS: std::sync::OnceLock<FolderTops> = std::sync::OnceLock::new();
    TOPS.get_or_init(FolderTops::default)
}

/// The transfer running for each file, by a key naming it (T6a). A phone
/// whose Wi-Fi dropped retries before this side notices the old connection
/// is dead (no reset arrives, so it waits out its idle limit); its retry
/// replaces the old attempt, which would otherwise hold a transfer slot and
/// work on the same partial file.
#[derive(Default)]
pub(crate) struct Running {
    tasks: tokio::sync::Mutex<std::collections::HashMap<String, tokio::task::JoinHandle<()>>>,
}

impl Running {
    /// End the attempt running under `key`, if any, and wait until it is
    /// gone (its slot and files released).
    pub(crate) async fn supersede(&self, key: &str) {
        let old = self.tasks.lock().await.remove(key);
        if let Some(old) = old {
            tracing::info!("A retry replaces the transfer still running for {key}");
            old.abort();
            let _ = old.await;
        }
    }

    /// Run `work` under `key`; it forgets itself when done.
    pub(crate) async fn spawn(
        &'static self,
        key: String,
        work: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        let mut tasks = self.tasks.lock().await;
        let handle = tokio::spawn({
            let key = key.clone();
            async move {
                work.await;
                let mut tasks = self.tasks.lock().await;
                if tasks
                    .get(&key)
                    .is_some_and(|t| Some(t.id()) == tokio::task::try_id())
                {
                    tasks.remove(&key);
                }
            }
        });
        // Two requests for one file that both passed `supersede` (a
        // reconnect racing a retry): the newer one wins, and the older is
        // stopped, not left running unseen.
        if let Some(older) = tasks.insert(key, handle) {
            older.abort();
        }
    }

    #[cfg(test)]
    async fn is_running(&self, key: &str) -> bool {
        self.tasks.lock().await.contains_key(key)
    }
}

/// The running transfers, shared by every connection.
pub(crate) fn running() -> &'static Running {
    static RUNNING: std::sync::OnceLock<Running> = std::sync::OnceLock::new();
    RUNNING.get_or_init(Running::default)
}

#[derive(Debug)]
pub struct SharePlugin {
    /// A fixed folder for received files (tests); otherwise each file goes
    /// where [`download_dir`] says at that moment.
    fixed_dir: Option<PathBuf>,
}

impl Default for SharePlugin {
    fn default() -> Self {
        Self::new()
    }
}

/// Where received files go: Downloads, else home; or
/// `POCKETDESK_DOWNLOAD_DIR` (an absolute path), for tests that run two
/// servers on one machine (2h), which would share its real Downloads. The
/// server warns at start when it is set.
pub(crate) fn download_dir() -> Option<PathBuf> {
    if let Some(seam) = std::env::var_os("POCKETDESK_DOWNLOAD_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Some(seam);
    }
    // The owner's folder (C2), read at each file so a change applies at once;
    // a config that cannot be read now keeps the last one read (C2 review).
    static LAST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    let chosen = match pocketdesk_core::local::config::Config::load() {
        Ok(config) => {
            *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(config.downloads_folder.clone());
            config.downloads_folder
        }
        Err(e) => {
            warn_once(&format!(
                "The settings could not be read ({e}); received files go where they went last"
            ));
            LAST.lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
                .unwrap_or_default()
        }
    };
    chosen_folder(&chosen, &protected_dirs())
        .or_else(dirs::download_dir)
        .or_else(dirs::home_dir)
}

/// The owner's folder for received files when it can take them: set,
/// absolute and a folder. Anything else is Downloads, and the log says why.
fn chosen_folder(chosen: &str, protected: &[PathBuf]) -> Option<PathBuf> {
    let chosen = chosen.trim();
    if chosen.is_empty() {
        return None;
    }
    let path = PathBuf::from(chosen);
    if path.is_absolute() && path.is_dir() {
        // Never the server's own folders, nor inside them (C2 review): a
        // paired phone could plant a PIN file or a cast token there.
        let real = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let theirs = protected.iter().any(|dir| {
            let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.clone());
            real.starts_with(&dir)
        });
        if theirs {
            warn_once(&format!(
                "Received files go to Downloads: the folder chosen for them ({chosen}) is Pocketdesk's own"
            ));
            return None;
        }
        Some(path)
    } else {
        warn_once(&format!(
            "Received files go to Downloads: the folder chosen for them ({chosen}) is not there"
        ));
        None
    }
}

/// The server's own folders, which received files never go into.
fn protected_dirs() -> Vec<PathBuf> {
    [
        pocketdesk_core::local::paths::state_dir(),
        pocketdesk_core::local::paths::config_dir(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Log `message` unless it was the last one logged: a folder of a thousand
/// files says why once.
fn warn_once(message: &str) {
    static LAST: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if *last != message {
        tracing::warn!("{message}");
        *last = message.to_string();
    }
}

impl SharePlugin {
    pub fn new() -> Self {
        Self { fixed_dir: None }
    }

    #[allow(dead_code)]
    pub fn with_download_dir(dir: PathBuf) -> Self {
        Self {
            fixed_dir: Some(dir),
        }
    }
}

#[async_trait::async_trait]
impl Plugin for SharePlugin {
    fn name(&self) -> &'static str {
        "share"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[
            "kdeconnect.share.request",
            FETCH,
            FOLDER_FETCH,
            crate::offers::DECLINE,
        ]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[
            "kdeconnect.share.request",
            FETCH_REFUSED,
            crate::offers::OFFER,
            crate::offers::FOLDER_FETCH_REPLY,
        ]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type == FETCH {
            handle_fetch(packet, sender).await;
            return Ok(());
        }
        if packet.packet_type == FOLDER_FETCH {
            handle_folder_fetch(packet, sender).await;
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
                let name =
                    safe_file_name(body.get("filename").and_then(|v| v.as_str()).unwrap_or(""));
                let modified = received_date(body);
                // The phone hands the bytes only to whoever presents the key
                // it put in this request, so they reach this desktop and no
                // one else on the network. Without one there is nothing to
                // name in a refusal either.
                let Some(key) = transfer_info
                    .get("key")
                    .and_then(|v| v.as_str())
                    .filter(|k| is_transfer_key(k))
                    .map(str::to_string)
                else {
                    tracing::warn!("File share of {name} refused: no transfer key");
                    return Ok(());
                };
                // A file the drive asked for (D8) names its fetch.
                let drive = body
                    .get("drive")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                // Every refusal is answered, so the phone stops waiting for a
                // connection that will not come, and can say why; a drive
                // fetch's request is told at once too (D8 review).
                let refused = |reason: String| {
                    let name = name.clone();
                    let key = key.clone();
                    let drive = drive.clone();
                    async move {
                        if let Some(id) = &drive {
                            crate::drive::live::failed(id);
                        }
                        tracing::warn!("File share of {name} refused: {reason}");
                        refuse(sender, &key, &reason).await;
                        Ok(())
                    }
                };
                // A port to connect to at home; none asks for a stream on
                // the phone's own link, by the internet road (2c).
                let port = transfer_info
                    .get("port")
                    .and_then(|v| v.as_u64())
                    .and_then(|p| u16::try_from(p).ok())
                    .filter(|p| *p != 0);
                let reach = match reach(sender, port) {
                    Ok(reach) => reach,
                    Err(reason) => return refused(reason).await,
                };
                let size = packet.payload_size.unwrap_or(0);
                if size == 0 || size > MAX_FILE_BYTES {
                    return refused(format!(
                        "a declared size of {size} bytes (empty, or too large)"
                    ))
                    .await;
                }
                let Some(dir) = self.fixed_dir.clone().or_else(download_dir) else {
                    return refused("no downloads or home folder".into()).await;
                };
                // A file the drive asked for (D8) goes to its cache, for the
                // request waiting for it; one no request waits for, refused.
                let dir = match &drive {
                    None => dir,
                    Some(id) => match crate::drive::live::waiting_dir(sender.device_id(), id) {
                        Some(cache) => cache,
                        None => {
                            return refused("not a file this computer's drive asked for".into())
                                .await;
                        }
                    },
                };
                // At home the bytes travel inside TLS with the phone that
                // asked (phase C); on the link, iroh proved both keys.
                let route = match route(reach, sender) {
                    Ok(route) => route,
                    Err(reason) => return refused(reason).await,
                };
                let Some(store) = partials() else {
                    return refused("no state folder for the transfer".into()).await;
                };
                // The phone's id for the file, the same on every attempt: a
                // retry continues what an earlier attempt left (T6a).
                let source = body
                    .get("sourceId")
                    .and_then(|v| v.as_str())
                    .filter(|s| pocketdesk_core::protocol::partials::valid_source(s))
                    .map(str::to_string);
                let from = sender.device_id().to_string();
                // A retry of this file replaces an attempt still running for
                // it, before the slot is taken.
                let running_key = source
                    .as_ref()
                    .map(|s| format!("share:{from}:{s}"))
                    .unwrap_or_else(|| format!("share:{}", uuid::Uuid::new_v4().simple()));
                // The owner cancelled this file while it ran (C1): its retry
                // is told so, and stops.
                if crate::transfers::take_cancelled(&running_key) {
                    return refused(crate::transfers::CANCELLED_HERE.to_string()).await;
                }
                running().supersede(&running_key).await;
                let slot = match transfer_slots().take(sender.device_id()) {
                    Ok(slot) => slot,
                    Err(reason) => return refused(reason.to_string()).await,
                };
                // A file of a folder send (T6b) goes to its place under the
                // send's own new folder; its path was checked part by part.
                // Made only now, so a share refused above leaves no folder.
                let folder_name = folder_of(body).map(|(_, folder, _)| folder);
                let folder = folder_of(body).filter(|_| drive.is_none());
                let (dir, name, top) = match folder {
                    None => (dir, name.clone(), None),
                    Some((folder_id, folder, path)) => {
                        match folder_tops().place(
                            &dir,
                            sender.device_id(),
                            &folder_id,
                            &folder,
                            &path,
                        ) {
                            Ok((dir, name)) => {
                                let top = folder_tops().top(sender.device_id(), &folder_id);
                                (dir, name, top)
                            }
                            Err(reason) => return refused(reason).await,
                        }
                    }
                };
                let tracker = crate::transfers::track(crate::transfers::Shown {
                    key: running_key.clone(),
                    direction: pocketdesk_core::local::transfers::Direction::FromPhone,
                    device_id: from.clone(),
                    name: name.clone(),
                    folder: folder_name,
                    size,
                });
                running()
                    .spawn(running_key, async move {
                        let progress = Box::new(tracker.progress());
                        let _tracker = tracker;
                        match receive_file(
                            route,
                            &key,
                            Incoming {
                                dir: &dir,
                                name: &name,
                                size,
                                device: &from,
                                source: source.as_deref(),
                                store: &store,
                                limits: partial_limits(),
                                progress,
                                on_saved: {
                                    let from = from.clone();
                                    Box::new(move |path: &Path| match &drive {
                                        // Not received as a share is: no
                                        // notification, list or event.
                                        Some(id) => crate::drive::live::arrived(id, path),
                                        None => on_received(path, size, &from, top.as_deref()),
                                    })
                                },
                                modified,
                            },
                        )
                        .await
                        {
                            Ok(_) => {
                                // Placed and recorded (receive_file does it,
                                // so a superseded attempt still finishes).
                                drop(slot);
                            }
                            Err(e) => tracing::warn!("File transfer of {name} failed: {e}"),
                        }
                    })
                    .await;
            }
        }
        Ok(())
    }
}

/// Answer a packet the owner's permissions denied, when its sender waits for
/// an answer: a folder request is told why, so the phone does not wait on
/// silence. Every other denied packet stays unanswered.
pub(crate) async fn answer_denied(packet: &NetworkPacket, sender: &dyn DeviceSender) {
    if packet.packet_type != FOLDER_FETCH {
        return;
    }
    let asked = packet
        .body
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let refused = Err("Files is turned off for this phone on the computer".to_string());
    crate::offers::answer_folder_request(sender, asked, &refused).await;
}

/// The phone asks for the folder at `path` below home, whole: held to the
/// remote browser's rules, then offered to it as the owner's folder sends
/// are. Always answered.
async fn handle_folder_fetch(packet: &NetworkPacket, sender: &dyn DeviceSender) {
    let asked = packet
        .body
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    // Every file would be fetched by a download, so a phone the desktop
    // cannot reach is told now, not promised a folder that never comes.
    let checked =
        can_reach(sender).and_then(|_| crate::plugins::file_browse::resolve_folder(asked));
    let refused = match checked {
        Err(reason) => reason,
        Ok((_, shown)) => match crate::state::state_dir() {
            // Offered and answered there, or refused and answered there.
            Ok(dir) => {
                let _ =
                    crate::offers::offer_folder_to(&dir, sender.device_id(), &shown, asked, sender)
                        .await;
                return;
            }
            Err(e) => format!("no state folder: {e}"),
        },
    };
    crate::offers::answer_folder_request(sender, asked, &Err(refused)).await;
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
    // A retry of this download replaces an attempt still running for it,
    // before plan_fetch takes a slot (T6a).
    let body = &packet.body;
    let running_key = match (
        body.get("offer").and_then(|v| v.as_str()),
        body.get("path").and_then(|v| v.as_str()),
    ) {
        (Some(offer), _) => format!("offer:{offer}"),
        (None, Some(path)) => format!("path:{}:{path}", sender.device_id()),
        (None, None) => format!("fetch:{}", uuid::Uuid::new_v4().simple()),
    };
    // The owner cancelled this file while it ran (C1): its retry is told so.
    if crate::transfers::take_cancelled(&running_key) {
        tracing::info!("Download refused: the owner cancelled it");
        refuse(sender, &key, crate::transfers::CANCELLED_HERE).await;
        return;
    }
    running().supersede(&running_key).await;
    match plan_fetch(&packet.body, sender) {
        Ok(fetch) => {
            tracing::info!("Sending {} to {}", fetch.path.display(), fetch.route);
            let tracker = crate::transfers::track(crate::transfers::Shown {
                key: running_key.clone(),
                direction: pocketdesk_core::local::transfers::Direction::ToPhone,
                device_id: phone_of(sender),
                name: fetch
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                folder: None,
                size: fetch.size,
            });
            running()
                .spawn(running_key, async move {
                    let _slot = fetch.slot;
                    // An offer being fetched stays while bytes go (N6 review).
                    let mut shown = tracker.progress();
                    let alive = fetch.offer.clone();
                    let mut touched = std::time::Instant::now();
                    let progress = move |sent: u64| {
                        shown(sent);
                        if let Some(id) = &alive
                            && touched.elapsed() >= std::time::Duration::from_secs(60)
                        {
                            crate::offers::keep_alive(id);
                            touched = std::time::Instant::now();
                        }
                    };
                    let sent = send_file_counted(fetch.route, &key, &fetch.path, progress).await;
                    drop(tracker);
                    match &sent {
                        Ok(size) => {
                            tracing::info!("File sent: {} ({size} bytes)", fetch.path.display())
                        }
                        Err(e) => tracing::warn!("Sending {} failed: {e}", fetch.path.display()),
                    }
                    if let Some(id) = fetch.offer {
                        crate::offers::finish(&id, sent.map(|_| ()).map_err(|e| e.to_string()));
                    }
                })
                .await;
        }
        Err(reason) => {
            tracing::warn!("Download refused: {reason}");
            refuse(sender, &key, &reason).await;
        }
    }
}

/// A download that passed every check, holding its transfer slot.
struct Fetch {
    /// How the bytes reach the phone that asked.
    route: Route,
    path: PathBuf,
    size: u64,
    /// The desktop app's send this download answers, if any.
    offer: Option<String>,
    slot: TransferSlot,
}

fn plan_fetch(
    body: &serde_json::Value,
    sender: &dyn DeviceSender,
) -> std::result::Result<Fetch, String> {
    let port = body
        .get("port")
        .and_then(|v| v.as_u64())
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p != 0);
    let reach = reach(sender, port)?;
    // Taken before an offer is redeemed, so a busy desktop does not spend
    // one of the phone's tries.
    let slot = transfer_slots().take(sender.device_id())?;
    let (path, size, offer) = if let Some(path) = body.get("path").and_then(|v| v.as_str()) {
        // A file by its path is browsing this computer (M2b): the same
        // permission as listing a folder, beyond Files, which the packet's
        // gate checked for the desktop's offers.
        if !crate::plugins::pair::permits(sender.device_id(), "kdeconnect.filebrowse.request") {
            return Err("browsing this computer's files is off for this phone".to_string());
        }
        let (path, size) = crate::plugins::file_browse::resolve_file(path)?;
        (path, size, None)
    } else if let Some(id) = body.get("offer").and_then(|v| v.as_str()) {
        let path = crate::offers::redeem(id, &phone_of(sender))?;
        // A file of a folder the phone asked for is held to the browser's
        // rules again now, not only when the folder was walked.
        if let Some(within) = crate::offers::asked_within(id)
            && let Err(reason) = crate::plugins::file_browse::still_within(&path, &within)
        {
            crate::offers::fail(id, reason.clone());
            return Err(reason);
        }
        let Some(size) = std::fs::metadata(&path)
            .ok()
            .filter(|m| m.is_file())
            .map(|m| m.len())
        else {
            let reason = "the file is no longer there";
            crate::offers::fail(id, reason.to_string());
            return Err(reason.to_string());
        };
        (path, size, Some(id.to_string()))
    } else {
        return Err("no file named".to_string());
    };
    if size > MAX_FILE_BYTES {
        let reason = "the file is larger than 8 GB";
        if let Some(id) = &offer {
            crate::offers::fail(id, reason.to_string());
        }
        return Err(reason.to_string());
    }
    // Last, after every check that can refuse: at home it reads this
    // desktop's certificate.
    let route = match route(reach, sender) {
        Ok(route) => route,
        Err(reason) => {
            if let Some(id) = &offer {
                crate::offers::fail(id, reason.clone());
            }
            return Err(reason);
        }
    };
    Ok(Fetch {
        route,
        path,
        size,
        offer,
        slot,
    })
}

/// How a transfer reaches the phone.
pub(crate) enum Route {
    /// Dial it back at home, inside the transfer's TLS (phase C).
    Dial {
        to: SocketAddr,
        tls: std::sync::Arc<tokio_rustls::rustls::ClientConfig>,
    },
    /// A stream on the phone's own control connection, by the internet road
    /// (2c): iroh proved both keys, and nothing dials back across a NAT.
    Link(std::sync::Arc<dyn OpenLinkStream>),
    /// A stream another of the owner's computers opened on this one's link
    /// to it (2h), its key line already read: that end presented the key.
    Arrived(std::sync::Mutex<Option<Box<dyn TransferIo>>>),
}

impl std::fmt::Display for Route {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dial { to, .. } => write!(f, "{to}"),
            Self::Link(_) => f.write_str("the phone's link by the internet road"),
            Self::Arrived(_) => f.write_str("the link to the other computer"),
        }
    }
}

impl Route {
    pub(crate) fn dial(
        to: SocketAddr,
        tls: std::sync::Arc<tokio_rustls::rustls::ClientConfig>,
    ) -> Self {
        Self::Dial { to, tls }
    }

    /// The stream another computer opened for a transfer (2h).
    pub(crate) fn arrived(stream: Box<dyn TransferIo>) -> Self {
        Self::Arrived(std::sync::Mutex::new(Some(stream)))
    }

    /// Whether this end presents the key: it does on a stream it opened.
    fn presents_key(&self) -> bool {
        !matches!(self, Self::Arrived(_))
    }

    /// Open the transfer's stream: the first thing written on it is the key,
    /// by whichever end opened it.
    async fn open(&self) -> Result<Box<dyn TransferIo>> {
        match self {
            Self::Arrived(stream) => stream
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
                .ok_or_else(|| io_error("open", "the stream was used")),
            Self::Dial { to, tls } => Ok(Box::new(dial_transfer(*to, tls.clone()).await?)),
            Self::Link(link) => tokio::time::timeout(CONNECT_TIMEOUT, link.open())
                .await
                .map_err(|_| io_error("open", "timed out"))?
                .map_err(|e| io_error("open", e)),
        }
    }
}

/// Where a request from `sender` that named `port` (or none) can be
/// answered, before the checks that can refuse it: a port means dialing the
/// phone back at home; none, a stream on its link by the internet road.
pub(crate) enum Reach {
    Dial(IpAddr, u16),
    Link(std::sync::Arc<dyn OpenLinkStream>),
}

pub(crate) fn reach(
    sender: &dyn DeviceSender,
    port: Option<u16>,
) -> std::result::Result<Reach, String> {
    reach_with(sender, port, crate::events::current_config().internet_files)
}

/// [`reach`] with `internet_files` given.
fn reach_with(
    sender: &dyn DeviceSender,
    port: Option<u16>,
    internet_files: bool,
) -> std::result::Result<Reach, String> {
    match (sender.link_streams(), port) {
        (Some(link), None) => {
            if !internet_files {
                return Err("files over the internet road are off on this computer".to_string());
            }
            Ok(Reach::Link(link))
        }
        (Some(_), Some(_)) => {
            Err("nothing connects back by the internet road: ask without a port".to_string())
        }
        (None, Some(port)) => Ok(Reach::Dial(dial_back(sender)?, port)),
        (None, None) => Err("no usable port".to_string()),
    }
}

/// Whether files can reach the phone behind `sender`: a connection with an
/// address to dial back, or a link by the internet road while
/// `internet_files` is on (2c).
pub(crate) fn files_reach(sender: &dyn DeviceSender) -> bool {
    sender.peer_addr().is_some()
        || (sender.link_streams().is_some() && crate::events::current_config().internet_files)
}

/// Whether files can reach the phone behind `sender` at all: a folder it
/// asks for is fetched file by file.
fn can_reach(sender: &dyn DeviceSender) -> std::result::Result<(), String> {
    if sender.link_streams().is_some() {
        return reach(sender, None).map(|_| ());
    }
    dial_back(sender).map(|_| ())
}

/// The route for `reach`: at home with the transfer's TLS; on the internet
/// road with the iroh key the link proved, held to the one the phone
/// registered at home.
pub(crate) fn route(reach: Reach, sender: &dyn DeviceSender) -> std::result::Result<Route, String> {
    match reach {
        Reach::Dial(ip, port) => Ok(Route::dial(
            SocketAddr::new(ip, port),
            transfer_tls(sender)?,
        )),
        Reach::Link(link) => {
            let Some(key) = sender.peer_key() else {
                return Err("the phone's connection proved no key".to_string());
            };
            crate::plugins::pair::may_transfer(sender.device_id(), &key)?;
            Ok(Route::Link(link))
        }
    }
}

/// This desktop's side of a transfer's TLS with the phone behind `sender`
/// (phase C): this desktop's certificate, and the phone must present the
/// key its control connection proved. A connection that proved none gets
/// no transfer.
pub(crate) fn transfer_tls(
    sender: &dyn DeviceSender,
) -> std::result::Result<std::sync::Arc<tokio_rustls::rustls::ClientConfig>, String> {
    let Some(key @ pocketdesk_core::streaming::PeerKey::Tls(phone)) = sender.peer_key() else {
        return Err("the phone's connection proved no key".to_string());
    };
    crate::plugins::pair::may_transfer(sender.device_id(), &key)?;
    Ok(desktop_certs()?.transfer_client_config(phone))
}

/// This desktop's identity, read once: the one its control channel and
/// stream present (loaded at start by the service from the same folder).
fn desktop_certs()
-> std::result::Result<std::sync::Arc<pocketdesk_core::streaming::transport::CertManager>, String> {
    use pocketdesk_core::streaming::transport::CertManager;
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
    if stream.get_ref().1.alpn_protocol() != Some(pocketdesk_core::protocol::ALPN_TRANSFER) {
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

pub(crate) async fn refuse(sender: &dyn DeviceSender, key: &str, reason: &str) {
    let packet = NetworkPacket::new(FETCH_REFUSED)
        .with_body(serde_json::json!({ "key": key, "reason": reason }));
    if let Err(e) = sender.send_packet(&packet).await {
        tracing::debug!("Could not tell the phone the download was refused: {e}");
    }
}

/// Send the file at `path` to the phone by `route`, and return its size
/// once the phone says it saved it.
pub(crate) async fn send_file_to(route: Route, key: &str, path: &Path) -> Result<u64> {
    send_file_counted(route, key, path, |_| {}).await
}

/// [`send_file_to`], telling `progress` how much of the file the phone has
/// (C1).
pub(crate) async fn send_file_counted(
    route: Route,
    key: &str,
    path: &Path,
    progress: impl FnMut(u64),
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
    send_reader_counted(route, key, &mut file, size, progress).await
}

/// Open `route`, present `key`, and send `size` bytes of `reader` with the
/// resumable transfer (T6a): from where the phone's partial copy ends, if it
/// has one of this file, then the whole file's hash; the result is the
/// phone's status byte. A reader that ends early (the file shrank) fails the
/// send.
pub(crate) async fn send_reader<R: tokio::io::AsyncRead + Unpin>(
    route: Route,
    key: &str,
    reader: &mut R,
    size: u64,
) -> Result<u64> {
    send_reader_counted(route, key, reader, size, |_| {}).await
}

/// [`send_reader`], telling `progress` how much the phone has.
async fn send_reader_counted<R: tokio::io::AsyncRead + Unpin>(
    route: Route,
    key: &str,
    reader: &mut R,
    size: u64,
    progress: impl FnMut(u64),
) -> Result<u64> {
    let mut stream = route.open().await?;
    if route.presents_key() {
        present_key(&mut stream, key).await?;
    }
    let from = transfer::send(reader, size, &mut stream, Pace::DEFAULT, progress)
        .await
        .map_err(|e| io_error("send", e))?;
    if from > 0 {
        tracing::info!("The phone had {from} of {size} bytes; sent the rest");
    }
    Ok(size)
}

async fn present_key<S: tokio::io::AsyncWrite + Unpin>(stream: &mut S, key: &str) -> Result<()> {
    stream
        .write_all(format!("{key}\n").as_bytes())
        .await
        .map_err(|e| io_error("write", e))?;
    stream.flush().await.map_err(|e| io_error("write", e))
}

/// Tell the owner a file arrived: a desktop notification, and a row in the
/// received-files list the desktop app shows (roadmap F7).
/// A file from `from` was saved at `path`; `top` is the folder of the
/// folder send (T6b) it belongs to, if it does. It is listed for the
/// desktop app and announced.
pub(crate) fn on_received(path: &Path, size: u64, from: &str, top: Option<&Path>) {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    // O7: an image sent on its own also goes onto the clipboard, with the
    // switch on; the clipboard is not held by a pause (K2a).
    if top.is_none() && crate::events::current_config().received_images_clipboard {
        crate::plugins::clipboard::put_received_image(path.to_path_buf(), size, from.to_string());
    }
    let Some((summary, body)) = received_in(&dir, path, size, from, top) else {
        return;
    };
    // Paused (K2a): listed on the Files page, not announced (its name
    // could show on a shared screen).
    if crate::paused::now() {
        tracing::info!("A file arrived while paused; it is on the Files page");
        return;
    }
    std::thread::spawn(move || {
        if !crate::platform::notify(summary, &body, Duration::from_secs(8)) {
            tracing::info!("{summary} — {body}");
        }
    });
}

/// List a received file in `state_dir`, and the notification to show for
/// it, if any. A folder send's files make one row for the folder, and only
/// its first file is announced: a folder of a thousand files is not a
/// thousand notifications.
fn received_in(
    state_dir: &Path,
    path: &Path,
    size: u64,
    from: &str,
    top: Option<&Path>,
) -> Option<(&'static str, String)> {
    use pocketdesk_core::local::received::{self, ReceivedFile};
    let name_of = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    // A folder at a drive's root has no name of its own: its path then.
    let parent_name = |p: &Path| {
        p.parent()
            .map(|parent| {
                let name = name_of(parent);
                if name.is_empty() {
                    parent.display().to_string()
                } else {
                    name
                }
            })
            .unwrap_or_default()
    };
    // Transfers run concurrently; the list is read, changed and replaced.
    static LIST: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LIST.lock().unwrap_or_else(|e| e.into_inner());
    let now = pocketdesk_core::local::pin::now_unix();
    match top {
        None => {
            let name = name_of(path);
            let entry = ReceivedFile {
                name: name.clone(),
                path: path.display().to_string(),
                size,
                received_unix: now,
                from: from.to_string(),
            };
            if let Err(e) = received::record(state_dir, entry) {
                tracing::warn!("Could not record the received file: {e}");
            }
            crate::events::emit(
                "file_received",
                Some(from),
                serde_json::json!({ "size": size, "key": received::path_key(&path.display().to_string()) }),
                serde_json::json!({ "name": name }),
            );
            Some((
                "Pocketdesk: file received",
                format!("{name} ({}) is in {}.", human_size(size), parent_name(path)),
            ))
        }
        Some(top) => {
            let folder = name_of(top);
            let entry = ReceivedFile {
                name: format!("{folder}/"),
                path: top.display().to_string(),
                size,
                received_unix: now,
                from: from.to_string(),
            };
            let added = received::add_to_folder(state_dir, entry);
            // A folder is one event, as it is one notification: a folder of
            // a thousand files would otherwise be a thousand hooks.
            if matches!(added, Ok(true)) {
                crate::events::emit(
                    "file_received",
                    Some(from),
                    serde_json::json!({ "folder": true, "key": received::path_key(&top.display().to_string()) }),
                    serde_json::json!({ "name": format!("{folder}/") }),
                );
            }
            match added {
                Ok(true) => Some((
                    "Pocketdesk: folder arriving",
                    format!("{folder} is arriving in {}.", parent_name(top)),
                )),
                Ok(false) => None,
                Err(e) => {
                    tracing::warn!("Could not record the received file: {e}");
                    None
                }
            }
        }
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
pub(crate) fn is_transfer_key(key: &str) -> bool {
    key.len() == 32 && key.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether the desktop may connect back to a phone at `ip`: not itself
/// (a loopback peer means a local proxy such as `adb reverse`, and the
/// phone's port would name a service on this machine), and not an address
/// that is no single host or is the link-local range (cloud metadata).
/// The address a download to `sender` would be dialed at, or why there is
/// none: the desktop dials the transport's peer, and only a real one.
fn dial_back(sender: &dyn DeviceSender) -> std::result::Result<IpAddr, String> {
    let ip = sender
        .peer_addr()
        .map(|a| a.ip())
        .ok_or("downloads need the local network connection")?;
    if !dialable(ip) {
        return Err(format!("will not connect back to {ip}"));
    }
    Ok(ip)
}

pub(crate) fn dialable(ip: IpAddr) -> bool {
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
                && !pocketdesk_core::protocol::kdeconnect::is_invisible_format(*c)
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
    Err(PocketdeskError::Io {
        operation: "create_file",
        detail: format!("too many files named like {name}"),
    })
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

/// A file a phone is sending: where it goes, and what names its partial
/// copy between attempts.
struct Incoming<'a> {
    /// The folder the finished file goes to (Downloads).
    dir: &'a Path,
    name: &'a str,
    /// The size the request announced.
    size: u64,
    /// The phone sending it.
    device: &'a str,
    /// The phone's id for the file; without one the transfer cannot resume.
    source: Option<&'a str>,
    store: &'a Store,
    /// How much unfinished work is kept; an age of 0 keeps none.
    limits: pocketdesk_core::protocol::partials::Limits,
    /// Told how much of the file this end has as it arrives (C1).
    progress: Box<dyn FnMut(u64) + Send>,
    /// Told where the file went once it is in place (the notification and
    /// the received list), before the phone hears "saved".
    on_saved: Box<dyn FnOnce(&Path) + Send>,
    /// The date the file was last changed where it came from (K1), set
    /// once it is in place; none keeps the arrival's.
    modified: Option<std::time::SystemTime>,
}

/// Reach the phone by `route` and receive one file with the resumable
/// transfer (T6a), continuing a partial copy an earlier attempt left. The
/// file goes into `dir` under a name no file has yet, and the phone hears
/// "saved" only once it is there. A connection that breaks keeps what
/// arrived for the next attempt (when the file has a source id and
/// unfinished files are kept); anything else leaves nothing behind.
async fn receive_file(route: Route, key: &str, incoming: Incoming<'_>) -> Result<PathBuf> {
    let Incoming {
        dir,
        name,
        size,
        device,
        source,
        store,
        limits,
        progress,
        on_saved,
        modified,
    } = incoming;
    if size == 0 || size > MAX_FILE_BYTES {
        return Err(io_error(
            "receive",
            format!("refused a file of declared size {size}"),
        ));
    }
    let resumable = limits.max_age_secs > 0 && source.is_some();
    // Without a source id the partial is this attempt's alone.
    let source = source
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
    std::fs::create_dir_all(store.dir()).map_err(|e| io_error("create_dir", e))?;
    let location = format!("{}.part", Store::id(device, &source));
    // In use before the sweep, so the sweep keeps this transfer's own part.
    let in_use_guard = InUse::new(&location);
    sweep_partials(store, limits);
    let part_path = store.dir().join(&location);
    let record = store
        .get(device, &source)
        .filter(|_| resumable && part_path.exists());
    let have = record.as_ref().map(|r| Have {
        size: r.size,
        committed: r.committed,
    });
    let created_unix = record
        .as_ref()
        .map_or_else(pocketdesk_core::local::pin::now_unix, |r| r.created_unix);
    let mut file = tokio::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&part_path)
        .await
        .map_err(|e| io_error("create_file", e))?;
    let forget = || {
        store.remove(device, &source);
        let _ = std::fs::remove_file(&part_path);
    };

    let mut stream = match route.open().await {
        Ok(stream) => stream,
        Err(e) => {
            drop(file);
            if !resumable {
                forget();
            }
            return Err(e);
        }
    };
    if route.presents_key() {
        present_key(&mut stream, key).await?;
    }
    tracing::info!("Receiving {name} from {route} ({size} bytes)");
    let outcome = transfer::receive_counted(
        &mut stream,
        &mut file,
        have,
        Some(size),
        MAX_FILE_BYTES,
        Pace::DEFAULT,
        |size, committed| {
            if resumable {
                let record = Record {
                    device: device.to_string(),
                    source: source.clone(),
                    name: name.to_string(),
                    size,
                    committed,
                    created_unix,
                    location: location.clone(),
                };
                if let Err(e) = store.put(&record) {
                    tracing::warn!("Could not record the unfinished file: {e}");
                }
            }
        },
        progress,
    )
    .await;
    drop(file);
    match outcome {
        Ok(offset) => {
            if offset > 0 {
                tracing::info!("Continued {name} from {offset} of {size} bytes");
            }
            // Its own task: a retry that supersedes this attempt (the phone
            // gave up waiting while a large copy ran) must not stop the move
            // half way, nor leave the record of a file already in Downloads.
            let completion = tokio::spawn({
                let part_path = part_path.clone();
                let dir = dir.to_path_buf();
                let name = name.to_string();
                let records = Store::new(store.dir().to_path_buf());
                let device = device.to_string();
                let source = source.clone();
                async move {
                    let _in_use = in_use_guard;
                    let placed = place(&part_path, &dir, &name, modified).await;
                    records.remove(&device, &source);
                    let _ = std::fs::remove_file(&part_path);
                    if let Ok(path) = &placed {
                        tracing::info!("File received: {} ({size} bytes)", path.display());
                        on_saved(path);
                    }
                    transfer::finish(&mut stream, placed.is_ok(), Pace::DEFAULT).await;
                    placed
                }
            });
            completion.await.map_err(|e| io_error("place", e))?
        }
        Err(e) => {
            // Only a broken connection is worth continuing; a changed or
            // damaged file, or a refused size, starts over next time.
            if !(resumable && matches!(e, TransferError::Io(_))) {
                forget();
            }
            Err(io_error("receive", e))
        }
    }
}

/// Receive a file another of the owner's computers offered this one (2h),
/// on the stream it opened on this computer's link to it, as a phone's
/// share is received: into Downloads (a folder send's file under its own
/// new folder), continuing what an earlier attempt left, and announced.
/// `offer` is the offer's body (`name`, `size`, `source`, and a folder
/// send's `folderId`, `folder`, `path`).
pub(crate) async fn receive_offered(
    stream: Box<dyn TransferIo>,
    from: &str,
    offer: &serde_json::Value,
) -> Result<PathBuf> {
    let name = safe_file_name(offer.get("name").and_then(|v| v.as_str()).unwrap_or(""));
    let size = offer.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
    let source = offer
        .get("source")
        .and_then(|v| v.as_str())
        .filter(|s| pocketdesk_core::protocol::partials::valid_source(s))
        .map(str::to_string);
    let dir = download_dir().ok_or_else(|| io_error("receive", "no downloads or home folder"))?;
    let store =
        partials().ok_or_else(|| io_error("receive", "no state folder for the transfer"))?;
    let (dir, name, top) = match folder_of(offer) {
        None => (dir, name, None),
        Some((folder_id, folder, path)) => {
            let (dir, name) = folder_tops()
                .place(&dir, from, &folder_id, &folder, &path)
                .map_err(|reason| io_error("receive", reason))?;
            let top = folder_tops().top(from, &folder_id);
            (dir, name, top)
        }
    };
    let announced = from.to_string();
    receive_file(
        Route::arrived(stream),
        "",
        Incoming {
            dir: &dir,
            name: &name,
            size,
            device: from,
            source: source.as_deref(),
            store: &store,
            limits: partial_limits(),
            progress: Box::new(|_| {}),
            on_saved: Box::new(move |path: &Path| {
                on_received(path, size, &announced, top.as_deref())
            }),
            modified: received_date(offer),
        },
    )
    .await
}

/// The date a share or offer `body` gives its file (K1), held to now; none
/// with Settings "Keep files' dates" off.
pub(crate) fn received_date(body: &serde_json::Value) -> Option<std::time::SystemTime> {
    received_date_when(
        body,
        crate::events::current_config().keep_file_dates,
        std::time::SystemTime::now(),
    )
}

/// [`received_date`] with the setting given.
fn received_date_when(
    body: &serde_json::Value,
    keep: bool,
    now: std::time::SystemTime,
) -> Option<std::time::SystemTime> {
    keep.then(|| pocketdesk_core::protocol::file_time::from_body(body, now))
        .flatten()
}

/// Date the file at `path` `at` (K1). A file that will not take it is kept
/// as it is, and the failure logged.
fn set_date(path: &Path, at: std::time::SystemTime) {
    // Only the right to change its times (K1 review): a file that has gone
    // read-only, or that another program holds, still takes its date.
    let mut options = std::fs::OpenOptions::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
        options.access_mode(FILE_WRITE_ATTRIBUTES);
    }
    #[cfg(not(windows))]
    options.read(true);
    let set = options.open(path).and_then(|file| file.set_modified(at));
    if let Err(e) = set {
        tracing::warn!("Could not date {}: {e}", path.display());
    }
}

/// Move the finished `part` into `dir` under a name like `name` that no
/// file has yet, and return where it went. A rename when the state folder
/// and `dir` share a volume, else a copy. With `modified`, the file is
/// dated so once it is there (K1).
pub(crate) async fn place(
    part: &Path,
    dir: &Path,
    name: &str,
    modified: Option<std::time::SystemTime>,
) -> Result<PathBuf> {
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| io_error("create_dir", e))?;
    let (final_path, placeholder) = create_unique(dir, name).await?;
    drop(placeholder);
    let moved = match tokio::fs::rename(part, &final_path).await {
        Ok(()) => Ok(()),
        Err(_) => tokio::fs::copy(part, &final_path)
            .await
            .map(|_| ())
            .map_err(|e| io_error("copy", e)),
    };
    if let Err(e) = moved {
        let _ = tokio::fs::remove_file(&final_path).await;
        return Err(e);
    }
    mark_of_the_web(&final_path);
    // After the mark: writing it may touch the file's time.
    if let Some(at) = modified {
        set_date(&final_path, at);
    }
    Ok(final_path)
}

/// Connect to the phone at `from`, receive `size` bytes (at most `max`)
/// into memory, and hand them to `keep`. The phone hears "saved" only when
/// `keep` succeeds, so its status byte means the bytes were used, not only
/// received (the clipboard image, roadmap N4). Small and replaced by the
/// next copy, so it never resumes; the whole-file hash is still checked.
pub(crate) async fn receive_bytes<F, Fut>(
    route: Route,
    key: &str,
    size: u64,
    max: u64,
    keep: F,
) -> Result<()>
where
    F: FnOnce(Vec<u8>) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    if size == 0 || size > max {
        return Err(io_error(
            "receive",
            format!("refused a declared size of {size}"),
        ));
    }
    let mut stream = route.open().await?;
    present_key(&mut stream, key).await?;
    let mut memory = transfer::Memory::holding(&[]);
    let received = transfer::receive(
        &mut stream,
        &mut memory,
        None,
        Some(size),
        max,
        Pace::DEFAULT,
        |_, _| {},
    )
    .await;
    let kept = match received {
        Ok(_) => keep(memory.into_bytes()).await,
        Err(e) => return Err(io_error("receive", e)),
    };
    transfer::finish(&mut stream, kept.is_ok(), Pace::DEFAULT).await;
    kept
}

pub(crate) fn io_error(operation: &'static str, detail: impl std::fmt::Display) -> PocketdeskError {
    PocketdeskError::Io {
        operation,
        detail: detail.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// K1 review: with "Keep files' dates" off, a body's date is no date.
    #[test]
    fn a_date_is_none_with_the_setting_off() {
        let body = serde_json::json!({ "lastModified": 1_780_000_000_000u64 });
        let now = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_800_000_000);
        let june = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_780_000_000);
        assert_eq!(received_date_when(&body, true, now), Some(june));
        assert_eq!(received_date_when(&body, false, now), None);
    }

    /// K1 review: a read-only file takes its date too.
    #[test]
    fn a_read_only_file_takes_its_date() {
        let dir = std::env::temp_dir().join(format!("pd-k1-readonly-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, b"hello").unwrap();
        let mut permissions = std::fs::metadata(&file).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&file, permissions.clone()).unwrap();
        let june = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_780_000_000);
        set_date(&file, june);
        assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), june);
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(&file, permissions).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// K1: a placed file takes the date it is given, and keeps its own
    /// without one.
    #[tokio::test]
    async fn a_placed_file_takes_its_date() {
        let dir = std::env::temp_dir().join(format!("pd-k1-place-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let june = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_780_000_000);
        let part = dir.join("a.part");
        std::fs::write(&part, b"hello").unwrap();
        let dated = place(&part, &dir, "a.txt", Some(june)).await.unwrap();
        assert_eq!(std::fs::metadata(&dated).unwrap().modified().unwrap(), june);
        let part = dir.join("b.part");
        std::fs::write(&part, b"hello").unwrap();
        let undated = place(&part, &dir, "b.txt", None).await.unwrap();
        let age = std::time::SystemTime::now()
            .duration_since(std::fs::metadata(&undated).unwrap().modified().unwrap())
            .unwrap_or_default();
        assert!(age.as_secs() < 60, "its own date: now");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn received_files_go_to_the_chosen_folder_only_when_it_is_one() {
        let dir = std::env::temp_dir().join(format!("pd-c2-folder-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let none: &[PathBuf] = &[];
        assert_eq!(
            chosen_folder(&dir.display().to_string(), none),
            Some(dir.clone())
        );
        assert_eq!(chosen_folder("", none), None, "unset: Downloads");
        assert_eq!(chosen_folder("   ", none), None);
        assert_eq!(chosen_folder("relative/folder", none), None, "not absolute");
        let gone = dir.join("gone");
        assert_eq!(
            chosen_folder(&gone.display().to_string(), none),
            None,
            "gone"
        );
        let file = dir.join("a-file");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(
            chosen_folder(&file.display().to_string(), none),
            None,
            "not a folder"
        );
        // The server's own folders, and inside them, are never it.
        let inside = dir.join("inside");
        std::fs::create_dir_all(&inside).unwrap();
        let ours = [dir.clone()];
        assert_eq!(chosen_folder(&dir.display().to_string(), &ours), None);
        assert_eq!(chosen_folder(&inside.display().to_string(), &ours), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    fn parts(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    const FOLDER_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const FOLDER_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn the_files_of_one_folder_share_a_new_top_folder() {
        let downloads = temp_dir("folders");
        let tops = FolderTops::default();
        std::fs::create_dir_all(downloads.join("Photos")).unwrap(); // already there
        let (dir, name) = tops
            .place(
                &downloads,
                "phone",
                FOLDER_A,
                "Photos",
                &parts(&["2026", "beach.jpg"]),
            )
            .unwrap();
        assert_eq!(dir, downloads.join("Photos (1)").join("2026"));
        assert_eq!(name, "beach.jpg");
        assert!(dir.is_dir());
        let (dir, _) = tops
            .place(
                &downloads,
                "phone",
                FOLDER_A,
                "Photos",
                &parts(&["top.jpg"]),
            )
            .unwrap();
        assert_eq!(
            dir,
            downloads.join("Photos (1)"),
            "the same send, the same folder"
        );
        let (dir, _) = tops
            .place(
                &downloads,
                "phone",
                FOLDER_B,
                "Photos",
                &parts(&["top.jpg"]),
            )
            .unwrap();
        assert_eq!(
            dir,
            downloads.join("Photos (2)"),
            "another send, a new folder"
        );
        let (dir, _) = tops
            .place(
                &downloads,
                "other-phone",
                FOLDER_A,
                "Photos",
                &parts(&["x.jpg"]),
            )
            .unwrap();
        assert_eq!(
            dir,
            downloads.join("Photos (3)"),
            "another phone's id is its own"
        );
    }

    #[test]
    fn a_folder_send_that_keeps_going_keeps_its_folder() {
        let downloads = temp_dir("folder-long").join("Downloads");
        let tops = FolderTops::default();
        let start = std::time::Instant::now();
        let at = |minutes: u64| start + Duration::from_secs(minutes * 60);
        let place = |file: &str, now| {
            tops.place_at(
                &downloads,
                "phone",
                FOLDER_A,
                "Photos",
                &parts(&[file]),
                now,
            )
            .unwrap()
            .0
        };
        let first = place("a.jpg", at(0));
        // Files keep coming for longer than the TTL, never far apart.
        assert_eq!(place("b.jpg", at(50)), first);
        assert_eq!(
            place("c.jpg", at(100)),
            first,
            "past an hour from the first"
        );
        // A send idle for longer than the TTL is forgotten.
        assert_ne!(place("d.jpg", at(100 + 61)), first);
    }

    #[test]
    fn a_folder_path_that_climbs_out_is_refused() {
        let downloads = temp_dir("folders-bad");
        let tops = FolderTops::default();
        for bad in [
            parts(&["..", "x"]),
            parts(&["a", "..", "..", "x"]),
            parts(&["C:", "x"]),
            parts(&[]),
        ] {
            assert!(
                tops.place(&downloads, "phone", FOLDER_A, "Photos", &bad)
                    .is_err(),
                "{bad:?}"
            );
        }
        assert!(
            tops.place(&downloads, "phone", "not-an-id", "Photos", &parts(&["x"]))
                .is_err()
        );
        assert!(
            tops.place(&downloads, "phone", FOLDER_A, "..", &parts(&["x"]))
                .is_err()
        );
        assert_eq!(
            std::fs::read_dir(&downloads).unwrap().count(),
            0,
            "nothing was made"
        );
    }

    #[test]
    fn the_sweep_leaves_a_part_a_receive_is_writing() {
        let store = Store::new(temp_dir("sweep-in-use"));
        let part = store.dir().join("busy.part");
        std::fs::write(&part, b"half").unwrap();
        let nothing_kept = pocketdesk_core::protocol::partials::Limits {
            max_age_secs: 0,
            per_device: 0,
            total_bytes: 0,
        };
        let writing = InUse::new("busy.part");
        sweep_partials(&store, nothing_kept);
        assert!(part.exists(), "another transfer's sweep must not take it");
        drop(writing);
        sweep_partials(&store, nothing_kept);
        assert!(!part.exists(), "once unused, an orphan part goes");
    }

    #[tokio::test]
    async fn a_retry_replaces_the_attempt_still_running_for_its_file() {
        static RUNNING: std::sync::OnceLock<Running> = std::sync::OnceLock::new();
        let running = RUNNING.get_or_init(Running::default);
        let slots = std::sync::Arc::new(TransferSlots::new(2, 1));
        // A stale attempt: holds the phone's one slot and never ends.
        let slot = slots.take("phone").unwrap();
        running
            .spawn("file-a".into(), async move {
                let _slot = slot;
                std::future::pending::<()>().await;
            })
            .await;
        assert!(
            slots.take("phone").is_err(),
            "the stale attempt holds the slot"
        );
        running.supersede("file-a").await;
        assert!(!running.is_running("file-a").await);
        let slot = slots.take("phone").expect("the slot came back");
        // A finished attempt forgets itself.
        running
            .spawn("file-b".into(), async move { drop(slot) })
            .await;
        for _ in 0..50 {
            if !running.is_running("file-b").await {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(!running.is_running("file-b").await);
    }

    #[test]
    fn a_phones_next_file_is_not_refused_while_its_last_one_finishes() {
        let slots = TransferSlots::new(MAX_TRANSFERS, MAX_TRANSFERS_PER_PHONE);
        let finishing = slots.take("phone-a").unwrap();
        let next = slots.take("phone-a").expect("the next queued file");
        let other = slots
            .take("phone-b")
            .expect("another phone still gets a slot");
        assert!(slots.take("phone-a").is_err(), "but not a third");
        drop((finishing, next, other));
    }

    #[test]
    fn one_phone_cannot_take_every_transfer_slot() {
        let slots = TransferSlots::new(2, 1);
        let a = slots.take("phone-a").unwrap();
        assert!(
            slots.take("phone-a").is_err(),
            "a phone's second transfer waits"
        );
        let b = slots.take("phone-b").unwrap();
        assert!(slots.take("phone-c").is_err(), "the desktop's limit");
        drop(a);
        assert!(
            slots.take("phone-a").is_ok(),
            "a finished transfer frees both"
        );
        drop(b);
    }

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

    use pocketdesk_core::streaming::transport::CertManager;

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

    /// A phone-side listener that sends the first `holding` bytes of a
    /// `size`-byte file to whoever presents [`KEY`], with the resumable
    /// transfer, then closes. Yields the send's outcome: the offset the
    /// desktop continued from, or why it did not end saved.
    async fn phone_sending_part(
        data: Vec<u8>,
        size: u64,
    ) -> (
        SocketAddr,
        tokio::task::JoinHandle<std::result::Result<u64, TransferError>>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let outcome = tokio::spawn(async move {
            let mut socket = accept_as(&certs().1, &listener).await.unwrap();
            let mut presented = [0u8; 33];
            socket.read_exact(&mut presented).await.unwrap();
            assert_eq!(&presented, format!("{KEY}\n").as_bytes());
            let mut reader: &[u8] = &data;
            transfer::send(&mut reader, size, &mut socket, Pace::DEFAULT, |_| {}).await
        });
        (addr, outcome)
    }

    /// [`phone_sending_part`] with the whole file.
    async fn phone_sending(
        data: Vec<u8>,
    ) -> (
        SocketAddr,
        tokio::task::JoinHandle<std::result::Result<u64, TransferError>>,
    ) {
        let size = data.len() as u64;
        phone_sending_part(data, size).await
    }

    const SOURCE: &str = "00112233445566778899aabbccddeeff";

    const LIMITS: pocketdesk_core::protocol::partials::Limits =
        pocketdesk_core::protocol::partials::Limits {
            max_age_secs: 3600,
            per_device: 4,
            total_bytes: 1 << 40,
        };

    fn incoming<'a>(
        dir: &'a Path,
        name: &'a str,
        size: u64,
        source: Option<&'a str>,
        store: &'a Store,
    ) -> Incoming<'a> {
        Incoming {
            dir,
            name,
            size,
            device: "phone",
            source,
            store,
            limits: LIMITS,
            progress: Box::new(|_| {}),
            on_saved: Box::new(|_| {}),
            modified: None,
        }
    }

    #[test]
    fn a_received_file_is_listed_for_the_desktop_app() {
        let state = temp_dir("list");
        let path = state.join("photo.jpg");
        let toast = received_in(&state, &path, 2_100_000, "192.0.2.7", None);
        assert!(toast.is_some(), "a plain file is announced");
        let files = pocketdesk_core::local::received::read(&state);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "photo.jpg");
        assert_eq!(files[0].size, 2_100_000);
        assert_eq!(files[0].path, path.display().to_string());
        assert_eq!(human_size(2_100_000), "2.0 MB");
        assert_eq!(human_size(512), "512 B");
    }

    #[test]
    fn a_folders_files_are_one_row_and_one_toast() {
        let state = temp_dir("list-folder");
        let top = state.join("Trip");
        let first = received_in(&state, &top.join("a.jpg"), 10, "phone", Some(&top));
        let second = received_in(
            &state,
            &top.join("day 2").join("b.jpg"),
            5,
            "phone",
            Some(&top),
        );
        let (_, body) = first.expect("the folder's first file is announced");
        assert!(body.contains("Trip"), "{body}");
        assert!(second.is_none(), "its later files are not");
        let files = pocketdesk_core::local::received::read(&state);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "Trip/");
        assert_eq!(files[0].path, top.display().to_string());
        assert_eq!(files[0].size, 15);
    }

    #[test]
    fn a_transfer_key_is_32_hex_digits() {
        assert!(is_transfer_key(KEY));
        assert!(!is_transfer_key(""));
        assert!(!is_transfer_key(&KEY[1..]));
        assert!(!is_transfer_key("0123456789abcdef0123456789abcdeg"));
    }

    #[tokio::test]
    async fn bytes_taken_into_memory_are_saved_only_once_they_are_used() {
        let data = vec![9u8; 70_000];
        let (from, status) = phone_sending(data.clone()).await;
        let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = got.clone();
        receive_bytes(
            Route::dial(from, desktop_tls()),
            KEY,
            data.len() as u64,
            1 << 20,
            |b| async move {
                *sink.lock().unwrap() = b;
                Ok(())
            },
        )
        .await
        .unwrap();
        assert!(status.await.unwrap().is_ok(), "told it was saved");
        assert_eq!(*got.lock().unwrap(), data);

        let (from, status) = phone_sending(vec![1, 2, 3]).await;
        let refused = receive_bytes(
            Route::dial(from, desktop_tls()),
            KEY,
            3,
            1 << 20,
            |_| async { Err(io_error("clipboard", "not a PNG")) },
        )
        .await;
        assert!(refused.is_err());
        assert!(
            matches!(status.await.unwrap(), Err(TransferError::NotSaved)),
            "received but not used is not saved"
        );
    }

    #[tokio::test]
    async fn bytes_over_the_limit_are_refused_before_connecting() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let from = listener.local_addr().unwrap();
        for size in [0, 11] {
            assert!(
                receive_bytes(Route::dial(from, desktop_tls()), KEY, size, 10, |_| async {
                    Ok(())
                })
                .await
                .is_err()
            );
        }
        let accepted =
            tokio::time::timeout(std::time::Duration::from_millis(200), listener.accept()).await;
        assert!(accepted.is_err(), "no connection was made");
    }

    #[tokio::test]
    async fn the_desktop_dials_the_phone_and_saves_exactly_the_declared_bytes() {
        let dir = temp_dir("receive");
        let store = Store::new(temp_dir("receive-partials"));
        let data: Vec<u8> = (0..200_000u32).map(|i| i as u8).collect();
        let (from, status) = phone_sending(data.clone()).await;
        let size = data.len() as u64;
        let path = receive_file(
            Route::dial(from, desktop_tls()),
            KEY,
            incoming(&dir, "big.bin", size, Some(SOURCE), &store),
        )
        .await
        .unwrap();
        assert_eq!(path, dir.join("big.bin"));
        assert_eq!(status.await.unwrap().unwrap(), 0, "told it was saved");
        assert_eq!(std::fs::read(path).unwrap(), data);
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["big.bin"], "nothing else in Downloads");
        assert_eq!(
            std::fs::read_dir(store.dir()).unwrap().count(),
            0,
            "no partial kept"
        );
    }

    #[tokio::test]
    async fn a_broken_share_is_continued_by_the_phones_retry() {
        let dir = temp_dir("resume");
        let store = Store::new(temp_dir("resume-partials"));
        let data: Vec<u8> = (0..300_000u32).map(|i| (i * 13) as u8).collect();
        let size = data.len() as u64;

        // The first attempt stops at 120 000 bytes.
        let (from, first) = phone_sending_part(data[..120_000].to_vec(), size).await;
        let broken = receive_file(
            Route::dial(from, desktop_tls()),
            KEY,
            incoming(&dir, "video.mp4", size, Some(SOURCE), &store),
        )
        .await;
        assert!(broken.is_err());
        assert!(first.await.unwrap().is_err());
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "no half file in Downloads"
        );
        assert_eq!(store.get("phone", SOURCE).unwrap().committed, 120_000);

        // The retry sends only the rest.
        let (from, second) = phone_sending(data.clone()).await;
        let path = receive_file(
            Route::dial(from, desktop_tls()),
            KEY,
            incoming(&dir, "video.mp4", size, Some(SOURCE), &store),
        )
        .await
        .unwrap();
        assert_eq!(second.await.unwrap().unwrap(), 120_000, "continued");
        assert_eq!(std::fs::read(path).unwrap(), data);
        assert!(store.get("phone", SOURCE).is_none(), "the record is gone");
    }

    /// A link by the internet road (2c), in memory: each stream the desktop
    /// opens is handed to the phone's side.
    struct FakeLink(tokio::sync::mpsc::UnboundedSender<tokio::io::DuplexStream>);

    #[async_trait::async_trait]
    impl OpenLinkStream for FakeLink {
        async fn open(&self) -> std::io::Result<Box<dyn TransferIo>> {
            let (desktop, phone) = tokio::io::duplex(64 * 1024);
            self.0
                .send(phone)
                .map_err(|_| std::io::Error::other("the phone is gone"))?;
            Ok(Box::new(desktop))
        }
    }

    type PhoneStreams = tokio::sync::mpsc::UnboundedReceiver<tokio::io::DuplexStream>;

    fn fake_link() -> (std::sync::Arc<dyn OpenLinkStream>, PhoneStreams) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (std::sync::Arc::new(FakeLink(tx)), rx)
    }

    /// The phone's side of a share on the link: the key, then the first
    /// `data` bytes of a `size`-byte file.
    fn phone_sending_on_link(
        mut streams: PhoneStreams,
        data: Vec<u8>,
        size: u64,
    ) -> tokio::task::JoinHandle<(PhoneStreams, std::result::Result<u64, TransferError>)> {
        tokio::spawn(async move {
            let mut stream = streams.recv().await.unwrap();
            let mut presented = [0u8; 33];
            stream.read_exact(&mut presented).await.unwrap();
            assert_eq!(&presented, format!("{KEY}\n").as_bytes());
            let mut reader: &[u8] = &data;
            let sent = transfer::send(&mut reader, size, &mut stream, Pace::DEFAULT, |_| {}).await;
            (streams, sent)
        })
    }

    #[tokio::test]
    async fn a_share_on_the_link_is_saved_and_a_broken_one_continued() {
        let dir = temp_dir("link-share");
        let store = Store::new(temp_dir("link-share-partials"));
        let data: Vec<u8> = (0..300_000u32).map(|i| (i * 11) as u8).collect();
        let size = data.len() as u64;
        let (link, streams) = fake_link();

        // The first attempt stops at 100 000 bytes.
        let first = phone_sending_on_link(streams, data[..100_000].to_vec(), size);
        let broken = receive_file(
            Route::Link(link.clone()),
            KEY,
            incoming(&dir, "photo.raw", size, Some(SOURCE), &store),
        )
        .await;
        assert!(broken.is_err());
        let (streams, sent) = first.await.unwrap();
        assert!(sent.is_err());
        assert_eq!(store.get("phone", SOURCE).unwrap().committed, 100_000);

        // The retry, on a new stream of the same link, sends only the rest.
        let second = phone_sending_on_link(streams, data.clone(), size);
        let path = receive_file(
            Route::Link(link),
            KEY,
            incoming(&dir, "photo.raw", size, Some(SOURCE), &store),
        )
        .await
        .unwrap();
        assert_eq!(second.await.unwrap().1.unwrap(), 100_000, "continued");
        assert_eq!(std::fs::read(path).unwrap(), data, "the SHA-256 held");
    }

    #[tokio::test]
    async fn a_download_on_the_link_sends_the_file_and_waits_for_the_save() {
        let dir = temp_dir("link-fetch");
        let data: Vec<u8> = (0..120_000u32).map(|i| (i * 5) as u8).collect();
        let path = dir.join("a.bin");
        std::fs::write(&path, &data).unwrap();
        let (link, mut streams) = fake_link();
        let phone = tokio::spawn(async move {
            let mut stream = streams.recv().await.unwrap();
            let mut presented = [0u8; 33];
            stream.read_exact(&mut presented).await.unwrap();
            assert_eq!(&presented, format!("{KEY}\n").as_bytes());
            let mut memory = transfer::Memory::holding(&[]);
            let received = transfer::receive(
                &mut stream,
                &mut memory,
                None,
                None,
                MAX_FILE_BYTES,
                Pace::DEFAULT,
                |_, _| {},
            )
            .await;
            transfer::finish(&mut stream, received.is_ok(), Pace::DEFAULT).await;
            memory.into_bytes()
        });
        assert_eq!(
            send_file_to(Route::Link(link), KEY, &path).await.unwrap(),
            data.len() as u64
        );
        assert_eq!(phone.await.unwrap(), data);
    }

    #[tokio::test]
    async fn a_clipboard_image_comes_over_the_link_and_its_limit_holds() {
        let png: Vec<u8> = (0..40_000u32).map(|i| (i % 253) as u8).collect();
        let size = png.len() as u64;
        let (link, streams) = fake_link();
        let phone = phone_sending_on_link(streams, png.clone(), size);
        let kept = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        receive_bytes(Route::Link(link.clone()), KEY, size, size, |bytes| {
            let kept = kept.clone();
            async move {
                *kept.lock().unwrap() = bytes;
                Ok(())
            }
        })
        .await
        .unwrap();
        assert_eq!(phone.await.unwrap().1.unwrap(), 0);
        assert_eq!(*kept.lock().unwrap(), png);
        // Over the limit: refused before a stream is opened.
        assert!(
            receive_bytes(Route::Link(link), KEY, size, size - 1, |_| async { Ok(()) })
                .await
                .is_err()
        );
    }

    #[test]
    fn the_road_decides_how_a_request_is_answered() {
        let (link, _streams) = fake_link();
        let internet = pocketdesk_core::protocol::kdeconnect::TcpDeviceSender::new(
            tokio::io::sink(),
            "phone".into(),
        )
        .with_link_streams(Some(link));
        let home = pocketdesk_core::protocol::kdeconnect::TcpDeviceSender::new(
            tokio::io::sink(),
            "phone".into(),
        )
        .with_peer(Some("192.0.2.20:40000".parse().unwrap()));
        assert!(matches!(
            reach_with(&internet, None, true),
            Ok(Reach::Link(_))
        ));
        assert!(
            reach_with(&internet, None, false)
                .err()
                .is_some_and(|e| e.contains("are off")),
            "the switch off says so"
        );
        assert!(
            reach_with(&internet, Some(5000), true).is_err(),
            "nothing dials back by the internet road"
        );
        assert!(matches!(
            reach_with(&home, Some(5000), true),
            Ok(Reach::Dial(_, 5000))
        ));
        assert!(
            reach_with(&home, None, true).is_err(),
            "at home a request names its port"
        );
    }

    #[tokio::test]
    async fn a_partial_is_continued_only_for_its_own_phone_and_only_while_kept() {
        let dir = temp_dir("resume-other");
        let store = Store::new(temp_dir("resume-other-partials"));
        let data: Vec<u8> = (0..200_000u32).map(|i| (i * 3) as u8).collect();
        let size = data.len() as u64;
        let (from, _) = phone_sending_part(data[..90_000].to_vec(), size).await;
        let _ = receive_file(
            Route::dial(from, desktop_tls()),
            KEY,
            incoming(&dir, "a.bin", size, Some(SOURCE), &store),
        )
        .await;
        // Another phone sending a file with the same source id starts over.
        let (from, other) = phone_sending(data.clone()).await;
        let mut theirs = incoming(&dir, "a.bin", size, Some(SOURCE), &store);
        theirs.device = "another-phone";
        receive_file(Route::dial(from, desktop_tls()), KEY, theirs)
            .await
            .unwrap();
        assert_eq!(other.await.unwrap().unwrap(), 0);
        assert!(
            store.get("phone", SOURCE).is_some(),
            "the first phone's stays"
        );

        // With nothing kept, a broken share leaves nothing.
        let none = Store::new(temp_dir("resume-none-partials"));
        let (from, _) = phone_sending_part(data[..90_000].to_vec(), size).await;
        let mut not_kept = incoming(&dir, "b.bin", size, Some(SOURCE), &none);
        not_kept.limits.max_age_secs = 0;
        assert!(
            receive_file(Route::dial(from, desktop_tls()), KEY, not_kept)
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_dir(none.dir()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn a_short_transfer_without_a_source_id_leaves_no_file() {
        let dir = temp_dir("short");
        let store = Store::new(temp_dir("short-partials"));
        let (from, status) = phone_sending_part(vec![1; 10], 20).await;
        assert!(
            receive_file(
                Route::dial(from, desktop_tls()),
                KEY,
                incoming(&dir, "x.bin", 20, None, &store)
            )
            .await
            .is_err()
        );
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        assert_eq!(std::fs::read_dir(store.dir()).unwrap().count(), 0);
        assert!(status.await.unwrap().is_err(), "not saved");
    }

    #[tokio::test]
    async fn an_undeclared_or_huge_size_is_refused_before_connecting() {
        let dir = temp_dir("size");
        let store = Store::new(temp_dir("size-partials"));
        let unused: SocketAddr = "127.0.0.1:9".parse().unwrap();
        for size in [0, MAX_FILE_BYTES + 1] {
            assert!(
                receive_file(
                    Route::dial(unused, desktop_tls()),
                    KEY,
                    incoming(&dir, "x", size, Some(SOURCE), &store)
                )
                .await
                .is_err()
            );
        }
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }

    /// A phone-side listener for a download: checks the key, receives the
    /// file with the resumable transfer, and answers saved or not. Yields
    /// what it received.
    async fn phone_fetching(saved: bool) -> (SocketAddr, tokio::task::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let got = tokio::spawn(async move {
            let mut socket = accept_as(&certs().1, &listener).await.unwrap();
            let mut presented = [0u8; 33];
            socket.read_exact(&mut presented).await.unwrap();
            assert_eq!(&presented, format!("{KEY}\n").as_bytes());
            let mut memory = transfer::Memory::holding(&[]);
            let received = transfer::receive(
                &mut socket,
                &mut memory,
                None,
                None,
                MAX_FILE_BYTES,
                Pace::DEFAULT,
                |_, _| {},
            )
            .await;
            transfer::finish(&mut socket, saved && received.is_ok(), Pace::DEFAULT).await;
            memory.into_bytes()
        });
        (addr, got)
    }

    #[tokio::test]
    async fn a_download_sends_the_size_then_the_bytes_and_waits_for_the_save() {
        let dir = temp_dir("fetch");
        let data: Vec<u8> = (0..150_000u32).map(|i| (i * 7) as u8).collect();
        let path = dir.join("a.bin");
        std::fs::write(&path, &data).unwrap();
        let (to, got) = phone_fetching(true).await;
        assert_eq!(
            send_file_to(Route::dial(to, desktop_tls()), KEY, &path)
                .await
                .unwrap(),
            data.len() as u64
        );
        assert_eq!(got.await.unwrap(), data);
    }

    #[tokio::test]
    async fn a_download_the_phone_could_not_save_fails() {
        let dir = temp_dir("fetch-fail");
        let path = dir.join("a.bin");
        std::fs::write(&path, b"abc").unwrap();
        let (to, _) = phone_fetching(false).await;
        assert!(
            send_file_to(Route::dial(to, desktop_tls()), KEY, &path)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_file_that_shrinks_fails_the_send() {
        let (to, got) = phone_fetching(true).await;
        // Listed at 10 bytes, holding 4 by the time it is read.
        let sent = send_reader(Route::dial(to, desktop_tls()), KEY, &mut &b"abcd"[..], 10).await;
        assert!(sent.is_err(), "{sent:?}");
        assert!(got.await.unwrap().len() <= 4, "nothing past what there was");
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
        assert!(
            send_file_to(Route::dial(to, desktop_tls()), KEY, &path)
                .await
                .is_err()
        );
        assert!(impostor.await.unwrap().is_empty(), "not a byte of it");
    }

    /// The phone's control connection, as the plugin sees it.
    struct Phone {
        peer: SocketAddr,
        sent: std::sync::Mutex<Vec<NetworkPacket>>,
        keyless: bool,
    }

    impl Phone {
        fn at(peer: &str) -> Self {
            Self {
                peer: peer.parse().unwrap(),
                sent: Default::default(),
                keyless: false,
            }
        }

        /// A phone whose connection proved no key: it can move no files.
        fn keyless(peer: &str) -> Self {
            Self {
                keyless: true,
                ..Self::at(peer)
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
        fn peer_key(&self) -> Option<pocketdesk_core::streaming::PeerKey> {
            if self.keyless {
                return None;
            }
            Some(pocketdesk_core::streaming::PeerKey::Tls(
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

    fn share(size: u64) -> NetworkPacket {
        NetworkPacket::new("kdeconnect.share.request")
            .with_body(serde_json::json!({
                "filename": "a.bin",
                "payloadTransferInfo": { "port": 40000, "key": KEY },
            }))
            .with_payload_size(size)
    }

    /// A file the owner cancelled while it ran (C1): the phone's retry is
    /// refused with why, once, so it stops; a later attempt is not told it
    /// again.
    #[tokio::test]
    async fn a_download_the_owner_cancelled_is_refused_with_why_once() {
        crate::transfers::cancelled_for_test("offer:c1-cancelled-offer");
        let asked = || {
            fetch(serde_json::json!({
                "offer": "c1-cancelled-offer", "port": 40000, "key": KEY,
            }))
        };
        let phone = Phone::at("192.0.2.7:51000");
        handle_fetch(&asked(), &phone).await;
        let (key, reason) = refusal(&phone).expect("refused");
        assert_eq!(key, KEY);
        assert_eq!(reason, crate::transfers::CANCELLED_HERE);
        let again = Phone::at("192.0.2.7:51001");
        handle_fetch(&asked(), &again).await;
        let (_, reason) = refusal(&again).expect("refused: the offer is gone");
        assert_ne!(reason, crate::transfers::CANCELLED_HERE, "told once");
    }

    /// A refused share is answered, so the phone stops waiting for a
    /// connection that will never come and can say why.
    #[tokio::test]
    async fn a_refused_share_is_answered_with_its_key() {
        let plugin = SharePlugin::new();
        let loopback = Phone::at("127.0.0.1:51000");
        plugin.handle_packet(&share(10), &loopback).await.unwrap();
        let (key, reason) = refusal(&loopback).expect("a refusal");
        assert_eq!(key, KEY);
        assert!(!reason.is_empty());

        let phone = Phone::at("192.0.2.7:51000");
        plugin
            .handle_packet(&share(MAX_FILE_BYTES + 1), &phone)
            .await
            .unwrap();
        let (key, reason) = refusal(&phone).expect("a refusal for the size");
        assert_eq!(key, KEY);
        assert!(
            reason.contains("size") || reason.contains("large"),
            "{reason}"
        );
    }

    #[tokio::test]
    async fn a_folder_share_whose_path_climbs_out_is_refused() {
        let plugin = SharePlugin::new();
        let phone = Phone::at("192.0.2.7:51000");
        let packet = NetworkPacket::new("kdeconnect.share.request")
            .with_body(serde_json::json!({
                "filename": "x.txt",
                "folder": "Photos",
                "folderId": FOLDER_A,
                "path": ["..", "..", "x.txt"],
                "payloadTransferInfo": { "port": 40000, "key": KEY },
            }))
            .with_payload_size(10);
        plugin.handle_packet(&packet, &phone).await.unwrap();
        let (key, reason) = refusal(&phone).expect("a refusal");
        assert_eq!(key, KEY);
        assert!(reason.contains("place in the folder"), "{reason}");
    }

    #[tokio::test]
    async fn a_refused_folder_share_makes_no_folder() {
        let downloads =
            std::env::temp_dir().join(format!("ll-share-refused-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&downloads);
        std::fs::create_dir_all(&downloads).unwrap();
        let plugin = SharePlugin::with_download_dir(downloads.clone());
        // This phone's connection proved no key, so no transfer can start.
        let phone = Phone::keyless("192.0.2.7:51000");
        let packet = NetworkPacket::new("kdeconnect.share.request")
            .with_body(serde_json::json!({
                "filename": "a.jpg",
                "folder": "Photos",
                "folderId": FOLDER_A,
                "path": ["day 1", "a.jpg"],
                "payloadTransferInfo": { "port": 40000, "key": KEY },
            }))
            .with_payload_size(10);
        plugin.handle_packet(&packet, &phone).await.unwrap();
        assert!(refusal(&phone).is_some());
        let made: Vec<_> = std::fs::read_dir(&downloads).unwrap().flatten().collect();
        assert!(made.is_empty(), "{made:?}");
    }

    #[tokio::test]
    async fn a_folder_is_not_promised_to_a_connection_the_desktop_cannot_dial() {
        let plugin = SharePlugin::new();
        let phone = Phone::at("127.0.0.1:51000");
        // A folder that is not there: whatever the order of the checks, this
        // test never offers a real folder or writes the real sent list.
        let home = dirs::home_dir().unwrap();
        let asked = home.join("pd-test-no-such-folder").display().to_string();
        let packet =
            NetworkPacket::new(FOLDER_FETCH).with_body(serde_json::json!({ "path": asked }));
        plugin.handle_packet(&packet, &phone).await.unwrap();
        let reply = phone.replies().pop().expect("an answer");
        assert_eq!(reply.body["queued"], false);
        assert!(
            reply.body["reason"]
                .as_str()
                .unwrap()
                .contains("connect back"),
            "{}",
            reply.body["reason"]
        );
    }

    #[tokio::test]
    async fn a_folder_request_the_permissions_deny_is_still_answered() {
        let phone = Phone::at("192.0.2.7:51000");
        let asked = NetworkPacket::new(FOLDER_FETCH).with_body(serde_json::json!({ "path": "x" }));
        answer_denied(&asked, &phone).await;
        let reply = phone
            .replies()
            .pop()
            .expect("an answer, not 30 s of silence");
        assert_eq!(reply.body["path"], "x");
        assert_eq!(reply.body["queued"], false);
        assert!(reply.body["reason"].as_str().unwrap().contains("Files"));
        // Other denied packets stay unanswered, as before.
        let other = NetworkPacket::new(FETCH).with_body(serde_json::json!({ "path": "x" }));
        answer_denied(&other, &phone).await;
        assert_eq!(phone.replies().len(), 1);
    }

    #[tokio::test]
    async fn a_folder_outside_home_is_refused_with_its_path() {
        let plugin = SharePlugin::new();
        assert!(
            plugin.incoming_capabilities().contains(&FOLDER_FETCH),
            "routed to this plugin"
        );
        let phone = Phone::at("192.0.2.7:51000");
        let asked = std::env::temp_dir().display().to_string();
        let packet =
            NetworkPacket::new(FOLDER_FETCH).with_body(serde_json::json!({ "path": asked }));
        plugin.handle_packet(&packet, &phone).await.unwrap();
        let reply = phone.replies().pop().expect("an answer");
        assert_eq!(reply.packet_type, crate::offers::FOLDER_FETCH_REPLY);
        assert_eq!(reply.body["path"], asked.as_str());
        assert_eq!(reply.body["queued"], false);
        assert!(!reply.body["reason"].as_str().unwrap().is_empty());
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
