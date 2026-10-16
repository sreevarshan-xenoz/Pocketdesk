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
/// The answer to a phone's request for a folder (`share::FOLDER_FETCH`):
/// `{path, queued, reason}`. Sent before the folder's offers.
pub const FOLDER_FETCH_REPLY: &str = "kdeconnect.pocketdesk.folder_fetch_reply";
/// How many folder sends one phone may have on the way when it asks for
/// another.
const MAX_ASKED_FOLDERS: usize = 2;
/// How many folders one phone may ask for in a minute, accepted or not:
/// each costs the desktop a walk, and a phone can end a send at once by
/// declining its offers.
const MAX_ASKED_PER_MINUTE: usize = 10;

/// Whether `device` may ask for another folder at `now`; counts the ask.
fn may_ask_again(device: &str, now: Instant) -> bool {
    static ASKED: OnceLock<Mutex<HashMap<String, std::collections::VecDeque<Instant>>>> =
        OnceLock::new();
    let mut asked = ASKED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let times = asked.entry(device.to_string()).or_default();
    while times
        .front()
        .is_some_and(|t| now.saturating_duration_since(*t) >= Duration::from_secs(60))
    {
        times.pop_front();
    }
    if times.len() >= MAX_ASKED_PER_MINUTE {
        return false;
    }
    times.push_back(now);
    true
}
/// How long the phone has to fetch an offered file. Long enough for a phone
/// that fetches a dropped batch one file after another; a folder's files
/// have this long from the phone's last fetch of any of them.
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

/// When an offer's time is up. A folder send's files share one (T6b), which
/// every fetch of them moves on, so a long folder taken one file after
/// another never runs out while the phone is still taking it.
type Deadline = Arc<Mutex<Instant>>;

/// A file offered to one phone.
#[derive(Debug, Clone)]
struct Offer {
    path: PathBuf,
    device_id: String,
    expires: Deadline,
    /// Fetches begun. One runs at a time: a retry replaces an attempt
    /// still running (`share::running`).
    attempts: u8,
    /// The real root of the folder the phone asked for, when it did: the
    /// file is checked against it again as it is fetched.
    within: Option<PathBuf>,
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
    offer_with(
        table,
        id,
        path,
        device_id,
        Arc::new(Mutex::new(now + OFFER_TTL)),
    );
}

/// Offer each of a folder send's `files` `(id, path)` to `device_id`, under
/// one deadline.
fn offer_folder_at(
    files: &[(String, PathBuf)],
    device_id: &str,
    within: Option<&Path>,
    now: Instant,
) {
    offer_folder_in(offers(), files, device_id, within, now);
}

fn offer_folder_in(
    table: &Table,
    files: &[(String, PathBuf)],
    device_id: &str,
    within: Option<&Path>,
    now: Instant,
) {
    let deadline: Deadline = Arc::new(Mutex::new(now + OFFER_TTL));
    for (id, path) in files {
        offer_with(table, id, path.clone(), device_id, deadline.clone());
        if let Some(within) = within
            && let Some(offer) = table.lock().unwrap_or_else(|e| e.into_inner()).get_mut(id)
        {
            offer.within = Some(within.to_path_buf());
        }
    }
}

/// The real root of the folder the phone asked for that offer `id` is
/// part of, if the phone asked for it (`None` for the owner's sends).
pub(crate) fn asked_within(id: &str) -> Option<PathBuf> {
    offers()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
        .and_then(|o| o.within.clone())
}

fn offer_with(table: &Table, id: &str, path: PathBuf, device_id: &str, expires: Deadline) {
    let mut offers = table.lock().unwrap_or_else(|e| e.into_inner());
    offers.insert(
        id.to_string(),
        Offer {
            path,
            device_id: device_id.to_string(),
            expires,
            attempts: 0,
            within: None,
        },
    );
}

impl Offer {
    fn expired(&self, now: Instant) -> bool {
        *self.expires.lock().unwrap_or_else(|e| e.into_inner()) <= now
    }

    /// The phone is at work on this offer: it (and its folder) has a whole
    /// TTL from `now`.
    fn touch(&self, now: Instant) {
        let mut until = self.expires.lock().unwrap_or_else(|e| e.into_inner());
        *until = (*until).max(now + OFFER_TTL);
    }
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
    if offer.expired(now) {
        offers.remove(id);
        return Err(OFFER_EXPIRED);
    }
    if offer.attempts >= MAX_ATTEMPTS {
        offers.remove(id);
        return Err(TOO_MANY);
    }
    offer.attempts += 1;
    offer.touch(now);
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

fn settle_in(table: &Table, id: &str, sent: bool, now: Instant) -> Settled {
    let mut offers = table.lock().unwrap_or_else(|e| e.into_inner());
    let Some(offer) = offers.get_mut(id) else {
        return if sent { Settled::Done } else { Settled::GaveUp };
    };
    // A fetch just ended: the rest of its folder, or its retry, is next.
    offer.touch(now);
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
        .filter(|(_, o)| o.expired(now))
        .map(|(id, _)| id.clone())
        .collect();
    for id in &expired {
        offers.remove(id);
    }
    expired
}

/// A folder send in progress (T6b): its files' offers, and how they ended.
#[derive(Debug, Default)]
struct Group {
    /// The phone it goes to.
    device: String,
    /// The phone asked for it (rather than the owner sending it): counted
    /// against that phone's [`MAX_ASKED_FOLDERS`].
    asked: bool,
    /// Its files' offers; `None` while the folder is still being walked.
    total: Option<usize>,
    ended: usize,
    failed: usize,
    last_reason: Option<String>,
}

/// Folder sends by their sent-list id, and each file offer's folder send.
#[derive(Default)]
struct Groups {
    groups: HashMap<String, Group>,
    member_of: HashMap<String, String>,
}

fn groups() -> &'static Mutex<Groups> {
    static GROUPS: OnceLock<Mutex<Groups>> = OnceLock::new();
    GROUPS.get_or_init(Default::default)
}

/// A folder send reserved and not yet started ([`start_group`]): dropped
/// before it starts (refused, or its request's future cancelled), the
/// reservation goes, so it can never hold a phone's place for good.
struct Reservation(String);

impl Reservation {
    /// Reserve the folder send `group` to `device`. `limit` is set for a
    /// phone's own request: refused (`None`) while that phone already has
    /// `limit` of its own on the way.
    fn take(group: &str, device: &str, limit: Option<usize>) -> Option<Self> {
        let mut g = groups().lock().unwrap_or_else(|e| e.into_inner());
        let asked_on_the_way = g
            .groups
            .values()
            .filter(|g| g.asked && g.device == device)
            .count();
        if limit.is_some_and(|limit| asked_on_the_way >= limit) {
            return None;
        }
        g.groups.insert(
            group.to_string(),
            Group {
                device: device.to_string(),
                asked: limit.is_some(),
                ..Group::default()
            },
        );
        Some(Self(group.to_string()))
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut g = groups().lock().unwrap_or_else(|e| e.into_inner());
        if g.groups.get(&self.0).is_some_and(|g| g.total.is_none()) {
            g.groups.remove(&self.0);
        }
    }
}

/// Make `offer_ids` the files of the folder send `group`.
fn start_group(group: &str, offer_ids: &[&str]) {
    let mut g = groups().lock().unwrap_or_else(|e| e.into_inner());
    g.groups.entry(group.to_string()).or_default().total = Some(offer_ids.len());
    for id in offer_ids {
        g.member_of.insert(id.to_string(), group.to_string());
    }
}

/// Record that offer `id` ended with `outcome`: its own row, or, for a
/// file of a folder send, its folder's row once every file has ended.
fn end_offer(dir: &Path, id: &str, outcome: Result<(), String>) {
    let folder_done = {
        let mut g = groups().lock().unwrap_or_else(|e| e.into_inner());
        match g.member_of.remove(id) {
            None => None,
            Some(group_id) => {
                let group = g.groups.entry(group_id.clone()).or_default();
                group.ended += 1;
                if let Err(reason) = &outcome {
                    group.failed += 1;
                    group.last_reason = Some(reason.clone());
                }
                if group.total.is_none_or(|total| group.ended < total) {
                    return;
                }
                let group = g.groups.remove(&group_id).unwrap_or_default();
                Some((group_id, group))
            }
        }
    };
    match folder_done {
        None => finish_in(dir, id, outcome),
        Some((group_id, group)) => {
            let outcome = if group.failed == 0 {
                Ok(())
            } else {
                Err(format!(
                    "{} of {} files not sent: {}",
                    group.failed,
                    group.total.unwrap_or_default(),
                    group.last_reason.unwrap_or_default()
                ))
            };
            finish_in(dir, &group_id, outcome);
        }
    }
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
    end_offer(dir, id, Err(reason));
}

/// Record how a fetch of offer `id` ended. A failure leaves the offer for
/// the phone's retry until its attempts run out.
pub fn finish(id: &str, outcome: Result<(), String>) {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    match settle_in(offers(), id, outcome.is_ok(), Instant::now()) {
        Settled::Done | Settled::GaveUp => end_offer(&dir, id, outcome),
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
        end_offer(&dir, id, Err(reason));
    }
}

fn finish_in(dir: &Path, id: &str, outcome: Result<(), String>) {
    let state = match outcome {
        Ok(()) => SentState::Sent,
        Err(reason) => SentState::Failed(reason),
    };
    update_sent(dir, id, state);
}

/// Send a file to another of the owner's computers (2h): on the link to it,
/// as a phone shares one, never as an offer (the other computer is not this
/// one's client). Recorded first, then sent on a task of its own.
fn deliver_to_computer(dir: &Path, request: SendRequest, path: PathBuf) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| request.path.clone());
    let size = std::fs::metadata(&path)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| m.len());
    let checked = if path.is_dir() {
        Err("a folder does not go to another computer yet: send its files")
    } else if is_servers_own(&path, &crate::plugins::file_browse::private_dirs()) {
        Err("the server's own files are not sent")
    } else if size.is_none() {
        Err("there is no such file")
    } else if !crate::peer_computers::is_linked(&request.device_id) {
        Err("the computer is not linked now")
    } else {
        Ok(())
    };
    let file = SentFile {
        id: request.id.clone(),
        name,
        path: request.path.clone(),
        size: size.unwrap_or(0),
        device_id: request.device_id.clone(),
        sent_unix: now_unix(),
        state: match &checked {
            Ok(()) => SentState::Offered,
            Err(reason) => SentState::Failed(reason.to_string()),
        },
        note: None,
    };
    {
        let _guard = sent_lock().lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = sent::record(dir, file) {
            tracing::warn!("Could not record the sent file: {e}");
        }
    }
    if let Err(reason) = checked {
        tracing::warn!(
            "Not sending {} to {}: {reason}",
            path.display(),
            request.device_id
        );
        return;
    }
    let dir = dir.to_path_buf();
    tokio::spawn(async move {
        let state = match crate::peer_computers::send_file(&request.device_id, &path).await {
            Ok(()) => SentState::Sent,
            Err(reason) => {
                tracing::warn!(
                    "Sending {} to another computer failed: {reason}",
                    path.display()
                );
                SentState::Failed(reason)
            }
        };
        update_sent(&dir, &request.id, state);
    });
}

/// The live connection that reaches `device_id` (a connection goes by its
/// device's id, phase B). Only one files can reach carries the download that
/// follows: an address to dial back, or a link by the internet road (2c).
fn target_for(device_id: &str, clients: &[Arc<dyn DeviceSender>]) -> Option<Arc<dyn DeviceSender>> {
    clients
        .iter()
        .filter(|c| crate::plugins::share::files_reach(c.as_ref()))
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
        crate::inbox::wait(pocketdesk_core::local::paths::OUTBOX_DIR, POLL).await;
        for id in sweep_at(Instant::now()) {
            tracing::info!("Offer {id} expired before the phone took it");
            end_offer(&dir, &id, Err(EXPIRED.to_string()));
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
    if crate::peer_computers::is_paired(&request.device_id) {
        return deliver_to_computer(dir, request, path);
    }
    if path.is_dir() {
        return deliver_folder(dir, request, path, clients, store).await;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| request.path.clone());
    let size = std::fs::metadata(&path)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| m.len());
    // The server's keys and trust store go nowhere, whoever names them.
    let checked = if is_servers_own(&path, &crate::plugins::file_browse::private_dirs()) {
        Err("the server's own files are not sent")
    } else {
        check(&request, size, clients, store)
    };
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
        note: None,
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

/// The most files one folder send offers.
const MAX_FOLDER_FILES: usize = 10_000;
/// The most entries one folder walk looks at (folders, files and names
/// left out alike), so a tree of empty folders cannot keep it going.
const MAX_FOLDER_ENTRIES: usize = 100_000;
/// The most bytes one folder send offers.
const MAX_FOLDER_BYTES: u64 = 32 * 1024 * 1024 * 1024;

/// One file of a folder to offer.
#[derive(Debug, Clone)]
struct FolderFile {
    full: PathBuf,
    /// Its names under the folder, its own name last.
    path: Vec<String>,
    size: u64,
}

#[derive(Debug, Default)]
struct FolderFiles {
    files: Vec<FolderFile>,
    /// Names left out because the phone could not take them.
    skipped: usize,
    /// Cut short at a limit.
    truncated: bool,
}

/// The files under `root` to offer (T6b): hidden entries (a leading dot,
/// or marked hidden on Windows), links and the server's own state and
/// config left out, names `relpath` refuses counted and left out, at most
/// [`MAX_FOLDER_FILES`], 32 folders deep, [`MAX_FOLDER_BYTES`] in all and
/// [`MAX_FOLDER_ENTRIES`] looked at,
/// and no single file over 8 GB. In name order, so the phone sees a send
/// the same way twice.
fn folder_files(root: &Path) -> Result<FolderFiles, String> {
    folder_files_with(root, &crate::plugins::file_browse::private_dirs())
}

/// [`folder_files`], leaving out anything at or below a folder in
/// `private` (real paths).
fn folder_files_with(root: &Path, private: &[PathBuf]) -> Result<FolderFiles, String> {
    folder_files_capped(root, private, MAX_FOLDER_ENTRIES)
}

/// [`folder_files_with`], looking at no more than `max_entries` entries.
fn folder_files_capped(
    root: &Path,
    private: &[PathBuf],
    max_entries: usize,
) -> Result<FolderFiles, String> {
    use crate::plugins::file_browse::is_hidden;
    use pocketdesk_core::protocol::relpath;
    fn walk(
        dir: &Path,
        under: &[String],
        private: &[PathBuf],
        found: &mut FolderFiles,
        bytes: &mut u64,
        entries_left: &mut usize,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if found.truncated {
                return;
            }
            if *entries_left == 0 {
                found.truncated = true;
                return;
            }
            *entries_left -= 1;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            // Links (and Windows junctions) are not followed, so every path
            // here stays under the real root and is compared as it is.
            if meta.file_type().is_symlink()
                || is_hidden(&name, &meta)
                || private.iter().any(|p| entry.path().starts_with(p))
            {
                continue;
            }
            if relpath::component_problem(&name).is_some() {
                found.skipped += 1;
                continue;
            }
            let mut path = under.to_vec();
            path.push(name);
            if meta.is_dir() {
                if path.len() >= relpath::MAX_DEPTH {
                    found.truncated = true;
                    return;
                }
                walk(&entry.path(), &path, private, found, bytes, entries_left);
            } else if meta.is_file() {
                if meta.len() > MAX_FILE_BYTES {
                    found.skipped += 1;
                    continue;
                }
                if found.files.len() >= MAX_FOLDER_FILES || *bytes + meta.len() > MAX_FOLDER_BYTES {
                    found.truncated = true;
                    return;
                }
                *bytes += meta.len();
                found.files.push(FolderFile {
                    full: entry.path(),
                    path,
                    size: meta.len(),
                });
            }
        }
    }
    let real = root
        .canonicalize()
        .ok()
        .filter(|r| r.is_dir())
        .ok_or("not a folder")?;
    if private.iter().any(|p| real.starts_with(p)) {
        return Err("the server's own folder".to_string());
    }
    let mut found = FolderFiles::default();
    let mut bytes = 0;
    let mut entries_left = max_entries;
    walk(
        &real,
        &[],
        private,
        &mut found,
        &mut bytes,
        &mut entries_left,
    );
    // Offered under the root as the owner named it, not the long form
    // Windows resolves it to.
    for file in &mut found.files {
        file.full = file.path.iter().fold(root.to_path_buf(), |p, n| p.join(n));
    }
    Ok(found)
}

/// What the sent list says a folder send left out, if it left out anything.
fn left_out_note(found: &FolderFiles) -> Option<String> {
    match (found.skipped, found.truncated) {
        (0, false) => None,
        (0, true) => Some("cut short at the folder limits".to_string()),
        (n, false) => Some(format!("{n} left out: names the phone cannot take")),
        (n, true) => Some(format!(
            "{n} left out: names the phone cannot take; cut short at the folder limits"
        )),
    }
}

/// Offer every file under the folder `root` to the request's phone (T6b),
/// each as its own offer that names the send, the folder and the file's
/// place in it. The sent list has one row for the folder, which ends when
/// every file has.
async fn deliver_folder(
    dir: &Path,
    request: SendRequest,
    root: PathBuf,
    clients: &[Arc<dyn DeviceSender>],
    store: Option<&TrustStore>,
) {
    let target = phone_for(&request, clients, store).map_err(str::to_string);
    let _ = send_folder(dir, &request, &root, target.as_deref(), None).await;
}

/// The phone `asker` asked for the folder `root` from the remote browser,
/// naming it `asked` (the path it browsed to): offer it the folder as the
/// owner's sends are, with a row in the sent list, and answer first. A
/// phone with [`MAX_ASKED_FOLDERS`] on the way is told it is busy.
pub(crate) async fn offer_folder_to(
    dir: &Path,
    device_id: &str,
    root: &Path,
    asked: &str,
    asker: &dyn DeviceSender,
) -> Result<(), String> {
    let request = SendRequest {
        id: uuid::Uuid::new_v4().to_string(),
        device_id: device_id.to_string(),
        path: root.display().to_string(),
        created_unix: now_unix(),
    };
    let reply = Reply { asked, to: asker };
    send_folder(dir, &request, root, Ok(asker), Some(reply)).await
}

/// Where a phone's request for a folder is answered.
#[derive(Clone, Copy)]
struct Reply<'a> {
    asked: &'a str,
    to: &'a dyn DeviceSender,
}

impl Reply<'_> {
    async fn send(&self, outcome: &Result<(), String>) {
        answer_folder_request(self.to, self.asked, outcome).await;
    }
}

/// Tell the phone `to` how its request for the folder `asked` went.
pub(crate) async fn answer_folder_request(
    to: &dyn DeviceSender,
    asked: &str,
    outcome: &Result<(), String>,
) {
    if let Err(reason) = outcome {
        tracing::info!("Folder request from {} refused: {reason}", to.device_id());
    }
    let packet = NetworkPacket::new(FOLDER_FETCH_REPLY).with_body(serde_json::json!({
        "path": asked,
        "queued": outcome.is_ok(),
        "reason": outcome.as_ref().err(),
    }));
    if let Err(e) = to.send_packet(&packet).await {
        tracing::debug!("Could not answer a folder request: {e}");
    }
}

/// Offer every file under `root` to `target` as the folder send `request`
/// (T6b); its outcome. A phone's own request (`reply`) is answered before
/// any offer, is held to [`MAX_ASKED_FOLDERS`], and gets no sent row when
/// refused as busy.
async fn send_folder(
    dir: &Path,
    request: &SendRequest,
    root: &Path,
    target: Result<&dyn DeviceSender, &String>,
    reply: Option<Reply<'_>>,
) -> Result<(), String> {
    if let Some(reply) = reply
        && !may_ask_again(&request.device_id, Instant::now())
    {
        let wait = Err("asked for too many folders: wait a minute".to_string());
        reply.send(&wait).await;
        return wait;
    }
    let limit = reply.map(|_| MAX_ASKED_FOLDERS);
    let Some(_reserved) = Reservation::take(&request.id, &request.device_id, limit) else {
        let busy = Err("busy: two folders are already on the way to this phone".to_string());
        if let Some(reply) = reply {
            reply.send(&busy).await;
        }
        return busy;
    };
    offer_folder(dir, request, root, target, reply).await
}

/// [`send_folder`] once the send is reserved.
async fn offer_folder(
    dir: &Path,
    request: &SendRequest,
    root: &Path,
    target: Result<&dyn DeviceSender, &String>,
    reply: Option<Reply<'_>>,
) -> Result<(), String> {
    let folder = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Folder".to_string());
    // The phone takes a send's id as 32 hex digits; the outbox's has hyphens.
    let folder_id = request.id.replace('-', "");
    // A walk of thousands of entries is disk work: off the runtime thread,
    // which also serves the asking phone's control connection.
    let found = {
        let root = root.to_path_buf();
        tokio::task::spawn_blocking(move || folder_files(&root))
            .await
            .unwrap_or_else(|e| Err(format!("the folder could not be read: {e}")))
    };
    // A folder whose own name the phone cannot make is refused, not sent
    // flat or renamed.
    let name_problem = pocketdesk_core::protocol::relpath::component_problem(&folder);
    let checked = match &found {
        _ if name_problem.is_some() => Err(format!(
            "the folder's name: {}",
            name_problem.clone().unwrap_or_default()
        )),
        Ok(found) if found.files.is_empty() => Err("the folder has nothing to send".to_string()),
        Ok(_) => target.map_err(String::clone),
        Err(reason) => Err(reason.clone()),
    };
    let total: u64 = found
        .as_ref()
        .map_or(0, |f| f.files.iter().map(|f| f.size).sum());
    // The owner's sends are listed either way; a phone's own request only
    // once it is going, so refused ones cannot push out the owner's history.
    if reply.is_none() || checked.is_ok() {
        let _guard = sent_lock().lock().unwrap_or_else(|e| e.into_inner());
        let row = SentFile {
            id: request.id.clone(),
            name: format!("{folder}/"),
            path: request.path.clone(),
            size: total,
            device_id: request.device_id.clone(),
            sent_unix: now_unix(),
            state: match &checked {
                Ok(_) => SentState::Offered,
                Err(reason) => SentState::Failed(reason.clone()),
            },
            note: found.as_ref().ok().and_then(left_out_note),
        };
        if let Err(e) = sent::record(dir, row) {
            tracing::warn!("Could not record the sent folder: {e}");
        }
    }
    let (found, target) = match (found, checked) {
        (Ok(found), Ok(target)) => (found, target),
        (_, Err(reason)) | (Err(reason), _) => {
            let refused = Err(reason);
            if let Some(reply) = reply {
                reply.send(&refused).await;
            }
            return refused;
        }
    };
    if let Some(reply) = reply {
        reply.send(&Ok(())).await;
    }
    if found.skipped > 0 || found.truncated {
        tracing::warn!(
            "Folder {folder}: {} names left out, cut short: {}",
            found.skipped,
            found.truncated
        );
    }
    let ids: Vec<String> = found
        .files
        .iter()
        .map(|_| uuid::Uuid::new_v4().simple().to_string())
        .collect();
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    start_group(&request.id, &id_refs);
    let offered: Vec<(String, PathBuf)> = ids
        .iter()
        .cloned()
        .zip(found.files.iter().map(|f| f.full.clone()))
        .collect();
    // A folder the phone asked for keeps the browser's rules until each
    // file is fetched.
    let within = reply.and_then(|_| root.canonicalize().ok());
    offer_folder_at(
        &offered,
        &request.device_id,
        within.as_deref(),
        Instant::now(),
    );
    for (file, id) in found.files.iter().zip(&ids) {
        let packet = NetworkPacket::new(OFFER).with_body(serde_json::json!({
            "id": id,
            "name": file.path.last(),
            "size": file.size,
            "source": source_of(&file.full, file.size),
            "folderId": folder_id,
            "folder": folder,
            "path": file.path,
        }));
        if target.send_packet(&packet).await.is_err() {
            // The phone left: every file not yet offered ends here.
            for id in &ids {
                if offers()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(id)
                    .is_some()
                {
                    end_offer(dir, id, Err("phone not connected".to_string()));
                }
            }
            // Its offers have ended, and so has the send.
            return Ok(());
        }
    }
    tracing::info!(
        "Offered folder {} ({} files) to {}",
        root.display(),
        found.files.len(),
        request.device_id
    );
    Ok(())
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
        format!("{}\n{size}\n{modified}", canonical.display()).as_bytes(),
    );
    digest.as_ref()[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Whether `request` may be offered: the file, the phone's permission, and
/// a live connection to it. The connection and the file's size.
/// Whether `path` is one of the server's own files: inside its state or
/// config folder (`private`, real paths), by the real path.
fn is_servers_own(path: &Path, private: &[PathBuf]) -> bool {
    path.canonicalize()
        .is_ok_and(|real| private.iter().any(|p| real.starts_with(p)))
}

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
    Ok((phone_for(request, clients, store)?, size))
}

/// The request's phone, if it may take files and is connected.
fn phone_for(
    request: &SendRequest,
    clients: &[Arc<dyn DeviceSender>],
    store: Option<&TrustStore>,
) -> Result<Arc<dyn DeviceSender>, &'static str> {
    // An unreadable store could be hiding a denial: refuse.
    let allowed = store.is_some_and(|s| {
        s.is_trusted(&request.device_id)
            && s.permissions(&request.device_id).allows(Capability::Files)
    });
    if !allowed {
        return Err("files are off for this phone");
    }
    target_for(&request.device_id, clients).ok_or("phone not connected")
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
        assert_eq!(settle_in(&table, "o-1", false, now), Settled::Retry);
        assert!(
            redeem_in(&table, "o-1", "phone-a", now).is_ok(),
            "a failed attempt leaves it"
        );
        assert_eq!(settle_in(&table, "o-1", true, now), Settled::Done);
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
    fn a_folder_offers_its_files_with_their_paths_and_no_hidden_ones() {
        let root = temp_dir("folder-walk").join("Holiday");
        std::fs::create_dir_all(root.join("day 1").join("raw")).unwrap();
        std::fs::write(root.join("notes.txt"), b"n").unwrap();
        std::fs::write(root.join(".DS_Store"), b"h").unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git").join("x"), b"h").unwrap();
        std::fs::write(root.join("day 1").join("a.jpg"), b"aa").unwrap();
        std::fs::write(root.join("day 1").join("raw").join("b.raw"), b"bbb").unwrap();
        let found = folder_files_with(&root, &[]).unwrap();
        let paths: Vec<Vec<String>> = found.files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                vec!["day 1".to_string(), "a.jpg".to_string()],
                vec!["day 1".to_string(), "raw".to_string(), "b.raw".to_string()],
                vec!["notes.txt".to_string()],
            ]
        );
        assert_eq!(found.files.iter().map(|f| f.size).sum::<u64>(), 6);
        assert!(!found.truncated);
    }

    #[tokio::test]
    async fn a_folder_send_says_what_it_left_out() {
        let dir = temp_dir("deliver-folder-note");
        let root = dir.join("Notes");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), b"a").unwrap();
        // A name the phone may not make: an invisible character in it.
        std::fs::write(root.join("b\u{200B}.txt"), b"b").unwrap();
        let store = trusting(&dir, "phone-a", true);
        let tcp = Conn::new("phone-a", true);
        let clients: [Arc<dyn DeviceSender>; 1] = [tcp.clone()];
        deliver(&dir, request(&dir, &root), &clients, Some(&store)).await;
        let listed = sent::read(&dir);
        let note = listed[0].note.as_deref().expect("a note");
        assert!(note.starts_with("1 "), "{note}");
    }

    fn folder_of_two(dir: &Path, name: &str) -> PathBuf {
        let root = dir.join(name);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.txt"), b"a").unwrap();
        std::fs::write(root.join("sub").join("b.txt"), b"bb").unwrap();
        root
    }

    #[tokio::test]
    async fn an_asked_folders_offers_remember_where_they_must_stay() {
        let dir = temp_dir("asked-within");
        let root = folder_of_two(&dir, "Kept");
        let phone = Conn::new("phone-within", true);
        offer_folder_to(&dir, "phone-within", &root, "Kept", &*phone)
            .await
            .unwrap();
        let first = phone.sent.lock().unwrap()[1].body["id"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(asked_within(&first), Some(root.canonicalize().unwrap()));
        // The owner's own sends are not held to the browser's rules.
        let owner = Conn::new("phone-owner-within", true);
        let store = trusting(&dir, "phone-owner-within", true);
        let clients: [Arc<dyn DeviceSender>; 1] = [owner.clone()];
        let mut request = request(&dir, &root);
        request.device_id = "phone-owner-within".to_string();
        deliver(&dir, request, &clients, Some(&store)).await;
        let id = owner.sent.lock().unwrap()[0].body["id"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(asked_within(&id), None);
    }

    #[tokio::test]
    async fn a_folder_the_phone_asks_for_is_answered_then_offered_to_it() {
        let dir = temp_dir("asked-folder");
        let root = folder_of_two(&dir, "Notes");
        let phone = Conn::new("phone-asks", true);
        offer_folder_to(&dir, "phone-asks", &root, "Docs/Notes", &*phone)
            .await
            .unwrap();
        let pushed = phone.sent.lock().unwrap().clone();
        // The answer comes first: thousands of offers must not bury it.
        assert_eq!(pushed[0].packet_type, FOLDER_FETCH_REPLY);
        assert_eq!(pushed[0].body["path"], "Docs/Notes");
        assert_eq!(pushed[0].body["queued"], true);
        let offers: Vec<_> = pushed[1..]
            .iter()
            .filter(|p| p.packet_type == OFFER)
            .collect();
        assert_eq!(offers.len(), 2);
        assert_eq!(offers[0].body["folder"], "Notes");
        let listed = sent::read(&dir);
        assert_eq!(listed.len(), 1, "one row, so the owner sees what left");
        assert_eq!(listed[0].device_id, "phone-asks");
        assert_eq!(listed[0].name, "Notes/");
    }

    #[tokio::test]
    async fn a_phone_may_have_two_folders_on_the_way_and_no_more() {
        let dir = temp_dir("asked-busy");
        let phone = Conn::new("phone-busy", true);
        for n in 0..2 {
            let root = folder_of_two(&dir, &format!("F{n}"));
            offer_folder_to(&dir, "phone-busy", &root, "x", &*phone)
                .await
                .unwrap();
        }
        let third = folder_of_two(&dir, "F2");
        let refused = offer_folder_to(&dir, "phone-busy", &third, "F2", &*phone).await;
        assert!(refused.unwrap_err().contains("busy"));
        let last = phone.sent.lock().unwrap().last().cloned().unwrap();
        assert_eq!(last.packet_type, FOLDER_FETCH_REPLY);
        assert_eq!(last.body["queued"], false);
        assert_eq!(sent::read(&dir).len(), 2, "no row for a refused request");
        // Another phone is not held up by this one.
        let other = Conn::new("phone-other", true);
        offer_folder_to(&dir, "phone-other", &third, "F2", &*other)
            .await
            .unwrap();
    }

    #[test]
    fn a_reservation_dropped_before_its_folder_starts_frees_the_phone() {
        for n in 0..3 {
            // Each dropped as if its request's future were cancelled.
            let held = Reservation::take(&format!("cancelled-{n}"), "phone-cancel", Some(2));
            assert!(held.is_some(), "reservation {n}");
        }
        let started = Reservation::take("started", "phone-cancel", Some(2)).unwrap();
        start_group("started", &["started-offer"]);
        drop(started);
        assert!(Reservation::take("next", "phone-cancel", Some(2)).is_some());
    }

    #[test]
    fn the_owners_sends_do_not_count_against_the_phones_own_requests() {
        let _a = Reservation::take("owner-a", "phone-owner", None).unwrap();
        let _b = Reservation::take("owner-b", "phone-owner", None).unwrap();
        start_group("owner-a", &["owner-a-1"]);
        start_group("owner-b", &["owner-b-1"]);
        assert!(Reservation::take("asked", "phone-owner", Some(2)).is_some());
    }

    #[tokio::test]
    async fn an_empty_folder_the_phone_asks_for_is_answered_with_why() {
        let dir = temp_dir("asked-empty");
        let root = dir.join("Empty");
        std::fs::create_dir_all(&root).unwrap();
        let phone = Conn::new("phone-empty", true);
        let refused = offer_folder_to(&dir, "phone-empty", &root, "Empty", &*phone).await;
        assert!(refused.is_err());
        let pushed = phone.sent.lock().unwrap().clone();
        assert_eq!(pushed.len(), 1);
        assert_eq!(pushed[0].body["queued"], false);
        assert!(
            pushed[0].body["reason"]
                .as_str()
                .unwrap()
                .contains("nothing")
        );
        assert!(
            sent::read(&dir).is_empty(),
            "a refused request leaves the owner's sent list alone"
        );
    }

    #[tokio::test]
    async fn a_phone_asking_again_and_again_is_told_to_wait() {
        let dir = temp_dir("asked-often");
        let root = dir.join("Empty");
        std::fs::create_dir_all(&root).unwrap();
        let phone = Conn::new("phone-often", true);
        for _ in 0..MAX_ASKED_PER_MINUTE {
            let refused = offer_folder_to(&dir, "phone-often", &root, "Empty", &*phone).await;
            assert!(refused.unwrap_err().contains("nothing"));
        }
        let refused = offer_folder_to(&dir, "phone-often", &root, "Empty", &*phone).await;
        assert!(refused.unwrap_err().contains("wait"));
        let last = phone.sent.lock().unwrap().last().cloned().unwrap();
        assert_eq!(last.body["queued"], false);
    }

    #[test]
    fn a_walk_stops_at_its_entries_whatever_they_are() {
        // Empty folders and skipped names are walked too: they count.
        let root = temp_dir("folder-entries").join("Wide");
        for n in 0..5 {
            std::fs::create_dir_all(root.join(format!("d{n}"))).unwrap();
        }
        std::fs::write(root.join("z.txt"), b"z").unwrap();
        let found = folder_files_capped(&root, &[], 3).unwrap();
        assert!(found.truncated);
        assert!(found.files.is_empty());
        let all = folder_files_capped(&root, &[], 100).unwrap();
        assert!(!all.truncated);
        assert_eq!(all.files.len(), 1);
    }

    #[test]
    fn the_servers_own_files_are_never_sent_alone_either() {
        let dir = temp_dir("own-file");
        let state = dir.join("state");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join("identity.key"), b"k").unwrap();
        std::fs::write(dir.join("notes.txt"), b"n").unwrap();
        let private = [state.canonicalize().unwrap()];
        assert!(is_servers_own(&state.join("identity.key"), &private));
        assert!(!is_servers_own(&dir.join("notes.txt"), &private));
        assert!(!is_servers_own(&dir.join("missing.txt"), &private));
    }

    #[test]
    fn a_folder_leaves_out_the_servers_own_files() {
        let home = temp_dir("folder-private").join("home");
        let state = home.join("AppData").join("pocketdesk");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join("identity.key"), b"k").unwrap();
        std::fs::write(home.join("notes.txt"), b"n").unwrap();
        let private = [state.canonicalize().unwrap()];
        let found = folder_files_with(&home, &private).unwrap();
        let paths: Vec<Vec<String>> = found.files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(paths, vec![vec!["notes.txt".to_string()]]);
        // A folder inside them is refused whole.
        assert!(folder_files_with(&state, &private).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn a_folder_leaves_out_what_windows_marks_hidden() {
        use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_HIDDEN, SetFileAttributesW};
        use windows::core::HSTRING;
        let root = temp_dir("folder-winhidden").join("Docs");
        std::fs::create_dir_all(root.join("AppData")).unwrap();
        std::fs::write(root.join("AppData").join("secret"), b"s").unwrap();
        std::fs::write(root.join("desktop.ini"), b"h").unwrap();
        std::fs::write(root.join("a.txt"), b"a").unwrap();
        for hidden in [root.join("AppData"), root.join("desktop.ini")] {
            // SAFETY: a plain call with a valid, NUL-terminated path.
            unsafe {
                SetFileAttributesW(&HSTRING::from(hidden.as_os_str()), FILE_ATTRIBUTE_HIDDEN)
            }
            .unwrap();
        }
        let found = folder_files_with(&root, &[]).unwrap();
        let paths: Vec<Vec<String>> = found.files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(paths, vec![vec!["a.txt".to_string()]]);
    }

    #[test]
    fn a_folder_send_ends_when_every_file_has() {
        let dir = temp_dir("folder-group");
        sent::record(
            &dir,
            SentFile {
                id: "folder-1".to_string(),
                name: "Holiday".to_string(),
                path: "/Holiday".to_string(),
                size: 3,
                device_id: "phone-a".to_string(),
                sent_unix: 1,
                state: SentState::Offered,
                note: None,
            },
        )
        .unwrap();
        start_group("folder-1", &["f-1", "f-2", "f-3"]);
        end_offer(&dir, "f-1", Ok(()));
        end_offer(&dir, "f-2", Err("no room".to_string()));
        assert_eq!(
            sent::read(&dir)[0].state,
            SentState::Offered,
            "one file still to go"
        );
        end_offer(&dir, "f-3", Ok(()));
        assert_eq!(
            sent::read(&dir)[0].state,
            SentState::Failed("1 of 3 files not sent: no room".to_string())
        );
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
            let settled = settle_in(&table, "o-4", false, now);
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
    fn a_folders_offers_last_while_the_phone_keeps_fetching_them() {
        let table = Table::default();
        let now = Instant::now();
        let files = [
            ("f-1".to_string(), PathBuf::from("/t/a")),
            ("f-2".to_string(), PathBuf::from("/t/b")),
        ];
        offer_folder_in(&table, &files, "phone-a", None, now);
        // The first file is fetched late, and takes a long time.
        let begun = now + OFFER_TTL - Duration::from_secs(60);
        assert!(redeem_in(&table, "f-1", "phone-a", begun).is_ok());
        let saved = begun + OFFER_TTL + Duration::from_secs(60);
        assert_eq!(settle_in(&table, "f-1", true, saved), Settled::Done);
        // The second is still there for the phone, long past its own TTL...
        let next = saved + OFFER_TTL - Duration::from_secs(1);
        assert!(sweep_in(&table, next).is_empty());
        assert!(redeem_in(&table, "f-2", "phone-a", next).is_ok());
        // ...and ends once the phone stops for a whole TTL.
        let idle = next + OFFER_TTL + Duration::from_secs(1);
        assert_eq!(sweep_in(&table, idle), ["f-2"]);
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
            note: None,
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
    async fn a_folder_is_offered_file_by_file_with_each_files_place() {
        let dir = temp_dir("deliver-folder");
        let root = dir.join("Trip");
        std::fs::create_dir_all(root.join("day 2")).unwrap();
        std::fs::write(root.join("map.png"), b"m").unwrap();
        std::fs::write(root.join("day 2").join("a.jpg"), b"aa").unwrap();
        let store = trusting(&dir, "phone-a", true);
        let tcp = Conn::new("phone-a", true);
        let clients: [Arc<dyn DeviceSender>; 1] = [tcp.clone()];
        let sent_request = request(&dir, &root);
        deliver(&dir, sent_request.clone(), &clients, Some(&store)).await;

        let pushed = tcp.sent.lock().unwrap().clone();
        assert_eq!(pushed.len(), 2);
        // The id the phone takes: 32 hex digits, whatever the outbox's is.
        let folder_id = pushed[0].body["folderId"].as_str().unwrap();
        assert_eq!(folder_id.len(), 32, "{folder_id}");
        assert!(folder_id.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(folder_id, sent_request.id.replace('-', ""));
        assert_eq!(pushed[0].body["folder"], "Trip");
        assert_eq!(
            pushed[0].body["path"],
            serde_json::json!(["day 2", "a.jpg"])
        );
        assert_eq!(pushed[0].body["name"], "a.jpg");
        assert_eq!(pushed[1].body["path"], serde_json::json!(["map.png"]));
        assert_ne!(
            pushed[0].body["id"], pushed[1].body["id"],
            "each file its own offer"
        );
        let listed = sent::read(&dir);
        assert_eq!(listed.len(), 1, "one row for the folder");
        assert_eq!(listed[0].name, "Trip/");
        assert_eq!(listed[0].size, 3);
        let first = pushed[0].body["id"].as_str().unwrap().to_string();
        assert_eq!(
            redeem(&first, "phone-a"),
            Ok(root.join("day 2").join("a.jpg"))
        );
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
        // A folder is sent file by file (T6b); one with nothing in it is not.
        let empty = dir.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        for (path, clients, store, why) in [
            (file.clone(), &[][..], Some(&allowed), "phone not connected"),
            (
                empty.clone(),
                &connected[..],
                Some(&allowed),
                "the folder has nothing to send",
            ),
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
