//! The phone's folders as a drive (D8): the folders phones share (T10g),
//! served read-only as WebDAV on loopback behind a token, with
//! `phone_drive` on. A folder is listed by asking its phone (T10g's
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

use pocketdesk_core::protocol::phone_files::{Entry, Listing, valid_name};
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

/// The drive: its token, its phones, and the listings it keeps.
pub struct Drive {
    token: String,
    phones: Arc<dyn Phones>,
    kept: Mutex<HashMap<Asked, (Instant, Listing)>>,
    /// Files fetched, for [`FILE_KEEP`]: when, where.
    fetched: Mutex<HashMap<Asked, (Instant, PathBuf)>>,
    file_keep: Duration,
    fetching: tokio::sync::Semaphore,
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
        let entry = listing
            .entries
            .into_iter()
            .find(|e| &e.name == name)
            .ok_or(Fail::NotFound)?;
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
                for entry in self.folder(device, *root, path).await?.entries {
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

fn status(code: u16) -> Answer {
    Answer::Text(code, Vec::new(), String::new())
}

/// The answer to `request`.
async fn answer(drive: &Drive, request: &dav::Request) -> Answer {
    // OPTIONS says what this is, anywhere: a client asks it first.
    if request.method == "OPTIONS" {
        return Answer::Text(
            200,
            vec![
                ("DAV", "1".to_string()),
                ("MS-Author-Via", "DAV".to_string()),
                ("Allow", ALLOW.to_string()),
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
    if !matches!(request.method.as_str(), "PROPFIND" | "GET" | "HEAD") {
        return Answer::Text(405, vec![("Allow", ALLOW.to_string())], String::new());
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
            tokio::select! {
                _ = stop.cancelled() => {}
                served = serve_one(&drive, stream) => {
                    if let Err(e) = served {
                        tracing::debug!("A drive request ended: {e}");
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests;
