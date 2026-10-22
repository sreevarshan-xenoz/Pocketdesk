//! The phone's folders as a drive (D8): the folders phones share (T10g),
//! served as WebDAV on loopback behind a token, with `phone_drive` on;
//! with `phone_drive_write` on, files and folders go into those a phone's
//! owner marked (N6), never over what is there. A folder is listed by asking its phone (T10g's
//! `list`), kept 30 s; a file is fetched by asking its phone to send it
//! (T10g's `fetch` with `drive`), into `<state>/drive_cache/`, served, and
//! removed. The answers are in [`answer`], over the [`Phones`] the server
//! has ([`live::LivePhones`]) or a test's stand-in.

pub(crate) mod dav;
pub(crate) mod live;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pocketdesk_core::protocol::phone_files::{
    EXISTS, Entry, Listing, MAX_PUT_BYTES, addable_name, valid_name,
};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use dav::Prop;

/// How long a listing is kept: a file manager looks again and again.
pub const LISTING_KEEP: Duration = Duration::from_secs(30);

/// How long a fetched file is kept (D8 review): a file manager reads it
/// again, or in pieces, and each would cross from the phone again.
pub const FILE_KEEP: Duration = Duration::from_secs(60);

/// Files fetched from phones at once, at most: the phone sends one at a
/// time, so more only wait, each holding a connection.
pub const FETCHES_AT_ONCE: usize = 2;

/// How long a file copied in waits for its phone to take it before the
/// copy is answered as still under way (N6).
pub const PUT_WAIT: Duration = Duration::from_secs(45);

/// How long an empty file is held here for its content (N6): Windows makes
/// a file empty, then writes it on close.
pub const STUB_KEEP: Duration = Duration::from_secs(120);

/// How long each piece of a copied file may take to arrive.
const BODY_WAIT: Duration = Duration::from_secs(30);

/// Empty files held at once, at most (N6 review).
const MAX_STUBS: usize = 64;

/// How a phone took a file the drive gave it (N6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Put {
    /// Saved in its folder.
    Saved,
    /// Still on its way: it carries on, and a failure is said later.
    UnderWay,
    /// The phone would not take it, and why.
    Refused(String),
    /// Not room for it on the phone.
    NoRoom(String),
}

/// Why the drive could not answer from a phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fail {
    /// Not there (or the phone said so): 404.
    NotFound,
    /// The phone is not connected, or Files is off: 503.
    Unreachable,
    /// The phone did not answer in time: 504.
    Timeout,
    /// The phone answered, but could not give it: 502.
    Phone,
}

impl Fail {
    fn status(self) -> u16 {
        match self {
            Fail::NotFound => 404,
            Fail::Unreachable => 503,
            Fail::Timeout => 504,
            Fail::Phone => 502,
        }
    }
}

/// What the drive asks of phones.
#[async_trait::async_trait]
pub trait Phones: Send + Sync {
    /// The paired phones with Files on: id and the name they are shown by.
    fn phones(&self) -> Vec<(String, String)>;
    /// A folder's listing; with no `root`, only the roots matter.
    async fn list(&self, device: &str, root: Option<u32>, path: &[String])
    -> Result<Listing, Fail>;
    /// A file, fetched whole into the cache; its place there.
    async fn fetch(&self, device: &str, root: u32, path: &[String]) -> Result<PathBuf, Fail>;
    /// Make the folder `path` (N6): its listing, or one whose error says why
    /// ([`EXISTS`] when the name is taken).
    async fn mkdir(&self, device: &str, root: u32, path: &[String]) -> Result<Listing, Fail>;
    /// Tell the owner that `name`, copied onto the drive, did not reach the
    /// phone, and why (N6): no one waited for the answer.
    fn tell(&self, _name: &str, _why: &str) {}
    /// Give `file` (of `size` bytes) to the phone, into `folder` as `name`
    /// (N6). The file is the phone's to remove ([`remove_upload`]) from here
    /// on, however it ends.
    async fn put(
        &self,
        device: &str,
        root: u32,
        folder: &[String],
        name: &str,
        file: PathBuf,
        size: u64,
    ) -> Result<Put, Fail>;
}

/// Each phone as a folder: its name, with its short id when two share one.
pub fn phone_folders(phones: &[(String, String)]) -> Vec<(String, String)> {
    let shown = |name: &str| clean(name);
    phones
        .iter()
        .map(|(id, name)| {
            let name = shown(name);
            let shared = phones
                .iter()
                .filter(|(_, other)| shown(other) == name)
                .count()
                > 1;
            let short: String = id.chars().take(8).collect();
            let folder = if name.is_empty() {
                format!("Phone {short}")
            } else if shared {
                format!("{name} ({short})")
            } else {
                name
            };
            (folder, id.clone())
        })
        .collect()
}

/// A shared folder's name as a folder: its name, with its id when two share
/// one.
pub fn root_folders(listing: &Listing) -> Vec<(String, u32)> {
    listing
        .roots
        .iter()
        .map(|root| {
            let shared = listing.roots.iter().filter(|r| r.name == root.name).count() > 1;
            let name = if shared {
                format!("{} ({})", root.name, root.id)
            } else {
                root.name.clone()
            };
            (name, root.id)
        })
        .collect()
}

/// A name safe as one part of a path: separators and controls out.
fn clean(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control() && *c != '/' && *c != '\\')
        .collect::<String>()
        .trim()
        .to_string()
}

/// What a path names.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Top,
    Phone(String),
    /// A folder: its phone, root and path below it.
    Dir(String, u32, Vec<String>),
    /// A file, as its folder's listing describes it.
    File(String, u32, Vec<String>, Entry),
}

/// A place in a phone's folder: its phone, root and path, name included.
type Place = (String, u32, Vec<String>);

/// The drive: its token, its phones, and the listings it keeps.
pub struct Drive {
    token: String,
    phones: Arc<dyn Phones>,
    kept: Mutex<HashMap<Asked, (Instant, Listing)>>,
    /// Files fetched, for [`FILE_KEEP`]: when, where.
    fetched: Mutex<HashMap<Asked, (Instant, PathBuf)>>,
    file_keep: Duration,
    fetching: tokio::sync::Semaphore,
    /// Whether files and folders may be added (N6).
    write: AtomicBool,
    /// Where files copied in wait for their phone (N6); none takes none.
    uploads: Option<PathBuf>,
    /// Empty files held for their content (N6): since when.
    stubs: Mutex<HashMap<Place, Instant>>,
    stub_keep: Duration,
}

/// A listing asked for: its phone, root (none for the roots alone) and path.
type Asked = (String, Option<u32>, Vec<String>);

impl Drive {
    pub fn new(token: String, phones: Arc<dyn Phones>) -> Self {
        Self {
            token,
            phones,
            kept: Mutex::new(HashMap::new()),
            fetched: Mutex::new(HashMap::new()),
            file_keep: FILE_KEEP,
            fetching: tokio::sync::Semaphore::new(FETCHES_AT_ONCE),
            write: AtomicBool::new(false),
            uploads: None,
            stubs: Mutex::new(HashMap::new()),
            stub_keep: STUB_KEEP,
        }
    }

    /// The same, taking files into `dir` while they wait for their phone
    /// (N6).
    pub fn taking_files_into(mut self, dir: PathBuf) -> Self {
        self.uploads = Some(dir);
        self
    }

    /// Let files and folders be added, or not (`phone_drive_write`, N6).
    pub fn set_write(&self, on: bool) {
        self.write.store(on, Ordering::Relaxed);
    }

    pub(crate) fn writes(&self) -> bool {
        self.write.load(Ordering::Relaxed)
    }

    /// The same, holding empty files `keep` (tests).
    #[cfg(test)]
    fn holding_stubs(mut self, keep: Duration) -> Self {
        self.stub_keep = keep;
        self
    }

    /// Where a write to `parts` goes (N6): a folder below a root its phone's
    /// owner marked, and a name shown in listings; or the status refusing
    /// it. A folder above that is not there is a conflict, as WebDAV has it.
    async fn write_target(&self, parts: &[String]) -> Result<Place, u16> {
        if !self.writes() {
            return Err(405);
        }
        let Some((name, parent)) = parts.split_last() else {
            return Err(403);
        };
        // Only below a root: never a phone or a root itself.
        if parent.len() < 2 || !addable_name(name) {
            return Err(403);
        }
        let (device, root, folder) = match self.resolve(parent).await {
            Ok(Node::Dir(device, root, folder)) => (device, root, folder),
            Ok(_) | Err(Fail::NotFound) => return Err(409),
            Err(fail) => return Err(fail.status()),
        };
        let roots = self
            .listing(&device, None, &[])
            .await
            .map_err(Fail::status)?;
        if !roots.roots.iter().any(|r| r.id == root && r.add) {
            return Err(403);
        }
        let mut path = folder;
        path.push(name.clone());
        Ok((device, root, path))
    }

    /// Forget what is kept of the folder `place` is in, so its next look
    /// shows what was added (N6).
    fn forget_folder(&self, place: &Place) {
        let (device, root, path) = place;
        let folder = path[..path.len().saturating_sub(1)].to_vec();
        self.kept
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(device.clone(), Some(*root), folder));
    }

    /// The empty files held in `device`'s folder `folder` of `root` (N6).
    fn stubs_in(&self, device: &str, root: u32, folder: &[String]) -> Vec<String> {
        self.stubs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .filter(|(d, r, p)| {
                d == device && *r == root && p.len() == folder.len() + 1 && p.starts_with(folder)
            })
            .filter_map(|(_, _, p)| p.last().cloned())
            .collect()
    }

    /// A lock or times set on an empty file held here: Windows is still at
    /// work on it, so it is held longer (N6 review).
    fn touch_stub(&self, place: &Place) {
        if let Some(at) = self
            .stubs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(place)
        {
            *at = Instant::now();
        }
    }

    fn is_stub(&self, place: &Place) -> bool {
        self.stubs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(place)
    }

    /// Give each empty file held past [`STUB_KEEP`] to its phone as it is:
    /// nothing came to write in it (N6).
    pub fn deliver_stale_stubs(&self) {
        let Some(dir) = &self.uploads else {
            return;
        };
        let stale: Vec<Place> = {
            let mut stubs = self.stubs.lock().unwrap_or_else(|e| e.into_inner());
            let stale: Vec<Place> = stubs
                .iter()
                .filter(|(_, at)| at.elapsed() >= self.stub_keep)
                .map(|(place, _)| place.clone())
                .collect();
            for place in &stale {
                stubs.remove(place);
            }
            stale
        };
        for place in stale {
            let name = place.2.last().cloned().unwrap_or_default();
            let Ok(file) = new_upload(dir, &name) else {
                continue;
            };
            self.forget_folder(&place);
            let phones = self.phones.clone();
            tokio::spawn(async move {
                let (device, root, path) = place;
                if let Some((name, folder)) = path.split_last() {
                    let _ = phones.put(&device, root, folder, name, file.0, 0).await;
                }
            });
        }
    }

    /// The same, keeping fetched files `keep` (tests).
    #[cfg(test)]
    fn keeping_files(mut self, keep: Duration) -> Self {
        self.file_keep = keep;
        self
    }

    /// A file, fetched from its phone or kept from a fetch a moment ago.
    async fn file(&self, device: &str, root: u32, path: &[String]) -> Result<PathBuf, u16> {
        let key = (device.to_string(), Some(root), path.to_vec());
        {
            let mut fetched = self.fetched.lock().unwrap_or_else(|e| e.into_inner());
            fetched.retain(|_, (at, file)| {
                let keep = at.elapsed() < self.file_keep;
                if !keep {
                    let _ = std::fs::remove_file(file);
                }
                keep
            });
            if let Some((_, file)) = fetched.get(&key)
                && file.exists()
            {
                return Ok(file.clone());
            }
        }
        // More than the phone can send at once: asked again later.
        let Ok(_slot) = self.fetching.try_acquire() else {
            return Err(503);
        };
        let file = self
            .phones
            .fetch(device, root, path)
            .await
            .map_err(Fail::status)?;
        let mut fetched = self.fetched.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, old)) = fetched.insert(key, (Instant::now(), file.clone()))
            && old != file
        {
            let _ = std::fs::remove_file(old);
        }
        Ok(file)
    }

    /// A listing, kept for [`LISTING_KEEP`].
    async fn listing(
        &self,
        device: &str,
        root: Option<u32>,
        path: &[String],
    ) -> Result<Listing, Fail> {
        let key = (device.to_string(), root, path.to_vec());
        if let Some((at, listing)) = self
            .kept
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            && at.elapsed() < LISTING_KEEP
        {
            return Ok(listing.clone());
        }
        let listing = self.phones.list(device, root, path).await?;
        let mut kept = self.kept.lock().unwrap_or_else(|e| e.into_inner());
        kept.retain(|_, (at, _)| at.elapsed() < LISTING_KEEP);
        kept.insert(key, (Instant::now(), listing.clone()));
        Ok(listing)
    }

    /// The folder a listing describes, or why not: one the phone could not
    /// list is not found.
    async fn folder(&self, device: &str, root: u32, path: &[String]) -> Result<Listing, Fail> {
        let listing = self.listing(device, Some(root), path).await?;
        if listing.error.is_some() {
            return Err(Fail::NotFound);
        }
        Ok(listing)
    }

    /// What `parts` (the path after the token) names.
    async fn resolve(&self, parts: &[String]) -> Result<Node, Fail> {
        let Some((phone, rest)) = parts.split_first() else {
            return Ok(Node::Top);
        };
        let device = phone_folders(&self.phones.phones())
            .into_iter()
            .find(|(name, _)| name == phone)
            .map(|(_, id)| id)
            .ok_or(Fail::NotFound)?;
        let Some((root, path)) = rest.split_first() else {
            return Ok(Node::Phone(device));
        };
        let roots = self.listing(&device, None, &[]).await?;
        let root = root_folders(&roots)
            .into_iter()
            .find(|(name, _)| name == root)
            .map(|(_, id)| id)
            .ok_or(Fail::NotFound)?;
        let Some((name, parent)) = path.split_last() else {
            return Ok(Node::Dir(device, root, Vec::new()));
        };
        // The phone lists no hidden name, and answers nothing below one
        // (D8 review): not there, without asking.
        if !valid_name(name)
            || !parent.iter().all(|p| valid_name(p))
            || path.iter().any(|p| p.starts_with('.'))
        {
            return Err(Fail::NotFound);
        }
        // From the folder it is in: a name not there is not asked for.
        let listing = self.folder(&device, root, parent).await?;
        let entry = match listing.entries.into_iter().find(|e| &e.name == name) {
            Some(entry) => entry,
            // An empty file held here for its content (N6).
            None if self.is_stub(&(device.clone(), root, path.to_vec())) => Entry {
                name: name.clone(),
                dir: false,
                size: 0,
                modified: 0,
            },
            None => return Err(Fail::NotFound),
        };
        Ok(if entry.dir {
            Node::Dir(device, root, path.to_vec())
        } else {
            Node::File(device, root, path.to_vec(), entry)
        })
    }

    /// The resources a `PROPFIND` of `node` at `parts` describes.
    async fn props(&self, parts: &[String], node: &Node, depth: u8) -> Result<Vec<Prop>, Fail> {
        let mut full = vec![self.token.clone()];
        full.extend_from_slice(parts);
        let folder = |parts: &[String], name: &str| Prop {
            href: dav::href(parts, true),
            name: name.to_string(),
            size: None,
            modified: 0,
        };
        let name = parts
            .last()
            .cloned()
            .unwrap_or_else(|| "Phones".to_string());
        let mut props = Vec::new();
        if let Node::File(_, _, _, entry) = node {
            props.push(file_prop(&full, entry));
            return Ok(props);
        }
        props.push(folder(&full, &name));
        if depth == 0 {
            return Ok(props);
        }
        let child = |name: &str| {
            let mut parts = full.clone();
            parts.push(name.to_string());
            parts
        };
        match node {
            Node::Top => {
                for (name, _) in phone_folders(&self.phones.phones()) {
                    props.push(folder(&child(&name), &name));
                }
            }
            Node::Phone(device) => {
                let roots = self.listing(device, None, &[]).await?;
                for (name, _) in root_folders(&roots) {
                    props.push(folder(&child(&name), &name));
                }
            }
            Node::Dir(device, root, path) => {
                let entries = self.folder(device, *root, path).await?.entries;
                for name in self.stubs_in(device, *root, path) {
                    if !entries.iter().any(|e| e.name == name) {
                        let stub = Entry {
                            name: name.clone(),
                            dir: false,
                            size: 0,
                            modified: 0,
                        };
                        props.push(file_prop(&child(&name), &stub));
                    }
                }
                for entry in entries {
                    props.push(if entry.dir {
                        folder(&child(&entry.name), &entry.name)
                    } else {
                        file_prop(&child(&entry.name), &entry)
                    });
                }
            }
            Node::File(..) => {}
        }
        Ok(props)
    }
}

fn file_prop(parts: &[String], entry: &Entry) -> Prop {
    Prop {
        href: dav::href(parts, false),
        name: entry.name.clone(),
        size: Some(entry.size),
        modified: entry.modified / 1000,
    }
}

/// What a request is answered with: a head, and a body or a file.
enum Answer {
    Text(u16, Vec<(&'static str, String)>, String),
    /// A file's HEAD: its size from the listing, nothing fetched.
    Size(u64),
    /// A file's bytes from the first to before the end, of its whole size.
    File(PathBuf, u64, u64, u64),
}

const ALLOW: &str = "OPTIONS, PROPFIND, GET, HEAD";

/// What the drive takes with writing on (N6).
const ALLOW_WRITE: &str = "OPTIONS, PROPFIND, GET, HEAD, PUT, MKCOL, LOCK, UNLOCK, PROPPATCH";

fn allow(drive: &Drive) -> String {
    if drive.writes() { ALLOW_WRITE } else { ALLOW }.to_string()
}

/// A new file to take a copy named `name` into, in a folder of its own in
/// `dir` (owner-only), so the Files page names it as it is (N6 review):
/// its path and the open file.
fn new_upload(dir: &std::path::Path, name: &str) -> std::io::Result<(PathBuf, std::fs::File)> {
    pocketdesk_core::secure_fs::create_private_dir(dir)?;
    let own = dir.join(uuid::Uuid::new_v4().simple().to_string());
    std::fs::create_dir(&own)?;
    let name = if addable_name(name) { name } else { "file" };
    let path = own.join(name);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    Ok((path, file))
}

/// Remove an upload and the folder of its own it was in.
pub(crate) fn remove_upload(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    if let Some(own) = path.parent() {
        let _ = std::fs::remove_dir(own);
    }
}

/// A lock that holds nothing (N6): Windows writes only to a drive that
/// answers `LOCK`, and nothing here changes what is there.
fn lock_answer() -> Answer {
    let token = format!("opaquelocktoken:{}", uuid::Uuid::new_v4());
    Answer::Text(
        200,
        vec![
            ("Content-Type", "application/xml; charset=utf-8".to_string()),
            ("Lock-Token", format!("<{token}>")),
        ],
        dav::lock_discovery(&token),
    )
}

fn status(code: u16) -> Answer {
    Answer::Text(code, Vec::new(), String::new())
}

/// The answer to `request`.
async fn answer(drive: &Drive, request: &dav::Request) -> Answer {
    // OPTIONS says what this is, anywhere: a client asks it first.
    if request.method == "OPTIONS" {
        // Class 2 only with writing on: Windows writes to no other.
        let dav = if drive.writes() { "1, 2" } else { "1" };
        return Answer::Text(
            200,
            vec![
                ("DAV", dav.to_string()),
                ("MS-Author-Via", "DAV".to_string()),
                ("Allow", allow(drive)),
            ],
            String::new(),
        );
    }
    let Some(parts) = dav::segments(&request.path) else {
        return status(400);
    };
    let Some((token, parts)) = parts.split_first() else {
        return status(404);
    };
    if token != &drive.token {
        return status(404);
    }
    let writing = drive.writes()
        && matches!(
            request.method.as_str(),
            "MKCOL" | "LOCK" | "UNLOCK" | "PROPPATCH"
        );
    if !writing && !matches!(request.method.as_str(), "PROPFIND" | "GET" | "HEAD") {
        return Answer::Text(405, vec![("Allow", allow(drive))], String::new());
    }
    match request.method.as_str() {
        "MKCOL" => return make_folder(drive, parts).await,
        "UNLOCK" => return status(204),
        // A lock on what may be written (a new file's place included), and
        // times set on a file, taken and kept nowhere.
        "LOCK" | "PROPPATCH" => {
            let at = match drive.resolve(parts).await {
                Ok(Node::File(device, root, path, _)) => {
                    drive.touch_stub(&(device, root, path));
                    Ok(())
                }
                Ok(_) => Ok(()),
                Err(Fail::NotFound) => drive.write_target(parts).await.map(|_| ()),
                Err(fail) => Err(fail.status()),
            };
            return match at {
                Err(code) => status(code),
                Ok(()) if request.method == "LOCK" => lock_answer(),
                Ok(()) => {
                    let mut full = vec![drive.token.clone()];
                    full.extend_from_slice(parts);
                    Answer::Text(
                        207,
                        vec![("Content-Type", "application/xml; charset=utf-8".to_string())],
                        dav::props_set(&dav::href(&full, false)),
                    )
                }
            };
        }
        _ => {}
    }
    let node = match drive.resolve(parts).await {
        Ok(node) => node,
        Err(fail) => return status(fail.status()),
    };
    match request.method.as_str() {
        "PROPFIND" => match drive.props(parts, &node, request.depth).await {
            Ok(props) => Answer::Text(
                207,
                vec![("Content-Type", "application/xml; charset=utf-8".to_string())],
                dav::multistatus(&props),
            ),
            Err(fail) => status(fail.status()),
        },
        "HEAD" => match node {
            Node::File(_, _, _, entry) => Answer::Size(entry.size),
            _ => status(200),
        },
        _ => match node {
            // An empty file held here: empty.
            Node::File(device, root, path, _)
                if drive.is_stub(&(device.clone(), root, path.clone())) =>
            {
                Answer::Text(200, Vec::new(), String::new())
            }
            Node::File(device, root, path, _) => match drive.file(&device, root, &path).await {
                Ok(file) => {
                    let size = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
                    match dav::byte_range(request.range.as_deref(), size) {
                        None => Answer::File(file, 0, size, size),
                        Some(Some((first, last))) => Answer::File(file, first, last + 1, size),
                        Some(None) => Answer::Text(
                            416,
                            vec![("Content-Range", format!("bytes */{size}"))],
                            String::new(),
                        ),
                    }
                }
                Err(503) => {
                    Answer::Text(503, vec![("Retry-After", "5".to_string())], String::new())
                }
                Err(code) => status(code),
            },
            _ => Answer::Text(
                200,
                vec![("Content-Type", "text/plain; charset=utf-8".to_string())],
                "A folder of a phone: open it in a file manager.\n".to_string(),
            ),
        },
    }
}

/// `MKCOL` (N6): the folder made on the phone, or why not.
async fn make_folder(drive: &Drive, parts: &[String]) -> Answer {
    let place = match drive.write_target(parts).await {
        Ok(place) => place,
        Err(code) => return status(code),
    };
    // A name the folder holds already: WebDAV's 405.
    if drive.resolve(parts).await.is_ok() {
        return status(405);
    }
    let (device, root, path) = &place;
    let made = drive.phones.mkdir(device, *root, path).await;
    drive.forget_folder(&place);
    match made {
        Ok(listing) => match listing.error {
            None => status(201),
            Some(error) if error == EXISTS => status(405),
            Some(error) => Answer::Text(403, Vec::new(), error),
        },
        Err(fail) => status(fail.status()),
    }
}

/// `PUT` (N6): the body taken into a file here, given to the phone, and
/// answered by how the phone took it, within [`PUT_WAIT`]. An empty body is
/// held for its content ([`STUB_KEEP`]).
async fn put<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    drive: &Drive,
    stream: &mut S,
    request: &dav::Request,
    early: &[u8],
) -> std::io::Result<()> {
    let refuse = |code: u16| Answer::Text(code, Vec::new(), String::new());
    let target = match dav::segments(&request.path) {
        None => Err(400),
        Some(parts) => match parts.split_first() {
            Some((token, parts)) if token == &drive.token => drive.write_target(parts).await,
            _ => Err(404),
        },
    };
    let place = match target {
        Ok(place) => place,
        Err(405) => {
            return write_text(stream, 405, &[("Allow", allow(drive))], "", false).await;
        }
        Err(code) => return answer_with(stream, refuse(code)).await,
    };
    if !request.has_length {
        return write_text(stream, 411, &[], "", false).await;
    }
    if request.content_length > MAX_PUT_BYTES {
        return write_text(stream, 413, &[], "", false).await;
    }
    if request.content_length == 0 {
        let full = {
            let mut stubs = drive.stubs.lock().unwrap_or_else(|e| e.into_inner());
            let full = stubs.len() >= MAX_STUBS && !stubs.contains_key(&place);
            if !full {
                stubs.insert(place.clone(), Instant::now());
            }
            full
        };
        if full {
            return write_text(stream, 503, &[("Retry-After", "30".to_string())], "", false).await;
        }
        drive.forget_folder(&place);
        return write_text(stream, 201, &[], "", false).await;
    }
    // Its empty file, held here, is out of the sweep while its content
    // comes, and back if the content never comes whole (N6 review).
    let held = drive
        .stubs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&place)
        .is_some();
    let read = take_body(drive, stream, request, early, &place).await;
    if !matches!(read, Ok(Some(_))) && held {
        drive
            .stubs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(place.clone(), Instant::now());
    }
    let Some(path) = read? else {
        return Ok(());
    };
    give(drive, stream, place, path, request.content_length).await
}

/// How long a copy of `size` bytes may take to arrive in all: a minute,
/// and a second for every 32 KiB (N6 review: a trickle holds no slot for
/// hours).
fn body_deadline(size: u64) -> Duration {
    Duration::from_secs(60 + size / (32 * 1024))
}

/// Take `request`'s body into a new upload: its path once whole, `None`
/// when the client left or stalled.
async fn take_body<S: tokio::io::AsyncRead + Unpin>(
    drive: &Drive,
    stream: &mut S,
    request: &dav::Request,
    early: &[u8],
    place: &Place,
) -> std::io::Result<Option<PathBuf>> {
    let Some(dir) = &drive.uploads else {
        return Ok(None);
    };
    let name = place.2.last().cloned().unwrap_or_default();
    let (path, file) = new_upload(dir, &name)?;
    // Removed unless it reaches the phone, which owns it from then on.
    struct Upload(Option<PathBuf>);
    impl Drop for Upload {
        fn drop(&mut self) {
            if let Some(path) = self.0.take() {
                remove_upload(&path);
            }
        }
    }
    let by = tokio::time::Instant::now() + body_deadline(request.content_length);
    let mut upload = Upload(Some(path.clone()));
    let mut file = tokio::fs::File::from_std(file);
    let size = request.content_length;
    let early = &early[..early.len().min(size as usize)];
    file.write_all(early).await?;
    let mut left = size - early.len() as u64;
    let mut chunk = vec![0u8; 64 * 1024];
    while left > 0 {
        let want = chunk.len().min(left as usize);
        let wait = BODY_WAIT.min(by.saturating_duration_since(tokio::time::Instant::now()));
        let n = match tokio::time::timeout(wait, stream.read(&mut chunk[..want])).await {
            Ok(Ok(n)) => n,
            Ok(Err(_)) | Err(_) => return Ok(None),
        };
        if n == 0 {
            return Ok(None);
        }
        file.write_all(&chunk[..n]).await?;
        left -= n as u64;
    }
    file.flush().await?;
    drop(file);
    Ok(upload.0.take())
}

/// Give the copy at `path` to its phone, and answer how it took it.
async fn give<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    drive: &Drive,
    stream: &mut S,
    place: Place,
    path: PathBuf,
    size: u64,
) -> std::io::Result<()> {
    let refuse = |code: u16| Answer::Text(code, Vec::new(), String::new());
    let (device, root, full) = place.clone();
    let Some((name, folder)) = full.split_last() else {
        return Ok(());
    };
    let (name, folder) = (name.clone(), folder.to_vec());
    let phones = drive.phones.clone();
    let shown = name.clone();
    // Given on a task of its own: a client that leaves does not take the
    // file back from the phone.
    let mut given =
        tokio::spawn(async move { phones.put(&device, root, &folder, &name, path, size).await });
    let taken = tokio::select! {
        taken = &mut given => taken.unwrap_or(Err(Fail::Phone)),
        () = gone(stream) => {
            // No one hears the answer (N6 review): the owner is told of a
            // refusal or a failure another way.
            let phones = drive.phones.clone();
            tokio::spawn(async move {
                let why = match given.await {
                    Ok(Ok(Put::Refused(why) | Put::NoRoom(why))) => why,
                    Ok(Err(_)) | Err(_) => "the phone did not take it".to_string(),
                    Ok(Ok(Put::Saved | Put::UnderWay)) => return,
                };
                phones.tell(&shown, &why);
            });
            return Ok(());
        }
    };
    drive.forget_folder(&place);
    let answer = match taken {
        Ok(Put::Saved) => refuse(201),
        Ok(Put::UnderWay) => refuse(202),
        Ok(Put::Refused(why)) => Answer::Text(403, Vec::new(), why),
        Ok(Put::NoRoom(why)) => Answer::Text(507, Vec::new(), why),
        Err(fail) => refuse(fail.status()),
    };
    answer_with(stream, answer).await
}

/// Write a text `answer`.
async fn answer_with<S: tokio::io::AsyncWrite + Unpin>(
    stream: &mut S,
    answer: Answer,
) -> std::io::Result<()> {
    match answer {
        Answer::Text(code, headers, body) => write_text(stream, code, &headers, &body, false).await,
        _ => write_text(stream, 500, &[], "", false).await,
    }
}

/// How long a request's head may take to arrive.
const HEAD_WAIT: Duration = Duration::from_secs(10);

/// Answer one connection's request, and close it.
async fn serve_one<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    drive: &Drive,
    mut stream: S,
) -> std::io::Result<()> {
    let mut buf = Vec::with_capacity(1024);
    // The whole head within the wait, not each read: a byte every few
    // seconds holds no slot for hours (D8 review).
    let head_by = tokio::time::Instant::now() + HEAD_WAIT;
    let end = loop {
        let mut chunk = [0u8; 1024];
        let n = tokio::time::timeout_at(head_by, stream.read(&mut chunk))
            .await
            .map_err(|_| std::io::Error::other("no request in time"))??;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > dav::MAX_HEAD {
            return write_text(&mut stream, 400, &[], "", false).await;
        }
    };
    let Some(request) = dav::parse_head(&buf[..end]) else {
        return write_text(&mut stream, 400, &[], "", false).await;
    };
    if request.chunked {
        return write_text(&mut stream, 501, &[], "", false).await;
    }
    // A copy's body is taken as it comes, not set aside (N6).
    if request.method == "PUT" {
        return put(drive, &mut stream, &request, &buf[end + 4..]).await;
    }
    if request.content_length > dav::MAX_BODY {
        return write_text(&mut stream, 413, &[], "", false).await;
    }
    // The body is read and set aside: every PROPFIND gets the same props.
    let mut body_left = request
        .content_length
        .saturating_sub((buf.len() - end - 4) as u64);
    while body_left > 0 {
        let mut chunk = [0u8; 4096];
        let n = tokio::time::timeout(HEAD_WAIT, stream.read(&mut chunk))
            .await
            .map_err(|_| std::io::Error::other("no body in time"))??;
        if n == 0 {
            return Ok(());
        }
        body_left = body_left.saturating_sub(n as u64);
    }
    let head_only = request.method == "HEAD";
    // A client that leaves while the phone is asked (WebClient gives up
    // after a minute) ends the asking, and frees its slot (D8 review).
    let answered = tokio::select! {
        answered = answer(drive, &request) => answered,
        () = gone(&mut stream) => return Ok(()),
    };
    match answered {
        Answer::Text(code, headers, body) => {
            write_text(&mut stream, code, &headers, &body, head_only).await
        }
        Answer::Size(size) => {
            let head = dav::head(
                200,
                &[
                    ("Content-Type", "application/octet-stream".to_string()),
                    ("Accept-Ranges", "bytes".to_string()),
                ],
                size,
            );
            stream.write_all(head.as_bytes()).await?;
            stream.flush().await
        }
        Answer::File(path, first, end, size) => {
            use tokio::io::AsyncSeekExt;
            let mut headers = vec![
                ("Content-Type", "application/octet-stream".to_string()),
                ("Accept-Ranges", "bytes".to_string()),
            ];
            let whole = first == 0 && end == size;
            if !whole {
                headers.push(("Content-Range", format!("bytes {first}-{}/{size}", end - 1)));
            }
            let head = dav::head(if whole { 200 } else { 206 }, &headers, end - first);
            stream.write_all(head.as_bytes()).await?;
            if !head_only {
                let mut file = tokio::fs::File::open(&path).await?;
                file.seek(std::io::SeekFrom::Start(first)).await?;
                tokio::io::copy(&mut file.take(end - first), &mut stream).await?;
            }
            stream.flush().await
        }
    }
}

async fn write_text<S: tokio::io::AsyncWrite + Unpin>(
    stream: &mut S,
    code: u16,
    headers: &[(&'static str, String)],
    body: &str,
    head_only: bool,
) -> std::io::Result<()> {
    let head = dav::head(code, headers, body.len() as u64);
    stream.write_all(head.as_bytes()).await?;
    if !head_only {
        stream.write_all(body.as_bytes()).await?;
    }
    stream.flush().await
}

/// End a connection gracefully: our side half-closed, then the client's
/// read to its end (briefly), so the close is no reset that could overtake
/// the answer it has not read yet.
async fn close(stream: &mut tokio::net::TcpStream) {
    let _ = stream.shutdown().await;
    let mut chunk = [0u8; 512];
    let drained = async {
        while let Ok(n) = stream.read(&mut chunk).await {
            if n == 0 {
                break;
            }
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(2), drained).await;
}

/// Whether the client has closed its side (or the connection failed):
/// anything it sends meanwhile is set aside.
async fn gone<S: tokio::io::AsyncRead + Unpin>(stream: &mut S) {
    let mut chunk = [0u8; 512];
    while let Ok(n) = stream.read(&mut chunk).await {
        if n == 0 {
            return;
        }
    }
}

/// Requests answered at once, at most: a file manager opens several.
const MAX_CONNECTIONS: usize = 16;

/// Answer every connection `listener` takes until `stop` is cancelled.
pub async fn serve(
    listener: tokio::net::TcpListener,
    drive: Arc<Drive>,
    stop: tokio_util::sync::CancellationToken,
) {
    let slots = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
    // Empty files nothing came to write in go to their phone as they are.
    {
        let (drive, stop) = (drive.clone(), stop.clone());
        tokio::spawn(async move {
            let mut every = tokio::time::interval(Duration::from_secs(15));
            loop {
                tokio::select! {
                    _ = stop.cancelled() => return,
                    _ = every.tick() => drive.deliver_stale_stubs(),
                }
            }
        });
    }
    loop {
        let accepted = tokio::select! {
            _ = stop.cancelled() => return,
            accepted = listener.accept() => accepted,
        };
        let Ok((stream, peer)) = accepted else {
            // Out of descriptors, say: a pause, not a spin (D8 review).
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        // Bound to loopback; held to it anyway.
        if !peer.ip().is_loopback() {
            continue;
        }
        let Ok(slot) = slots.clone().try_acquire_owned() else {
            continue;
        };
        let drive = drive.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            let _slot = slot;
            let mut stream = stream;
            tokio::select! {
                _ = stop.cancelled() => return,
                served = serve_one(&drive, &mut stream) => {
                    if let Err(e) = served {
                        tracing::debug!("A drive request ended: {e}");
                    }
                }
            }
            close(&mut stream).await;
        });
    }
}

#[cfg(test)]
mod tests;
