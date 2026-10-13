use linux_link_core::error::Result;
use linux_link_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

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

    /// Sanitize and validate path to prevent directory traversal.
    fn sanitize_path(requested: &str) -> Option<PathBuf> {
        Self::sanitize_path_in(&dirs::home_dir()?, requested)
    }

    /// The real directory `requested` names, if it lies inside `home`.
    /// Both are resolved first, so a symlink (or a Windows junction) inside
    /// home that points elsewhere cannot lead out of it.
    fn sanitize_path_in(home: &Path, requested: &str) -> Option<PathBuf> {
        if requested.is_empty() || requested == "/" {
            return home.canonicalize().ok();
        }
        let path = Path::new(requested);
        // Checked as text before the filesystem sees it: opening a path is
        // already an action. On Windows a UNC path (\\host\share) makes the
        // system connect out and offer the user's NTLM credentials, and a
        // device path (\\.\pipe\x) opens a device, before any check that
        // follows could refuse it. So it must be under home as written, and
        // contain no "..".
        if !path.starts_with(home)
            || path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return None;
        }
        let real = path.canonicalize().ok()?;
        real.starts_with(home.canonicalize().ok()?).then_some(real)
    }

    /// List directory contents as a JSON array of file entries.
    fn list_directory(path: &Path) -> Result<serde_json::Value> {
        let entries: Vec<serde_json::Value> = std::fs::read_dir(path)
            .map_err(|e| linux_link_core::error::LinuxLinkError::Io {
                operation: "read_dir",
                detail: e.to_string(),
            })?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let metadata = entry.metadata().ok()?;
                let name = entry.file_name().to_string_lossy().to_string();
                let is_dir = metadata.is_dir();
                let size = if is_dir { 0 } else { metadata.len() };
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);

                Some(json!({
                    "name": name,
                    "isDirectory": is_dir,
                    "size": size,
                    "modified": modified,
                }))
            })
            .collect();

        Ok(json!(entries))
    }
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
            let requested_path = packet
                .body
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let path = match Self::sanitize_path(requested_path) {
                Some(p) => p,
                None => {
                    let response =
                        NetworkPacket::new("kdeconnect.filebrowse.response").with_body(json!({
                            "error": "Invalid or unauthorized path",
                            "path": requested_path,
                            "files": [],
                        }));
                    sender.send_packet(&response).await?;
                    return Ok(());
                }
            };

            let files = match Self::list_directory(&path) {
                Ok(f) => f,
                Err(e) => {
                    let response =
                        NetworkPacket::new("kdeconnect.filebrowse.response").with_body(json!({
                            "error": e.to_string(),
                            "path": requested_path,
                            "files": [],
                        }));
                    sender.send_packet(&response).await?;
                    return Ok(());
                }
            };

            let response = NetworkPacket::new("kdeconnect.filebrowse.response").with_body(json!({
                "path": requested_path,
                "files": files,
            }));
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
            FileBrowsePlugin::sanitize_path_in(&home, docs.to_str().unwrap()),
            Some(real_docs)
        );
        assert_eq!(
            FileBrowsePlugin::sanitize_path_in(&home, ""),
            Some(home.canonicalize().unwrap())
        );
        let up = docs.join("..").join("..");
        assert_eq!(
            FileBrowsePlugin::sanitize_path_in(&home, up.to_str().unwrap()),
            None
        );
        let outside = std::env::temp_dir();
        assert_eq!(
            FileBrowsePlugin::sanitize_path_in(&home, outside.to_str().unwrap()),
            None
        );
        assert_eq!(
            FileBrowsePlugin::sanitize_path_in(&home, home.join("missing").to_str().unwrap()),
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
                FileBrowsePlugin::sanitize_path_in(&home, path),
                None,
                "{path}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_inside_home_cannot_lead_out_of_it() {
        let home = temp_home("link");
        let link = home.join("escape");
        std::os::unix::fs::symlink("/", &link).unwrap();
        assert_eq!(
            FileBrowsePlugin::sanitize_path_in(&home, link.to_str().unwrap()),
            None
        );
    }
}
