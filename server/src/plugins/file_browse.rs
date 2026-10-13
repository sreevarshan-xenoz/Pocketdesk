use linux_link_core::error::Result;
use linux_link_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
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
    let real_home = home.canonicalize().ok()?;
    if requested.is_empty() || requested == "/" {
        return Some((real_home, home.to_path_buf()));
    }
    if !lexically_inside(home, requested) {
        return None;
    }
    let real = Path::new(requested).canonicalize().ok()?;
    let rest = real.strip_prefix(&real_home).ok()?;
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
        Ok(files) => json!({
            "path": shown.display().to_string(),
            "parent": parent,
            "files": files,
        }),
        Err(e) => json!({
            "error": e.to_string(),
            "path": shown.display().to_string(),
            "parent": parent,
            "files": [],
        }),
    }
}

/// Entries the system keeps out of sight: a name starting with a dot, and
/// on Windows anything marked hidden. A home folder is full of them
/// (`.cache`, `AppData`, `desktop.ini`), and on a phone they bury the
/// folders a person means to open.
fn is_hidden(name: &str, metadata: &std::fs::Metadata) -> bool {
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
            let metadata = entry.metadata().ok()?;
            let name = entry.file_name().to_string_lossy().to_string();
            if is_hidden(&name, &metadata) {
                return None;
            }
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
