use super::*;
use pocketdesk_core::protocol::phone_files::Root;
use std::sync::atomic::{AtomicUsize, Ordering};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

/// A phone from memory: its listings, and files written to a temp folder
/// when fetched.
struct StandIn {
    listings: HashMap<(Option<u32>, Vec<String>), Listing>,
    files: HashMap<(u32, Vec<String>), Vec<u8>>,
    lists: AtomicUsize,
    fetches: AtomicUsize,
    /// Fetches given up on before they finished.
    abandoned: Arc<AtomicUsize>,
    cache: PathBuf,
}

/// Counts a fetch dropped before it finished.
struct Abandoned(Arc<AtomicUsize>);

impl Drop for Abandoned {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn path(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

fn entry(name: &str, dir: bool, size: u64) -> Entry {
    Entry {
        name: name.to_string(),
        dir,
        size,
        modified: 784_111_777_000,
    }
}

fn stand_in(name: &str) -> Arc<StandIn> {
    let roots = vec![Root {
        id: 7,
        name: "DCIM".to_string(),
    }];
    let listing = |root: Option<u32>, path: Vec<String>, entries: Vec<Entry>| Listing {
        roots: roots.clone(),
        root,
        path,
        entries,
        error: None,
        truncated: false,
    };
    let mut listings = HashMap::new();
    listings.insert((None, vec![]), listing(Some(7), vec![], vec![]));
    listings.insert(
        (Some(7), vec![]),
        listing(
            Some(7),
            vec![],
            vec![
                entry("Camera", true, 0),
                entry("a & b.jpg", false, 5),
                entry("slow.bin", false, 9),
            ],
        ),
    );
    listings.insert(
        (Some(7), path(&["Camera"])),
        listing(Some(7), path(&["Camera"]), vec![entry("x.jpg", false, 3)]),
    );
    let mut files = HashMap::new();
    files.insert((7, path(&["a & b.jpg"])), b"hello".to_vec());
    let cache = std::env::temp_dir().join(format!("pd-drive-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache);
    std::fs::create_dir_all(&cache).unwrap();
    Arc::new(StandIn {
        listings,
        files,
        lists: AtomicUsize::new(0),
        fetches: AtomicUsize::new(0),
        abandoned: Arc::new(AtomicUsize::new(0)),
        cache,
    })
}

#[async_trait::async_trait]
impl Phones for StandIn {
    fn phones(&self) -> Vec<(String, String)> {
        vec![("phone-a".to_string(), "Pixel".to_string())]
    }
    async fn list(
        &self,
        device: &str,
        root: Option<u32>,
        path: &[String],
    ) -> Result<Listing, Fail> {
        assert_eq!(device, "phone-a");
        self.lists.fetch_add(1, Ordering::SeqCst);
        self.listings
            .get(&(root, path.to_vec()))
            .cloned()
            .ok_or(Fail::NotFound)
    }
    async fn fetch(&self, _device: &str, root: u32, path: &[String]) -> Result<PathBuf, Fail> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        if path.last().map(String::as_str) == Some("slow.bin") {
            // A phone that never sends it.
            let _abandoned = Abandoned(self.abandoned.clone());
            std::future::pending::<()>().await;
        }
        let bytes = self
            .files
            .get(&(root, path.to_vec()))
            .ok_or(Fail::NotFound)?;
        let file = self
            .cache
            .join(format!("f{}", self.fetches.load(Ordering::SeqCst)));
        std::fs::write(&file, bytes).unwrap();
        Ok(file)
    }
}

async fn start(phone: Arc<StandIn>) -> (std::net::SocketAddr, tokio_util::sync::CancellationToken) {
    start_keeping(phone, FILE_KEEP).await
}

async fn start_keeping(
    phone: Arc<StandIn>,
    keep: Duration,
) -> (std::net::SocketAddr, tokio_util::sync::CancellationToken) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let stop = tokio_util::sync::CancellationToken::new();
    let drive = Arc::new(Drive::new(TOKEN.to_string(), phone).keeping_files(keep));
    tokio::spawn(serve(listener, drive, stop.clone()));
    (addr, stop)
}

async fn ask(addr: std::net::SocketAddr, method: &str, path: &str, headers: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\n{headers}\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    stream.read_to_end(&mut out).await.unwrap();
    String::from_utf8(out).unwrap()
}

fn code(response: &str) -> u16 {
    response[9..12].parse().unwrap()
}

#[tokio::test]
async fn options_says_dav_and_the_token_guards_the_rest() {
    let phone = stand_in("guard");
    let (addr, stop) = start(phone.clone()).await;
    let options = ask(addr, "OPTIONS", "/", "").await;
    assert_eq!(code(&options), 200);
    assert!(options.contains("DAV: 1\r\n"), "{options}");
    for wrong in [
        "/",
        "/nottoken/",
        "/0123456789abcdef0123456789abcdee/Pixel/",
    ] {
        assert_eq!(
            code(&ask(addr, "PROPFIND", wrong, "Depth: 1\r\n").await),
            404,
            "{wrong}"
        );
    }
    let base = format!("/{TOKEN}/Pixel/DCIM/");
    for method in ["PUT", "DELETE", "MKCOL", "MOVE", "LOCK", "PROPPATCH"] {
        assert_eq!(code(&ask(addr, method, &base, "").await), 405, "{method}");
    }
    assert_eq!(
        code(&ask(addr, "PROPFIND", &format!("/{TOKEN}/..%2F"), "").await),
        400
    );
    assert_eq!(
        phone.lists.load(Ordering::SeqCst),
        0,
        "nothing was asked of the phone"
    );
    stop.cancel();
}

#[tokio::test]
async fn folders_are_listed_from_the_phone_and_kept() {
    let phone = stand_in("list");
    let (addr, stop) = start(phone.clone()).await;
    let top = ask(addr, "PROPFIND", &format!("/{TOKEN}/"), "Depth: 1\r\n").await;
    assert_eq!(code(&top), 207);
    assert!(
        top.contains(&format!("<D:href>/{TOKEN}/Pixel/</D:href>")),
        "{top}"
    );
    let roots = ask(
        addr,
        "PROPFIND",
        &format!("/{TOKEN}/Pixel/"),
        "Depth: 1\r\n",
    )
    .await;
    assert!(
        roots.contains(&format!("<D:href>/{TOKEN}/Pixel/DCIM/</D:href>")),
        "{roots}"
    );
    let dir = ask(
        addr,
        "PROPFIND",
        &format!("/{TOKEN}/Pixel/DCIM/"),
        "Depth: 1\r\n",
    )
    .await;
    assert!(
        dir.contains(&format!("<D:href>/{TOKEN}/Pixel/DCIM/Camera/</D:href>")),
        "{dir}"
    );
    assert!(dir.contains(&format!(
        "<D:href>/{TOKEN}/Pixel/DCIM/a%20%26%20b.jpg</D:href>"
    )));
    assert!(dir.contains("<D:displayname>a &amp; b.jpg</D:displayname>"));
    assert!(dir.contains("<D:getcontentlength>5</D:getcontentlength>"));
    let one = ask(
        addr,
        "PROPFIND",
        &format!("/{TOKEN}/Pixel/DCIM/"),
        "Depth: 0\r\n",
    )
    .await;
    assert_eq!(one.matches("<D:response>").count(), 1, "depth 0: itself");
    let sub = ask(
        addr,
        "PROPFIND",
        &format!("/{TOKEN}/Pixel/DCIM/Camera"),
        "Depth: 1\r\n",
    )
    .await;
    assert!(sub.contains("x.jpg"), "{sub}");
    let asked = phone.lists.load(Ordering::SeqCst);
    // A file manager's looks for files that are not there, and again at the
    // same folder: answered from what is kept.
    for missing in ["desktop.ini", "Thumbs.db", "folder.jpg"] {
        let r = ask(
            addr,
            "PROPFIND",
            &format!("/{TOKEN}/Pixel/DCIM/{missing}"),
            "Depth: 0\r\n",
        )
        .await;
        assert_eq!(code(&r), 404, "{missing}");
    }
    let again = ask(
        addr,
        "PROPFIND",
        &format!("/{TOKEN}/Pixel/DCIM/"),
        "Depth: 1\r\n",
    )
    .await;
    assert_eq!(code(&again), 207);
    assert_eq!(
        phone.lists.load(Ordering::SeqCst),
        asked,
        "kept, not asked again"
    );
    assert_eq!(
        code(&ask(addr, "PROPFIND", &format!("/{TOKEN}/Other/"), "").await),
        404
    );
    stop.cancel();
}

#[tokio::test]
async fn a_file_is_fetched_once_read_again_in_pieces_and_removed_later() {
    let phone = stand_in("file");
    let (addr, stop) = start(phone.clone()).await;
    let file = format!("/{TOKEN}/Pixel/DCIM/a%20%26%20b.jpg");
    let head = ask(addr, "HEAD", &file, "").await;
    assert_eq!(code(&head), 200);
    assert!(head.contains("Content-Length: 5\r\n"), "{head}");
    assert!(head.ends_with("\r\n\r\n"), "a HEAD has no body");
    assert_eq!(
        phone.fetches.load(Ordering::SeqCst),
        0,
        "a HEAD fetches nothing"
    );
    let got = ask(addr, "GET", &file, "").await;
    assert_eq!(code(&got), 200);
    assert!(got.ends_with("\r\n\r\nhello"), "{got}");
    assert!(got.contains("Accept-Ranges: bytes\r\n"), "{got}");
    // Read again, and in a piece: from what was fetched (D8 review).
    let again = ask(addr, "GET", &file, "").await;
    assert!(again.ends_with("\r\n\r\nhello"), "{again}");
    let piece = ask(addr, "GET", &file, "Range: bytes=1-3\r\n").await;
    assert_eq!(code(&piece), 206, "{piece}");
    assert!(piece.contains("Content-Range: bytes 1-3/5\r\n"), "{piece}");
    assert!(piece.ends_with("\r\n\r\nell"), "{piece}");
    let past = ask(addr, "GET", &file, "Range: bytes=9-\r\n").await;
    assert_eq!(code(&past), 416, "{past}");
    assert_eq!(phone.fetches.load(Ordering::SeqCst), 1, "fetched once");
    let folder = ask(addr, "GET", &format!("/{TOKEN}/Pixel/DCIM/"), "").await;
    assert_eq!(code(&folder), 200);
    // A body is read and set aside.
    let body = "<?xml version=\"1.0\"?><propfind xmlns=\"DAV:\"><allprop/></propfind>";
    let with_body = ask(
        addr,
        "PROPFIND",
        &format!("/{TOKEN}/Pixel/DCIM/"),
        &format!("Depth: 0\r\nContent-Length: {}\r\n\r\n{body}", body.len()),
    )
    .await;
    assert_eq!(code(&with_body), 207, "{with_body}");
    let _ = std::fs::remove_dir_all(&phone.cache);
    stop.cancel();
}

/// D8 review: a fetched file is kept a while, then removed and fetched again.
#[tokio::test]
async fn a_kept_file_goes_when_its_time_is_up() {
    let phone = stand_in("expire");
    let (addr, stop) = start_keeping(phone.clone(), Duration::ZERO).await;
    let file = format!("/{TOKEN}/Pixel/DCIM/a%20%26%20b.jpg");
    assert_eq!(code(&ask(addr, "GET", &file, "").await), 200);
    assert_eq!(code(&ask(addr, "GET", &file, "").await), 200);
    assert_eq!(
        phone.fetches.load(Ordering::SeqCst),
        2,
        "kept no time: fetched again"
    );
    assert_eq!(
        std::fs::read_dir(&phone.cache).unwrap().count(),
        1,
        "the first removed when the second was asked for"
    );
    let _ = std::fs::remove_dir_all(&phone.cache);
    stop.cancel();
}

/// D8 review: no more than the phone can send are asked at once.
#[tokio::test]
async fn more_fetches_than_the_phone_sends_are_asked_again_later() {
    let phone = stand_in("busy");
    let (addr, stop) = start(phone.clone()).await;
    let slow = format!("GET /{TOKEN}/Pixel/DCIM/slow.bin HTTP/1.1\r\nHost: x\r\n\r\n");
    let mut held = Vec::new();
    for _ in 0..FETCHES_AT_ONCE {
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        stream.write_all(slow.as_bytes()).await.unwrap();
        held.push(stream);
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while phone.fetches.load(Ordering::SeqCst) < FETCHES_AT_ONCE {
        assert!(
            tokio::time::Instant::now() < deadline,
            "TIMEOUT: the fetches never began"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let busy = ask(
        addr,
        "GET",
        &format!("/{TOKEN}/Pixel/DCIM/a%20%26%20b.jpg"),
        "",
    )
    .await;
    assert_eq!(code(&busy), 503, "{busy}");
    assert!(busy.contains("Retry-After: 5\r\n"), "{busy}");
    // Folders are still listed meanwhile.
    let listed = ask(
        addr,
        "PROPFIND",
        &format!("/{TOKEN}/Pixel/DCIM/"),
        "Depth: 1\r\n",
    )
    .await;
    assert_eq!(code(&listed), 207);
    drop(held);
    let _ = std::fs::remove_dir_all(&phone.cache);
    stop.cancel();
}

/// D8 review: nothing below a hidden name is asked of the phone.
#[tokio::test]
async fn a_hidden_folder_is_not_there_without_asking() {
    let phone = stand_in("hidden");
    let (addr, stop) = start(phone.clone()).await;
    let r = ask(
        addr,
        "PROPFIND",
        &format!("/{TOKEN}/Pixel/DCIM/.thumbnails/x.jpg"),
        "",
    )
    .await;
    assert_eq!(code(&r), 404);
    assert_eq!(
        phone.lists.load(Ordering::SeqCst),
        1,
        "only the phone's folders, to find DCIM"
    );
    let _ = std::fs::remove_dir_all(&phone.cache);
    stop.cancel();
}

/// D8 review: a head that trickles in is dropped at the head's deadline.
#[tokio::test(start_paused = true)]
async fn a_head_that_never_ends_is_dropped() {
    let phone = stand_in("trickle");
    let (addr, stop) = start(phone.clone()).await;
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream.write_all(b"PROPFIND /").await.unwrap();
    tokio::time::sleep(HEAD_WAIT + Duration::from_secs(1)).await;
    let mut out = Vec::new();
    let read = stream.read_to_end(&mut out).await;
    assert!(read.is_err() || out.is_empty(), "closed with no answer");
    let _ = std::fs::remove_dir_all(&phone.cache);
    stop.cancel();
}

/// D8 review: a client that leaves while the phone is asked ends the asking.
#[tokio::test]
async fn a_client_that_leaves_ends_the_fetch() {
    let phone = stand_in("leave");
    let (addr, stop) = start(phone.clone()).await;
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!("GET /{TOKEN}/Pixel/DCIM/slow.bin HTTP/1.1\r\nHost: x\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while phone.fetches.load(Ordering::SeqCst) == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "TIMEOUT: never fetched"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    drop(stream);
    while phone.abandoned.load(Ordering::SeqCst) == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "TIMEOUT: the fetch went on"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let _ = std::fs::remove_dir_all(&phone.cache);
    stop.cancel();
}

#[test]
fn phones_and_folders_sharing_a_name_are_told_apart() {
    let folders = phone_folders(&[
        ("aaaaaaaa-1".into(), "Galaxy".into()),
        ("bbbbbbbb-2".into(), "Galaxy".into()),
        ("cccccccc-3".into(), "Pixel/8".into()),
        ("dddddddd-4".into(), " ".into()),
    ]);
    let names: Vec<&str> = folders.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        [
            "Galaxy (aaaaaaaa)",
            "Galaxy (bbbbbbbb)",
            "Pixel8",
            "Phone dddddddd"
        ]
    );
    let listing = Listing {
        roots: vec![
            Root {
                id: 1,
                name: "Download".into(),
            },
            Root {
                id: 2,
                name: "Download".into(),
            },
            Root {
                id: 3,
                name: "DCIM".into(),
            },
        ],
        root: None,
        path: vec![],
        entries: vec![],
        error: None,
        truncated: false,
    };
    let roots: Vec<String> = root_folders(&listing).into_iter().map(|(n, _)| n).collect();
    assert_eq!(roots, ["Download (1)", "Download (2)", "DCIM"]);
}
