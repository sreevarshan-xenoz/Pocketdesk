//! Send to the phone from the browser (ecosystem phase 1d): the native
//! messaging host the Pocketdesk extension (`browser/`) talks to. The browser
//! starts this program with its own arguments (Chromium: the extension's
//! origin; Firefox: the manifest's path and the extension's id) and speaks
//! length-prefixed JSON on its stdin and stdout; each request is answered
//! `{ok}` or `{error}`. No port and no listener, as for the rest of the local
//! API: the browser runs this program as the owner.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

/// The host's name, as the extension and the browsers' registrations call it.
pub const HOST_NAME: &str = "dev.pocketdesk.host";
/// The extension's id in Chromium browsers (fixed by the `key` in its manifest).
pub const CHROMIUM_EXTENSION_ID: &str = "abojofjjnpcmejhdonndlegfaolfkcif";
/// The extension's id in Firefox (`browser_specific_settings.gecko.id`).
pub const FIREFOX_EXTENSION_ID: &str = "pocketdesk@pocketdesk.dev";

/// The longest message either way (the browsers' own limit from a host is
/// 1 MiB; ours to it is far more, but no request of ours comes near).
const MAX_MESSAGE: usize = 1024 * 1024;
/// The largest image sent on.
const MAX_IMAGE: usize = 32 * 1024 * 1024;
/// An image download has this long, all told.
const FETCH_DEADLINE: Duration = Duration::from_secs(20);
/// Downloaded images the phone has not taken are kept this long.
const KEEP_IMAGES: Duration = Duration::from_secs(24 * 3600);

/// Whether the browser started this program as its host: Chromium passes
/// the extension's origin, Firefox the manifest's path and the extension id.
pub fn launched_by_browser(args: &[String]) -> bool {
    args.get(1)
        .is_some_and(|a| a.starts_with("chrome-extension://"))
        || args.get(2).is_some_and(|a| a == FIREFOX_EXTENSION_ID)
}

/// One message from the browser, or `None` when it closed the pipe.
pub fn read_message(from: &mut impl Read) -> io::Result<Option<Value>> {
    let mut len = [0u8; 4];
    match from.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_ne_bytes(len) as usize;
    if len > MAX_MESSAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "a message too long",
        ));
    }
    let mut body = vec![0u8; len];
    from.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// One message to the browser.
pub fn write_message(to: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = message.to_string();
    let body = if body.len() > MAX_MESSAGE {
        json!({ "error": "the answer was too long" }).to_string()
    } else {
        body
    };
    to.write_all(&(body.len() as u32).to_ne_bytes())?;
    to.write_all(body.as_bytes())?;
    to.flush()
}

fn ok(text: impl Into<String>) -> Value {
    json!({ "ok": text.into() })
}

fn err(text: impl Into<String>) -> Value {
    json!({ "error": text.into() })
}

/// An image as downloaded: its bytes and a file name for it.
pub struct Image {
    pub bytes: Vec<u8>,
    pub name: String,
}

/// Answer one request. `fetch` downloads an image (the real one is
/// [`fetch_image`]); `running` says whether the server is up.
pub fn handle(
    state_dir: &Path,
    request: &Value,
    running: bool,
    fetch: &dyn Fn(&str) -> Result<Image, String>,
) -> Value {
    let url = request
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if !running {
        return err("Pocketdesk is not running on this computer.");
    }
    let now = pocketdesk_core::local::pin::now_unix();
    let Some(phone) = crate::local_api::default_phone(state_dir, now, running) else {
        return err("No phone is connected: open Pocketdesk on your phone.");
    };
    match request.get("send").and_then(Value::as_str) {
        Some("link") => {
            let (out, code) = crate::local_api::link_in(state_dir, &phone, url, running);
            if code == 0 {
                ok("Sent to your phone.")
            } else {
                err(out["error"].as_str().unwrap_or("Not sent."))
            }
        }
        Some("image") => {
            if let Some(problem) = pocketdesk_core::protocol::link::link_problem(url) {
                return err(format!("Not sent: {problem}"));
            }
            let image = match fetch(url) {
                Ok(image) => image,
                Err(why) => return err(format!("Not sent: {why}")),
            };
            let path = match images_dir().and_then(|dir| keep_image(&dir, &image)) {
                Ok(path) => path,
                Err(e) => return err(format!("Not sent: could not keep it ({e})")),
            };
            let (out, code) =
                crate::local_api::send_in(state_dir, &phone, &path, &|| running, None);
            if code == 0 {
                ok(format!("Sending {} to your phone.", image.name))
            } else {
                let _ = std::fs::remove_file(&path);
                err(out["error"].as_str().unwrap_or("Not sent."))
            }
        }
        _ => err("Pocketdesk does not know that request."),
    }
}

/// Where downloaded images wait for the phone: a folder of this user's own
/// (the cache folder on Linux, never the shared `/tmp`, where another user
/// could make it first; the per-user temporary folder on Windows), and
/// never the state folder, whose files the server never sends.
fn images_dir() -> io::Result<PathBuf> {
    #[cfg(windows)]
    let base = std::env::temp_dir();
    #[cfg(not(windows))]
    let base = dirs::cache_dir().ok_or_else(|| io::Error::other("no cache folder"))?;
    Ok(base.join("pocketdesk-browser"))
}

/// Keep `image` in its own owner-only folder under `dir`, so the phone gets
/// it under its own name; ones older than a day go first. `dir` must be a
/// real folder: a link is never followed, made private or emptied.
fn keep_image(dir: &Path, image: &Image) -> io::Result<PathBuf> {
    let dir = dir.to_path_buf();
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if !meta.file_type().is_dir() => {
            return Err(io::Error::other(format!(
                "{} is not a folder of its own",
                dir.display()
            )));
        }
        _ => {}
    }
    pocketdesk_core::secure_fs::create_private_dir(&dir)?;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| t.elapsed().is_ok_and(|age| age > KEEP_IMAGES));
            if old {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    let one = dir.join(uuid::Uuid::new_v4().simple().to_string());
    pocketdesk_core::secure_fs::create_private_dir(&one)?;
    let path = one.join(&image.name);
    pocketdesk_core::secure_fs::write_private(&path, &image.bytes)?;
    Ok(path)
}

/// The extension for an image of `content_type`, for the types sent on:
/// PNG, JPEG, GIF, WebP, AVIF and BMP. Others, SVG (which can carry script)
/// among them, are not sent.
pub fn image_extension(content_type: &str) -> Option<&'static str> {
    let kind = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    Some(match kind.as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" | "image/pjpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/avif" => "avif",
        "image/bmp" => "bmp",
        _ => return None,
    })
}

/// `%XX` escapes decoded; a bad one is kept as written.
fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The longest name kept from a URL, in bytes (well under every file
/// system's 255).
const MAX_STEM: usize = 80;

/// A file name for an image at `url` with extension `ext` (from
/// [`image_extension`]): the URL's last part without its own extension,
/// which the content type replaces, so a page cannot pass an `.apk` off as
/// a picture; `image` when the URL has no usable name.
pub fn image_name(url: &str, ext: &str) -> String {
    let last = url
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('/')
        .next()
        .unwrap_or("");
    let last = percent_decoded(last);
    let stem = match last.rsplit_once('.') {
        Some((stem, _)) => stem,
        None => last.as_str(),
    };
    let mut clean = String::new();
    for c in stem
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        if clean.len() + c.len_utf8() > MAX_STEM {
            break;
        }
        clean.push(c);
    }
    let clean = clean.trim_matches('.');
    let stem = if clean.is_empty() {
        "image".to_string()
    } else if is_windows_device_name(clean) {
        format!("_{clean}")
    } else {
        clean.to_string()
    };
    format!("{stem}.{ext}")
}

/// Whether `stem` is a name Windows keeps for a device (`CON`, `NUL`,
/// `COM1`, …), which no file may have, whatever its extension.
fn is_windows_device_name(stem: &str) -> bool {
    let base = stem.split('.').next().unwrap_or("").to_ascii_uppercase();
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((base.starts_with("COM") || base.starts_with("LPT"))
            && base.len() == 4
            && base.as_bytes()[3].is_ascii_digit()
            && base.as_bytes()[3] != b'0')
}

/// Download the image at `url`: `http(s)` only, at most [`MAX_IMAGE`], one
/// of [`image_extension`]'s types, within [`FETCH_DEADLINE`].
pub fn fetch_image(url: &str) -> Result<Image, String> {
    // A thread of its own: this program's main already runs a runtime,
    // and one may not be started within another.
    let url = url.to_string();
    std::thread::spawn(move || fetch_blocking(&url))
        .join()
        .map_err(|_| "the download failed".to_string())?
}

/// Follow at most 5 redirects, each held to the same rule as the address
/// the browser gave, so a site cannot bounce the download to `localhost`.
fn redirect_rule(attempt: reqwest::redirect::Attempt) -> reqwest::redirect::Action {
    match redirect_problem(attempt.previous().len(), attempt.url().as_str()) {
        Some(problem) => attempt.error(problem),
        None => attempt.follow(),
    }
}

/// An error with its causes, so a refused redirect says why.
fn why(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(&format!(": {cause}"));
        source = cause.source();
    }
    text
}

/// Why a redirect to `url`, after `followed` others, is not followed.
fn redirect_problem(followed: usize, url: &str) -> Option<String> {
    if followed >= 5 {
        return Some("too many redirects".to_string());
    }
    pocketdesk_core::protocol::link::link_problem(url).map(|p| format!("a redirect: {p}"))
}

fn fetch_blocking(url: &str) -> Result<Image, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let client = reqwest::Client::builder()
            .timeout(FETCH_DEADLINE)
            .redirect(reqwest::redirect::Policy::custom(redirect_rule))
            .build()
            .map_err(|e| e.to_string())?;
        let mut response = client.get(url).send().await.map_err(|e| why(&e))?;
        if !response.status().is_success() {
            return Err(format!("the site answered {}", response.status()));
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let Some(ext) = image_extension(&content_type) else {
            return Err(
                "that is not a picture Pocketdesk sends (PNG, JPEG, GIF, WebP, AVIF or BMP)"
                    .to_string(),
            );
        };
        if response
            .content_length()
            .is_some_and(|n| n as usize > MAX_IMAGE)
        {
            return Err("the image is larger than 32 MB".to_string());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
            bytes.extend_from_slice(&chunk);
            if bytes.len() > MAX_IMAGE {
                return Err("the image is larger than 32 MB".to_string());
            }
        }
        Ok(Image {
            name: image_name(response.url().as_str(), ext),
            bytes,
        })
    })
}

/// Serve the browser until it closes the pipe. The exit code.
pub fn run() -> i32 {
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    let dir = match crate::state::state_dir() {
        Ok(dir) => dir,
        Err(e) => {
            // Said before any request is read: the browser takes the first
            // message as the answer.
            let _ = write_message(
                &mut stdout,
                &err(format!("Pocketdesk could not find its folder ({e:#})")),
            );
            return 1;
        }
    };
    loop {
        let request = match read_message(&mut stdin) {
            Ok(Some(request)) => request,
            Ok(None) => return 0,
            Err(e) => {
                let _ = write_message(&mut stdout, &err(format!("A bad message: {e}")));
                return 1;
            }
        };
        let running = pocketdesk_core::local::server::is_running(&dir);
        let answer = handle(&dir, &request, running, &fetch_image);
        if write_message(&mut stdout, &answer).is_err() {
            return 0;
        }
    }
}

/// The host's registration for a browser family.
fn manifest(exe: &Path, firefox: bool) -> Value {
    let mut manifest = json!({
        "name": HOST_NAME,
        "description": "Pocketdesk: send links and images to your phone",
        "path": exe.display().to_string(),
        "type": "stdio",
    });
    if firefox {
        manifest["allowed_extensions"] = json!([FIREFOX_EXTENSION_ID]);
    } else {
        manifest["allowed_origins"] =
            json!([format!("chrome-extension://{CHROMIUM_EXTENSION_ID}/")]);
    }
    manifest
}

/// A browser this program can register with: its name, whether it is
/// Firefox, and where it looks for hosts.
pub struct Browser {
    pub name: &'static str,
    pub firefox: bool,
    /// Linux: the folder the browser reads `<HOST_NAME>.json` from; its
    /// parent existing says the browser is installed.
    pub folder: PathBuf,
    /// Windows: the key under HKCU whose default value names the manifest.
    pub key: &'static str,
}

/// The browsers this program knows, under `home` (Linux) or `config`.
pub fn browsers(home: &Path) -> Vec<Browser> {
    let chromium = |name, dir: &str, key| Browser {
        name,
        firefox: false,
        folder: home.join(".config").join(dir).join("NativeMessagingHosts"),
        key,
    };
    vec![
        chromium(
            "Chrome",
            "google-chrome",
            r"Software\Google\Chrome\NativeMessagingHosts",
        ),
        chromium(
            "Chromium",
            "chromium",
            r"Software\Chromium\NativeMessagingHosts",
        ),
        chromium(
            "Edge",
            "microsoft-edge",
            r"Software\Microsoft\Edge\NativeMessagingHosts",
        ),
        chromium(
            "Brave",
            "BraveSoftware/Brave-Browser",
            r"Software\BraveSoftware\Brave-Browser\NativeMessagingHosts",
        ),
        chromium(
            "Vivaldi",
            "vivaldi",
            r"Software\Vivaldi\NativeMessagingHosts",
        ),
        Browser {
            name: "Firefox",
            firefox: true,
            folder: home.join(".mozilla").join("native-messaging-hosts"),
            key: r"Software\Mozilla\NativeMessagingHosts",
        },
    ]
}

/// Register the host with each browser found under `home` (Linux): its
/// manifest in the browser's folder. The browsers registered.
pub fn install_linux(home: &Path, exe: &Path) -> io::Result<Vec<&'static str>> {
    let mut done = Vec::new();
    for browser in browsers(home) {
        let installed = browser.folder.parent().is_some_and(Path::exists);
        if !installed {
            continue;
        }
        std::fs::create_dir_all(&browser.folder)?;
        let text = serde_json::to_string_pretty(&manifest(exe, browser.firefox))
            .map_err(io::Error::other)?;
        std::fs::write(browser.folder.join(format!("{HOST_NAME}.json")), text)?;
        done.push(browser.name);
    }
    Ok(done)
}

/// Undo [`install_linux`].
pub fn remove_linux(home: &Path) {
    for browser in browsers(home) {
        let _ = std::fs::remove_file(browser.folder.join(format!("{HOST_NAME}.json")));
    }
}

/// Register the host with every Chromium browser and Firefox for this user
/// (Windows): the two manifests in the config folder, and a key per browser
/// naming one. The browsers registered.
#[cfg(windows)]
pub fn install_windows(config_dir: &Path, exe: &Path) -> io::Result<Vec<&'static str>> {
    let dir = config_dir.join("browser");
    std::fs::create_dir_all(&dir)?;
    let mut done = Vec::new();
    for browser in browsers(Path::new("")) {
        let file = dir.join(if browser.firefox {
            format!("{HOST_NAME}.firefox.json")
        } else {
            format!("{HOST_NAME}.chromium.json")
        });
        let text = serde_json::to_string_pretty(&manifest(exe, browser.firefox))
            .map_err(io::Error::other)?;
        std::fs::write(&file, text)?;
        crate::platform::set_user_default_value(
            &format!(r"{}\{HOST_NAME}", browser.key),
            &file.display().to_string(),
        )
        .map_err(io::Error::other)?;
        done.push(browser.name);
    }
    Ok(done)
}

/// Undo [`install_windows`].
#[cfg(windows)]
pub fn remove_windows(config_dir: &Path) {
    for browser in browsers(Path::new("")) {
        crate::platform::delete_user_key(&format!(r"{}\{HOST_NAME}", browser.key));
    }
    let _ = std::fs::remove_dir_all(config_dir.join("browser"));
}

/// What `registered.json` in the config folder's `browser/` says: the
/// browsers the host was last registered with (the desktop app reads it).
pub const REGISTERED_FILE: &str = "registered.json";

/// Register the host with the browsers found, and say which in
/// [`REGISTERED_FILE`]. The browsers registered.
pub fn install() -> anyhow::Result<Vec<&'static str>> {
    let config = pocketdesk_core::local::paths::config_dir()?;
    let exe = std::env::current_exe()?;
    #[cfg(windows)]
    let done = install_windows(&config, &exe)?;
    #[cfg(not(windows))]
    let done = {
        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home folder"))?;
        install_linux(&home, &exe)?
    };
    let dir = config.join("browser");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join(REGISTERED_FILE),
        serde_json::to_string(&json!({ "browsers": done }))?,
    )?;
    Ok(done)
}

/// Undo [`install`].
pub fn remove() -> anyhow::Result<()> {
    let config = pocketdesk_core::local::paths::config_dir()?;
    #[cfg(windows)]
    remove_windows(&config);
    #[cfg(not(windows))]
    if let Some(home) = dirs::home_dir() {
        remove_linux(&home);
    }
    let _ = std::fs::remove_dir_all(config.join("browser"));
    Ok(())
}

/// `pocketdesk browser-host --install | --remove`: say what was done.
pub fn run_install(remove_it: bool) -> anyhow::Result<()> {
    if remove_it {
        remove()?;
        println!("Removed Pocketdesk from the browsers");
        return Ok(());
    }
    let done = install()?;
    if done.is_empty() {
        println!("No browser found to register with");
    } else {
        println!("Pocketdesk can now be reached from {}", done.join(", "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-browser-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_browser_is_known_by_its_arguments() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(launched_by_browser(&args(&[
            "pd",
            "chrome-extension://abc/",
            "--parent-window=0"
        ])));
        assert!(launched_by_browser(&args(&[
            "pd",
            "/home/u/.mozilla/x.json",
            FIREFOX_EXTENSION_ID
        ])));
        assert!(!launched_by_browser(&args(&["pd", "status"])));
        assert!(!launched_by_browser(&args(&[
            "pd",
            "/x.json",
            "someone-else@example.org"
        ])));
    }

    #[test]
    fn a_message_reads_back_as_written_and_a_long_one_is_refused() {
        let mut wire = Vec::new();
        write_message(
            &mut wire,
            &json!({"send": "link", "url": "https://example.org"}),
        )
        .unwrap();
        let mut reader = io::Cursor::new(wire);
        assert_eq!(
            read_message(&mut reader).unwrap(),
            Some(json!({"send": "link", "url": "https://example.org"}))
        );
        assert_eq!(read_message(&mut reader).unwrap(), None, "the pipe closed");
        let mut long = ((MAX_MESSAGE + 1) as u32).to_ne_bytes().to_vec();
        long.extend_from_slice(b"{}");
        assert!(read_message(&mut io::Cursor::new(long)).is_err());
        let short = [5u8, 0, 0, 0, b'{'];
        assert!(
            read_message(&mut io::Cursor::new(short)).is_err(),
            "cut short"
        );
    }

    #[test]
    fn only_pictures_are_sent_whatever_their_header_says() {
        assert_eq!(image_extension("image/png"), Some("png"));
        assert_eq!(image_extension("IMAGE/JPEG; charset=x"), Some("jpg"));
        assert_eq!(image_extension("image/webp"), Some("webp"));
        assert_eq!(
            image_extension("image/svg+xml"),
            None,
            "SVG can carry script"
        );
        assert_eq!(image_extension("image/x-icon"), None);
        assert_eq!(image_extension("text/html"), None);
        assert_eq!(image_extension(""), None);
    }

    #[test]
    fn an_image_gets_a_name_of_its_own() {
        assert_eq!(
            image_name("https://example.org/a/cat.png?x=1", "png"),
            "cat.png"
        );
        assert_eq!(
            image_name("https://example.org/a/photo", "webp"),
            "photo.webp"
        );
        assert_eq!(image_name("https://example.org/", "gif"), "image.gif");
        assert_eq!(
            image_name("https://example.org/a/update.apk", "png"),
            "update.png",
            "the content type decides the extension"
        );
        assert_eq!(image_name("https://example.org/x.html", "jpg"), "x.jpg");
        assert_eq!(
            image_name("https://example.org/cat%20photo.png", "png"),
            "catphoto.png",
            "escapes are decoded before the name is cleaned"
        );
        assert_eq!(
            image_name("https://example.org/..%2F..%2Fx.png", "png"),
            "x.png",
            "a decoded slash is not a folder"
        );
        assert_eq!(image_name("https://example.org/CON.png", "png"), "_CON.png");
        assert_eq!(
            image_name("https://example.org/com1.gif", "gif"),
            "_com1.gif"
        );
        assert_eq!(
            image_name("https://example.org/COM0.gif", "gif"),
            "COM0.gif"
        );
        let long = format!("https://example.org/{}.png", "\u{10348}".repeat(100));
        assert!(image_name(&long, "png").len() <= MAX_STEM + 4);
        assert!(!image_name("https://example.org/a\\b<c>.png", "png").contains(['\\', '<', '>']));
    }

    #[test]
    fn a_redirect_is_held_to_the_same_rule() {
        assert_eq!(redirect_problem(0, "https://example.org/cat.png"), None);
        assert!(redirect_problem(0, "http://127.0.0.1:8080/x.png").is_some());
        assert!(redirect_problem(1, "http://localhost/x.png").is_some());
        assert!(redirect_problem(0, "file:///etc/passwd").is_some());
        assert!(redirect_problem(5, "https://example.org/cat.png").is_some());
    }

    #[test]
    fn nothing_is_sent_without_a_server_or_a_phone() {
        let dir = temp("refused");
        let never = |_: &str| -> Result<Image, String> { panic!("no fetch without a phone") };
        let request = json!({"send": "image", "url": "https://example.org/cat.png"});
        assert!(
            handle(&dir, &request, false, &never)["error"]
                .as_str()
                .unwrap()
                .contains("not running")
        );
        assert!(
            handle(&dir, &request, true, &never)["error"]
                .as_str()
                .unwrap()
                .contains("No phone")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_kept_image_is_owner_only_under_its_own_name() {
        let dir = temp("keep");
        let path = keep_image(
            &dir,
            &Image {
                bytes: vec![1, 2, 3],
                name: "cat.png".into(),
            },
        )
        .unwrap();
        assert!(path.starts_with(&dir));
        assert_eq!(path.file_name().unwrap(), "cat.png");
        assert_eq!(std::fs::read(&path).unwrap(), vec![1, 2, 3]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&dir), 0o700);
            assert_eq!(mode(&path), 0o600);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn images_never_wait_in_the_shared_temporary_folder_on_linux() {
        let dir = images_dir().unwrap();
        #[cfg(not(windows))]
        assert!(!dir.starts_with("/tmp"), "{}", dir.display());
        assert!(dir.ends_with("pocketdesk-browser"));
    }

    #[test]
    fn a_file_where_the_folder_should_be_is_left_alone() {
        let base = temp("not-a-folder");
        let dir = base.join("pocketdesk-browser");
        std::fs::write(&dir, b"someone else's").unwrap();
        let image = Image {
            bytes: vec![1],
            name: "cat.png".into(),
        };
        assert!(keep_image(&dir, &image).is_err());
        assert_eq!(std::fs::read(&dir).unwrap(), b"someone else's");
        #[cfg(unix)]
        {
            let target = base.join("victim");
            std::fs::create_dir_all(target.join("old")).unwrap();
            let link = base.join("linked");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert!(keep_image(&link, &image).is_err(), "a link is not followed");
            assert!(target.join("old").exists());
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn each_installed_browser_gets_its_manifest_and_others_none() {
        let home = temp("home");
        std::fs::create_dir_all(home.join(".config/google-chrome")).unwrap();
        std::fs::create_dir_all(home.join(".mozilla")).unwrap();
        let exe = Path::new("/usr/bin/pocketdesk");
        let done = install_linux(&home, exe).unwrap();
        assert_eq!(done, vec!["Chrome", "Firefox"]);
        let chrome: Value = serde_json::from_str(
            &std::fs::read_to_string(
                home.join(".config/google-chrome/NativeMessagingHosts/dev.pocketdesk.host.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            chrome["allowed_origins"][0],
            format!("chrome-extension://{CHROMIUM_EXTENSION_ID}/")
        );
        assert_eq!(chrome["path"], "/usr/bin/pocketdesk");
        let firefox: Value = serde_json::from_str(
            &std::fs::read_to_string(
                home.join(".mozilla/native-messaging-hosts/dev.pocketdesk.host.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(firefox["allowed_extensions"][0], FIREFOX_EXTENSION_ID);
        assert!(
            !home.join(".config/chromium").exists(),
            "not installed, not touched"
        );
        remove_linux(&home);
        assert!(
            !home
                .join(".mozilla/native-messaging-hosts/dev.pocketdesk.host.json")
                .exists()
        );
        let _ = std::fs::remove_dir_all(&home);
    }
}
