use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use serde_json::json;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Browsing the desktop's home folder from the phone (F7b). A reply names
/// every folder and file by the path the phone may ask for next, so the
/// phone never builds a path itself (and never needs to know `\` from `/`).
#[derive(Debug)]
pub struct FileBrowsePlugin;

impl Default for FileBrowsePlugin {
    fn default() -> Self {
        Self
    }
}

impl FileBrowsePlugin {
    pub fn new() -> Self {
        Self
    }
}

/// The real path `requested` names inside `home`, and the same place in the
/// form a request may name it (`home` as given, then the rest; not the
/// `\?\` form Windows resolves to). `""` and `"/"` are home itself.
/// The real path is resolved, so a symlink (or a Windows junction) inside
/// home that points elsewhere cannot lead out of it.
fn resolve_in(home: &Path, requested: &str) -> Option<(PathBuf, PathBuf)> {
    resolve_in_with(home, requested, &private_dirs())
}

/// The server's own folders, as real paths: its state (keys, the trust
/// store, the PIN) and its config. Both may lie inside home.
pub(crate) fn private_dirs() -> Vec<PathBuf> {
    [
        pocketdesk_core::local::paths::state_dir(),
        pocketdesk_core::local::paths::config_dir(),
    ]
    .into_iter()
    .filter_map(|dir| dir.ok()?.canonicalize().ok())
    .collect()
}

/// [`resolve_in`], also refusing anything at or below a folder in
/// `private` (real paths): the server's own state and config.
fn resolve_in_with(
    home: &Path,
    requested: &str,
    private: &[PathBuf],
) -> Option<(PathBuf, PathBuf)> {
    let real_home = home.canonicalize().ok()?;
    if requested.is_empty() || requested == "/" {
        return Some((real_home, home.to_path_buf()));
    }
    if !lexically_inside(home, requested) || names_a_device(home, requested) {
        return None;
    }
    let real = Path::new(requested).canonicalize().ok()?;
    let rest = real.strip_prefix(&real_home).ok()?;
    // Nothing hidden is reachable, by path any more than by listing: home's
    // hidden places hold its secrets (`.ssh`, `AppData`, this server's own
    // keys), and a phone allowed files is not allowed those.
    let mut at = real_home.clone();
    for part in rest.components() {
        at.push(part);
        let name = part.as_os_str().to_string_lossy();
        if is_hidden(&name, &std::fs::symlink_metadata(&at).ok()?) {
            return None;
        }
    }
    if private.iter().any(|p| real.starts_with(p)) {
        return None;
    }
    let shown = if rest.as_os_str().is_empty() {
        home.to_path_buf()
    } else {
        home.join(rest)
    };
    Some((real, shown))
}

/// Checked as text before the filesystem sees it: opening a path is already
/// an action. On Windows a UNC path (\host\share) makes the system connect
/// out and offer the user's NTLM credentials, and a device path (\.\pipe\x)
/// opens a device, before any check that follows could refuse it. So it
/// must be under home as written, and contain no "..".
fn lexically_inside(home: &Path, requested: &str) -> bool {
    let path = Path::new(requested);
    path.starts_with(home) && !path.components().any(|c| matches!(c, Component::ParentDir))
}

/// Whether a part of `requested` below home is a Windows device name (COM1,
/// NUL, CONIN$...), which names a device in any folder: opening it opens
/// the device. Checked as text, like [`lexically_inside`].
fn names_a_device(home: &Path, requested: &str) -> bool {
    Path::new(requested)
        .strip_prefix(home)
        .map(|rest| {
            rest.components().any(|c| match c {
                Component::Normal(name) => {
                    crate::plugins::share::is_reserved_name(&name.to_string_lossy())
                }
                _ => false,
            })
        })
        .unwrap_or(false)
}

/// The folder above `shown`, or `None` at (or above) home.
fn parent_of(home: &Path, shown: &Path) -> Option<String> {
    if shown == home {
        return None;
    }
    shown
        .parent()
        .filter(|p| p.starts_with(home))
        .map(|p| p.display().to_string())
}

/// The file a phone may download: a regular file inside home, as its real
/// path and its size.
pub(crate) fn resolve_file(requested: &str) -> std::result::Result<(PathBuf, u64), String> {
    let home = dirs::home_dir().ok_or("this computer has no home folder")?;
    resolve_file_in(&home, requested)
}

fn resolve_file_in(home: &Path, requested: &str) -> std::result::Result<(PathBuf, u64), String> {
    if requested.is_empty() {
        return Err("not a file".to_string());
    }
    let (real, _) = resolve_in(home, requested).ok_or("not a file in the home folder")?;
    let metadata = std::fs::metadata(&real).map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("not a file".to_string());
    }
    Ok((real, metadata.len()))
}

/// The folder `requested` names below home, for a phone that asks for it
/// whole (`share::FOLDER_FETCH`): its real path and the path as shown, by
/// the browser's own rules. Home itself is refused: it holds too much to
/// mean.
pub(crate) fn resolve_folder(requested: &str) -> std::result::Result<(PathBuf, PathBuf), String> {
    let home = dirs::home_dir().ok_or("this computer has no home folder")?;
    resolve_folder_in(&home, requested)
}

fn resolve_folder_in(
    home: &Path,
    requested: &str,
) -> std::result::Result<(PathBuf, PathBuf), String> {
    let (real, shown) = resolve_in(home, requested).ok_or("not a folder in the home folder")?;
    if home.canonicalize().is_ok_and(|h| h == real) {
        return Err("not the whole home folder".to_string());
    }
    if !real.is_dir() {
        return Err("not a folder".to_string());
    }
    Ok((real, shown))
}

/// Check again, as it is fetched, a file of a folder the phone asked for
/// (`within`, its real root): still a regular file, not a link, inside the
/// folder, with nothing hidden on the way and not the server's own. The
/// folder was walked by these rules; this holds them at the moment the file
/// is opened, in case something changed since.
pub(crate) fn still_within(path: &Path, within: &Path) -> std::result::Result<(), String> {
    still_within_with(path, within, &private_dirs())
}

fn still_within_with(
    path: &Path,
    within: &Path,
    private: &[PathBuf],
) -> std::result::Result<(), String> {
    const GONE: &str = "the file is no longer where it was offered";
    let meta = std::fs::symlink_metadata(path).map_err(|_| GONE)?;
    if !meta.is_file() {
        return Err(GONE.to_string());
    }
    let real = path.canonicalize().map_err(|_| GONE)?;
    let rest = real.strip_prefix(within).map_err(|_| GONE)?;
    let mut at = within.to_path_buf();
    for part in rest.components() {
        at.push(part);
        let name = part.as_os_str().to_string_lossy();
        let meta = std::fs::symlink_metadata(&at).map_err(|_| GONE)?;
        if is_hidden(&name, &meta) {
            return Err(GONE.to_string());
        }
    }
    if private.iter().any(|p| real.starts_with(p)) {
        return Err(GONE.to_string());
    }
    Ok(())
}

/// The reply to a request for `requested`, browsing inside `home`.
fn browse_reply(home: &Path, requested: &str) -> serde_json::Value {
    let Some((real, shown)) = resolve_in(home, requested) else {
        // A path the request may name but that is gone keeps its way up.
        let parent = lexically_inside(home, requested)
            .then(|| parent_of(home, Path::new(requested)))
            .flatten();
        return json!({
            "error": "This folder cannot be opened.",
            "path": requested,
            "parent": parent,
            "files": [],
        });
    };
    let parent = parent_of(home, &shown);
    match list_directory(&real, &shown) {
        Ok(files) => {
            let (files, truncated) = within_budget(files, LISTING_BUDGET);
            json!({
                "path": shown.display().to_string(),
                "parent": parent,
                "files": files,
                "truncated": truncated,
            })
        }
        Err(e) => json!({
            "error": e.to_string(),
            "path": shown.display().to_string(),
            "parent": parent,
            "files": [],
        }),
    }
}

/// The most a listing's entries may take, encoded. A control packet is at
/// most 4 MiB on both ends, and a reply over it would end the phone's
/// connection, so a huge folder is listed in part (R3 review).
const LISTING_BUDGET: usize = 2 * 1024 * 1024;

/// The leading entries that fit in `budget` bytes of JSON, and whether any
/// were left out.
fn within_budget(entries: Vec<serde_json::Value>, budget: usize) -> (Vec<serde_json::Value>, bool) {
    let mut used = 0;
    let mut kept = Vec::new();
    let total = entries.len();
    for entry in entries {
        used += entry.to_string().len() + 1;
        if used > budget {
            break;
        }
        kept.push(entry);
    }
    let truncated = kept.len() < total;
    (kept, truncated)
}

/// Entries the system keeps out of sight: a name starting with a dot, and
/// on Windows anything marked hidden. A home folder is full of them
/// (`.cache`, `AppData`, `desktop.ini`), and on a phone they bury the
/// folders a person means to open.
pub(crate) fn is_hidden(name: &str, metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        if metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0 {
            return true;
        }
    }
    #[cfg(not(windows))]
    let _ = metadata;
    name.starts_with('.')
}

/// The folder's entries, folders first and then by name ignoring case.
/// Each carries the path a request names it by: `shown` and its name.
fn list_directory(real: &Path, shown: &Path) -> std::io::Result<Vec<serde_json::Value>> {
    let mut entries: Vec<(bool, String, u64, u64)> = std::fs::read_dir(real)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().to_string();
            // Hidden is the entry's own mark; what it is (a folder, its
            // size) is what a link or junction leads to, so a link to a
            // folder opens like one.
            if is_hidden(&name, &entry.metadata().ok()?) {
                return None;
            }
            let metadata = std::fs::metadata(entry.path())
                .or_else(|_| entry.metadata())
                .ok()?;
            let is_dir = metadata.is_dir();
            let size = if is_dir { 0 } else { metadata.len() };
            let modified = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            Some((is_dir, name, size, modified))
        })
        .collect();
    entries.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });
    Ok(entries
        .into_iter()
        .map(|(is_dir, name, size, modified)| {
            json!({
                "name": name,
                "isDirectory": is_dir,
                "size": size,
                "modified": modified,
                "path": shown.join(&name).display().to_string(),
            })
        })
        .collect())
}

#[async_trait::async_trait]
impl Plugin for FileBrowsePlugin {
    fn name(&self) -> &'static str {
        "filebrowse"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.filebrowse.request"]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.filebrowse.response"]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type.as_str() == "kdeconnect.filebrowse.request" {
            let requested = packet
                .body
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let body = match dirs::home_dir() {
                Some(home) => browse_reply(&home, requested),
                None => json!({
                    "error": "This computer has no home folder.",
                    "path": requested,
                    "parent": null,
                    "files": [],
                }),
            };
            let response = NetworkPacket::new("kdeconnect.filebrowse.response").with_body(body);
            sender.send_packet(&response).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_huge_folder_is_listed_in_part_within_one_packet() {
        let entries: Vec<_> = (0..1000)
            .map(|i| json!({ "name": format!("file-{i:04}"), "path": "x".repeat(100) }))
            .collect();
        let one = entries[0].to_string().len() + 1;
        let (kept, truncated) = within_budget(entries.clone(), one * 10);
        assert_eq!(kept.len(), 10);
        assert!(truncated);
        assert_eq!(kept[9]["name"], "file-0009", "the leading ones, in order");
        let (kept, truncated) = within_budget(entries, LISTING_BUDGET);
        assert_eq!(kept.len(), 1000);
        assert!(!truncated);
    }

    fn temp_home(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-browse-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        dir
    }

    #[test]
    fn only_real_folders_inside_home_can_be_listed() {
        let home = temp_home("inside");
        let docs = home.join("docs");
        let real_docs = docs.canonicalize().unwrap();
        assert_eq!(
            resolve_in(&home, docs.to_str().unwrap()).map(|(real, _)| real),
            Some(real_docs)
        );
        assert_eq!(
            resolve_in(&home, "").map(|(real, _)| real),
            Some(home.canonicalize().unwrap())
        );
        let up = docs.join("..").join("..");
        assert_eq!(
            resolve_in(&home, up.to_str().unwrap()).map(|(real, _)| real),
            None
        );
        let outside = std::env::temp_dir();
        assert_eq!(
            resolve_in(&home, outside.to_str().unwrap()).map(|(real, _)| real),
            None
        );
        assert_eq!(
            resolve_in(&home, home.join("missing").to_str().unwrap()).map(|(real, _)| real),
            None,
            "a path that does not exist"
        );
    }

    #[test]
    fn network_and_device_paths_are_refused_without_being_opened() {
        let home = temp_home("unc");
        for path in [
            r"\\192.0.2.1\share",
            r"\\?\UNC\192.0.2.1\share",
            r"\\.\pipe\x",
            "//192.0.2.1/share",
            "relative/path",
        ] {
            assert_eq!(
                resolve_in(&home, path).map(|(real, _)| real),
                None,
                "{path}"
            );
        }
    }

    fn names(reply: &serde_json::Value) -> Vec<String> {
        reply["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["name"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn a_reply_says_where_it_is_and_how_to_go_up() {
        let home = temp_home("where");
        std::fs::write(home.join("a.txt"), "a").unwrap();
        std::fs::write(home.join("docs").join("b.txt"), "bb").unwrap();

        let top = browse_reply(&home, "");
        assert_eq!(top["path"], home.display().to_string());
        assert!(top["parent"].is_null(), "home has no way up");
        assert_eq!(names(&top), ["docs", "a.txt"]);

        // The phone opens a folder by the path the reply gave it, as is.
        let docs_path = top["files"][0]["path"].as_str().unwrap().to_string();
        let docs = browse_reply(&home, &docs_path);
        assert!(docs.get("error").is_none(), "{docs}");
        assert_eq!(docs["path"], docs_path);
        assert_eq!(docs["parent"], home.display().to_string());
        assert_eq!(names(&docs), ["b.txt"]);
        let file_path = docs["files"][0]["path"].as_str().unwrap();
        assert_eq!(
            resolve_file_in(&home, file_path),
            Ok((home.join("docs").join("b.txt").canonicalize().unwrap(), 2))
        );
    }

    #[test]
    fn nothing_hidden_can_be_opened_or_fetched_by_path() {
        let home = temp_home("hiddenpath");
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::write(home.join(".ssh").join("id_ed25519"), "secret").unwrap();
        std::fs::write(home.join("docs").join(".env"), "secret").unwrap();
        let key = home.join(".ssh").join("id_ed25519");
        assert!(resolve_file_in(&home, key.to_str().unwrap()).is_err());
        let env = home.join("docs").join(".env");
        assert!(resolve_file_in(&home, env.to_str().unwrap()).is_err());
        let ssh = browse_reply(&home, home.join(".ssh").to_str().unwrap());
        assert!(ssh["error"].is_string(), "{ssh}");
    }

    #[cfg(windows)]
    #[test]
    fn a_folder_windows_marks_hidden_cannot_be_opened_by_path() {
        use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_HIDDEN, SetFileAttributesW};
        use windows::core::HSTRING;
        let home = temp_home("winhiddenpath");
        let app_data = home.join("AppData");
        std::fs::create_dir_all(app_data.join("Local")).unwrap();
        std::fs::write(app_data.join("Local").join("key"), "secret").unwrap();
        // SAFETY: a plain call with a valid, NUL-terminated path.
        unsafe { SetFileAttributesW(&HSTRING::from(app_data.as_os_str()), FILE_ATTRIBUTE_HIDDEN) }
            .unwrap();
        let key = app_data.join("Local").join("key");
        assert!(resolve_file_in(&home, key.to_str().unwrap()).is_err());
    }

    #[test]
    fn the_servers_own_files_cannot_be_fetched() {
        let home = temp_home("private");
        let state = home.join("docs").join("pocketdesk");
        std::fs::create_dir_all(state.join("certs")).unwrap();
        std::fs::write(state.join("certs").join("identity.key"), "secret").unwrap();
        let private = [state.canonicalize().unwrap()];
        let key = state.join("certs").join("identity.key");
        assert!(resolve_in_with(&home, key.to_str().unwrap(), &private).is_none());
        assert!(resolve_in_with(&home, state.to_str().unwrap(), &private).is_none());
        let docs = home.join("docs");
        assert!(resolve_in_with(&home, docs.to_str().unwrap(), &private).is_some());
    }

    #[test]
    fn a_windows_device_name_below_home_is_refused_as_text() {
        let home = temp_home("devices");
        for name in ["COM1", "lpt1.txt", "CONIN$", "nul"] {
            let path = home.join("docs").join(name);
            assert!(names_a_device(&home, path.to_str().unwrap()), "{name}");
            assert!(
                resolve_in(&home, path.to_str().unwrap()).is_none(),
                "{name}"
            );
        }
        let fine = home.join("docs").join("COM10.txt");
        assert!(!names_a_device(&home, fine.to_str().unwrap()));
    }

    #[cfg(windows)]
    #[test]
    fn a_junction_to_a_folder_is_listed_as_a_folder() {
        let home = temp_home("junction");
        let target = home.join("docs");
        let link = home.join("Pictures");
        let made = std::process::Command::new(r"C:\Windows\System32\cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
        let reply = browse_reply(&home, "");
        let pictures = reply["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == "Pictures")
            .cloned()
            .unwrap();
        assert_eq!(pictures["isDirectory"], true, "{pictures}");
    }

    #[test]
    fn hidden_entries_are_not_listed() {
        let home = temp_home("hidden");
        std::fs::create_dir(home.join(".cache")).unwrap();
        std::fs::write(home.join(".bashrc"), "x").unwrap();
        std::fs::write(home.join("notes.txt"), "x").unwrap();
        assert_eq!(names(&browse_reply(&home, "")), ["docs", "notes.txt"]);
    }

    #[cfg(windows)]
    #[test]
    fn a_file_windows_marks_hidden_is_not_listed() {
        use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_HIDDEN, SetFileAttributesW};
        use windows::core::HSTRING;
        let home = temp_home("winhidden");
        let secret = home.join("desktop.ini");
        std::fs::write(&secret, "x").unwrap();
        // SAFETY: a plain call with a valid, NUL-terminated path.
        unsafe { SetFileAttributesW(&HSTRING::from(secret.as_os_str()), FILE_ATTRIBUTE_HIDDEN) }
            .unwrap();
        assert_eq!(names(&browse_reply(&home, "")), ["docs"]);
    }

    #[test]
    fn folders_come_first_then_names_regardless_of_case() {
        let home = temp_home("sort");
        std::fs::create_dir(home.join("Zeta")).unwrap();
        for f in ["b.txt", "A.txt", "c.txt"] {
            std::fs::write(home.join(f), "x").unwrap();
        }
        assert_eq!(
            names(&browse_reply(&home, "")),
            ["docs", "Zeta", "A.txt", "b.txt", "c.txt"]
        );
    }

    #[test]
    fn a_missing_folder_answers_an_error_that_keeps_the_way_up() {
        let home = temp_home("missing");
        let missing = home.join("docs").join("gone");
        let reply = browse_reply(&home, missing.to_str().unwrap());
        assert!(reply["error"].is_string());
        assert_eq!(reply["files"], serde_json::json!([]));
        assert_eq!(reply["parent"], home.join("docs").display().to_string());

        let outside = browse_reply(&home, r"\\192.0.2.1\share");
        assert!(outside["error"].is_string());
        assert!(outside["parent"].is_null());
    }

    #[test]
    fn only_a_regular_file_inside_home_can_be_fetched() {
        let home = temp_home("fetch");
        std::fs::write(home.join("a.txt"), "abc").unwrap();
        let a = home.join("a.txt");
        assert_eq!(
            resolve_file_in(&home, a.to_str().unwrap()),
            Ok((a.canonicalize().unwrap(), 3))
        );
        for bad in [
            home.join("docs"),
            home.join("missing.txt"),
            home.join("docs").join("..").join("a.txt"),
            std::env::temp_dir().join("x.txt"),
        ] {
            assert!(
                resolve_file_in(&home, bad.to_str().unwrap()).is_err(),
                "{}",
                bad.display()
            );
        }
        assert!(resolve_file_in(&home, "").is_err(), "home is a folder");
    }

    #[test]
    fn a_file_of_an_asked_folder_is_checked_again_when_fetched() {
        let home = temp_home("still-within");
        let root = home.join("docs");
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::create_dir_all(root.join(".h")).unwrap();
        std::fs::write(root.join("a").join("f.txt"), "f").unwrap();
        std::fs::write(root.join(".h").join("f.txt"), "f").unwrap();
        std::fs::write(home.join("outside.txt"), "o").unwrap();
        let within = root.canonicalize().unwrap();
        assert!(still_within_with(&root.join("a").join("f.txt"), &within, &[]).is_ok());
        for bad in [
            root.join(".h").join("f.txt"),
            home.join("outside.txt"),
            root.join("a").join("missing.txt"),
        ] {
            assert!(
                still_within_with(&bad, &within, &[]).is_err(),
                "{}",
                bad.display()
            );
        }
        let private = [root.join("a").canonicalize().unwrap()];
        assert!(still_within_with(&root.join("a").join("f.txt"), &within, &private).is_err());
    }

    #[test]
    fn only_a_real_folder_below_home_can_be_asked_for_whole() {
        let home = temp_home("fetch-folder");
        std::fs::write(home.join("a.txt"), "abc").unwrap();
        std::fs::create_dir_all(home.join(".secret")).unwrap();
        let docs = home.join("docs");
        let (real, shown) = resolve_folder_in(&home, docs.to_str().unwrap()).unwrap();
        assert_eq!(real, docs.canonicalize().unwrap());
        assert_eq!(shown, docs);
        for bad in [
            String::new(),
            "/".to_string(),
            home.to_str().unwrap().to_string(),
            home.join("a.txt").to_str().unwrap().to_string(),
            home.join(".secret").to_str().unwrap().to_string(),
            home.join("missing").to_str().unwrap().to_string(),
            std::env::temp_dir().to_str().unwrap().to_string(),
        ] {
            assert!(resolve_folder_in(&home, &bad).is_err(), "{bad:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_link_cannot_fetch_a_file_outside_home() {
        let home = temp_home("fetchlink");
        let link = home.join("passwd");
        std::os::unix::fs::symlink("/etc/passwd", &link).unwrap();
        assert!(resolve_file_in(&home, link.to_str().unwrap()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_inside_home_cannot_lead_out_of_it() {
        let home = temp_home("link");
        let link = home.join("escape");
        std::os::unix::fs::symlink("/", &link).unwrap();
        assert_eq!(
            resolve_in(&home, link.to_str().unwrap()).map(|(real, _)| real),
            None
        );
    }
}
