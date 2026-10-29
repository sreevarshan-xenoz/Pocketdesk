//! Hyprland IPC client: request/reply (socket1) and the event stream (socket2).
//!
//! Talks to the live sockets under
//! `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/` instead of shelling
//! out to `hyprctl`, so the server has no binary dependency. The socket file
//! name moved across Hyprland releases (`.socket1.sock` → `.socket.sock`);
//! both are probed.
//!
//! Used by the window picker (R3#7): enumerate `j/clients`, resolve the
//! active window geometry for capture cropping, and follow window
//! activations/moves via socket2 events.
//!
//! Only the read side is implemented: the *write* grammar of socket1
//! (`dispatch`, `keyword`, `setoption`) has been broken upstream since the
//! Lua IPC rewrite in 0.56 — plain `hyprctl dispatch workspace 3` errors on a
//! stock 0.56.2 compositor (hyprwm/Hyprland discussion #16224) — so no action
//! dispatchers live here. Workspace switching already works through injected
//! keybinds (ShortcutBar), and window focus can ride the input-injection path
//! if the picker needs it.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::debug;

/// The stream type Hyprland's sockets give us. Off Unix there are no Hyprland
/// sockets; the alias only lets the IPC code compile, and [`connect_socket`]
/// refuses before one could be built.
#[cfg(unix)]
type IpcStream = tokio::net::UnixStream;
#[cfg(not(unix))]
type IpcStream = tokio::io::DuplexStream;

async fn connect_socket(path: &Path) -> Result<IpcStream> {
    #[cfg(unix)]
    {
        tokio::net::UnixStream::connect(path)
            .await
            .with_context(|| format!("connect {}", path.display()))
    }
    #[cfg(not(unix))]
    {
        anyhow::bail!(
            "Hyprland IPC needs Unix sockets, which this OS does not have ({})",
            path.display()
        )
    }
}

/// Client-side handle to a running Hyprland instance.
#[derive(Debug, Clone)]
pub struct HyprlandIpc {
    socket_dir: PathBuf,
}

/// A single window as reported by `j/clients` / `j/activewindow`.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct HyprWindow {
    pub address: String,
    #[serde(default)]
    pub title: String,
    #[serde(default, rename = "class")]
    pub class: String,
    #[serde(default)]
    pub pid: u32,
    #[serde(default)]
    pub mapped: bool,
    #[serde(default)]
    pub hidden: bool,
    /// Top-left corner in global logical coordinates.
    #[serde(default)]
    pub at: [i32; 2],
    /// Window size including decorations.
    #[serde(default)]
    pub size: [i32; 2],
    #[serde(default)]
    pub monitor: i32,
    /// Its full-screen mode (j/clients, j/activewindow): 0 none; since
    /// Hyprland 0.42, 1 maximized, 2 full screen, 3 both (before it, 1 was
    /// full screen and 2 maximized).
    #[serde(default)]
    pub fullscreen: u8,
    #[serde(default)]
    pub workspace: HyprWorkspaceRef,
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct HyprWorkspaceRef {
    #[serde(default)]
    pub id: i32,
    #[serde(default)]
    pub name: String,
}

/// A monitor as reported by `j/monitors`. Array position is the ID windows
/// reference via `HyprWindow::monitor`.
#[derive(Debug, Clone, Deserialize)]
pub struct HyprMonitor {
    #[serde(default)]
    pub name: String,
    /// Global layout origin of this monitor.
    #[serde(default)]
    pub x: i32,
    #[serde(default)]
    pub y: i32,
    /// Its mode, in the pixels it has (not the logical ones it shows).
    #[serde(default)]
    pub width: i32,
    #[serde(default)]
    pub height: i32,
    /// How much it is scaled: its logical size is its mode divided by this.
    #[serde(default = "unit_scale")]
    pub scale: f64,
    /// Its rotation (wl_output's transform): an odd one turns it on its
    /// side, swapping its width and height.
    #[serde(default)]
    pub transform: i32,
}

fn unit_scale() -> f64 {
    1.0
}

/// A workspace as reported by `j/workspaces`.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct HyprWorkspace {
    pub id: i32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub active: bool,
}

/// One line from socket2: `eventname>>payload`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyprEvent {
    pub name: String,
    pub data: String,
}

impl HyprEvent {
    pub fn parse(line: &str) -> Option<Self> {
        let (name, data) = line.split_once(">>")?;
        Some(Self {
            name: name.to_string(),
            data: data.to_string(),
        })
    }
}

impl HyprlandIpc {
    /// Locate the IPC socket directory from the environment.
    pub fn from_env() -> Result<Self> {
        let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
            .context("HYPRLAND_INSTANCE_SIGNATURE not set (not running under Hyprland?)")?;
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
            .context("XDG_RUNTIME_DIR not set; cannot locate Hyprland IPC sockets")?;
        let socket_dir = PathBuf::from(runtime_dir).join("hypr").join(sig);
        anyhow::ensure!(
            socket_dir.is_dir(),
            "Hyprland IPC dir {} does not exist",
            socket_dir.display()
        );
        Ok(Self { socket_dir })
    }

    pub fn is_available() -> bool {
        Self::from_env().is_ok_and(|ipc| ipc.request_socket().is_some())
    }

    fn request_socket(&self) -> Option<PathBuf> {
        // Newer Hyprland renamed socket1; try both.
        for name in [".socket.sock", ".socket1.sock"] {
            let p = self.socket_dir.join(name);
            if p.exists() {
                return Some(p);
            }
        }
        None
    }

    fn event_socket_path(&self) -> PathBuf {
        self.socket_dir.join(".socket2.sock")
    }

    /// Send one `j/...` request and return the raw reply (JSON or plain text,
    /// exactly as Hyprland wrote it).
    pub async fn request(&self, req: &str) -> Result<String> {
        let path = self
            .request_socket()
            .context("no Hyprland request socket found")?;
        let mut stream = connect_socket(&path).await?;
        stream
            .write_all(req.as_bytes())
            .await
            .context("write Hyprland request")?;
        // Hyprland answers and closes; read to EOF.
        let mut buf = Vec::new();
        stream
            .read_to_end(&mut buf)
            .await
            .context("read Hyprland reply")?;
        String::from_utf8(buf).context("non-UTF8 Hyprland reply")
    }

    async fn json<T: serde::de::DeserializeOwned>(&self, req: &str) -> Result<T> {
        let reply = self.request(req).await?;
        serde_json::from_str(&reply)
            .with_context(|| format!("parse reply to {req}: {}", reply_trunc(&reply)))
    }

    /// All windows on the desktop (mapped and unmapped).
    pub async fn clients(&self) -> Result<Vec<HyprWindow>> {
        self.json("j/clients").await
    }

    /// The focused window, if any (Hyprland replies `{}` when there is none).
    pub async fn active_window(&self) -> Result<Option<HyprWindow>> {
        let reply = self.request("j/activewindow").await?;
        if reply.trim() == "{}" || reply.trim().is_empty() {
            return Ok(None);
        }
        let w: HyprWindow = serde_json::from_str(&reply)
            .with_context(|| format!("parse activewindow: {}", reply_trunc(&reply)))?;
        Ok(Some(w))
    }

    /// Visible windows only (mapped, not hidden) — the picker candidate set.
    pub async fn visible_windows(&self) -> Result<Vec<HyprWindow>> {
        Ok(self
            .clients()
            .await?
            .into_iter()
            .filter(|w| w.mapped && !w.hidden)
            .collect())
    }

    /// Monitors in layout order — array position is the ID referenced by
    /// `HyprWindow::monitor`.
    pub async fn monitors(&self) -> Result<Vec<HyprMonitor>> {
        self.json("j/monitors").await
    }

    /// All workspaces, in id order, with the `active` flag.
    pub async fn workspaces(&self) -> Result<Vec<HyprWorkspace>> {
        self.json("j/workspaces").await
    }

    /// Spawn a task streaming socket2 events (activewindow, openwindow,
    /// movewindow, …) into `rx`. Ends when the connection closes (Hyprland
    /// quit) — reconnection is a follow-up; consumers treat channel close as
    /// "no more events".
    pub async fn subscribe_events(&self) -> Result<mpsc::Receiver<HyprEvent>> {
        let path = self.event_socket_path();
        let stream = connect_socket(&path)
            .await
            .context("connect to the Hyprland event socket")?;
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            let mut stream = stream;
            let mut pending = String::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = match stream.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                pending.push_str(&String::from_utf8_lossy(&chunk[..n]));
                while let Some(pos) = pending.find('\n') {
                    let line = pending[..pos].trim_end();
                    if let Some(ev) = HyprEvent::parse(line)
                        && tx.send(ev).await.is_err()
                    {
                        return;
                    }
                    pending.drain(..=pos);
                }
            }
            debug!("Hyprland event socket closed");
        });
        Ok(rx)
    }
}

fn reply_trunc(reply: &str) -> String {
    let mut s: String = reply.chars().take(200).collect();
    if s.len() < reply.len() {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_event_line() {
        let ev = HyprEvent::parse("activewindow>>brave-browser,Some, Title").unwrap();
        assert_eq!(ev.name, "activewindow");
        assert_eq!(ev.data, "brave-browser,Some, Title");
        assert!(HyprEvent::parse("garbage without marker").is_none());
    }

    #[test]
    fn deserialize_window_shape() {
        let json = r#"{
            "address": "0x5cb3ceab0ab0", "mapped": true, "hidden": false,
            "at": [12, 36], "size": [1512, 816],
            "workspace": {"id": 2, "name": "2"},
            "monitor": 0, "class": "brave-browser",
            "title": "page - Brave", "pid": 12426, "fullscreen": 0
        }"#;
        let w: HyprWindow = serde_json::from_str(json).unwrap();
        assert_eq!(w.at, [12, 36]);
        assert_eq!(w.size, [1512, 816]);
        assert_eq!(w.workspace.name, "2");
        assert_eq!(w.class, "brave-browser");
    }

    /// Tolerate field drift across Hyprland versions: unknown fields ignored,
    /// missing optionals default.
    #[test]
    fn deserialize_minimal_window() {
        let w: HyprWindow = serde_json::from_str(r#"{"address":"0x1","futureField":42}"#).unwrap();
        assert_eq!(w.address, "0x1");
        assert!(!w.mapped);
        assert_eq!(w.size, [0, 0]);
    }

    #[test]
    fn deserialize_monitors_layout() {
        let json = r#"[
            {"name":"eDP-1","x":0,"y":0,"width":1920,"height":1200,"reserved":99},
            {"name":"HDMI-A-1","x":1920,"y":-100,"width":2560,"height":1440}
        ]"#;
        let monitors: Vec<HyprMonitor> = serde_json::from_str(json).unwrap();
        assert_eq!(monitors.len(), 2);
        assert_eq!(monitors[1].x, 1920);
        assert_eq!(monitors[1].y, -100);
        assert_eq!(monitors[1].width, 2560);
    }

    #[test]
    fn deserialize_workspaces() {
        let json = r#"[
            {"id":1,"name":"1","monitor":0,"active":true,"special":false,"windows":3,"extra":1},
            {"id":2,"name":"2:web","monitor":1,"active":false}
        ]"#;
        let ws: Vec<HyprWorkspace> = serde_json::from_str(json).unwrap();
        assert_eq!(ws.len(), 2);
        assert!(ws[0].active);
        assert_eq!(ws[1].name, "2:web");
        assert!(!ws[1].active);
    }

    // Live-IPC tests run only on a Hyprland session (skipped silently
    // elsewhere); they verify the module matches the real socket protocol.
    async fn live_ipc() -> Option<HyprlandIpc> {
        let ipc = HyprlandIpc::from_env().ok()?;
        ipc.request_socket()?;
        Some(ipc)
    }

    #[tokio::test]
    async fn live_active_window_roundtrip() {
        let Some(ipc) = live_ipc().await else {
            eprintln!("no Hyprland IPC here, skipping");
            return;
        };
        let w = ipc.active_window().await.unwrap();
        let w = w.expect("Hyprland reports an active window on this session");
        assert!(w.address.starts_with("0x"), "address shape: {}", w.address);
        assert!(w.size[0] > 0 && w.size[1] > 0, "size: {:?}", w.size);
        let clients = ipc.visible_windows().await.unwrap();
        assert!(
            clients.iter().any(|c| c.address == w.address),
            "active window must appear in j/clients"
        );
    }

    #[tokio::test]
    async fn live_event_socket_connects() {
        let Some(ipc) = live_ipc().await else {
            eprintln!("no Hyprland IPC here, skipping");
            return;
        };
        // subscribe_events pre-connects socket2 before returning, so an Ok
        // here proves the event path is live. Line parsing itself is covered
        // by parse_event_line; emitting a real compositor event needs a
        // mutation we deliberately do not perform from tests.
        let _rx = ipc.subscribe_events().await.unwrap();
    }
}
