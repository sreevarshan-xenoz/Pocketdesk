//! The control port the running server actually listens on (T1).
//!
//! Another program can hold the configured port: KDE Connect's `kdeconnectd`
//! and GSConnect both take TCP 1716, Pocketdesk's default. The server then
//! listens on the next free port and writes `<state>/listening.json`, so the
//! desktop app's pairing code, Overview and Diagnostics name the port phones
//! can actually reach.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::paths::LISTENING_FILE;

/// `listening.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listening {
    /// The server process that wrote it.
    pub pid: u32,
    /// The port it bound.
    pub control_port: u16,
    /// The port its config asked for.
    pub configured_port: u16,
}

impl Listening {
    /// Whether the server had to move off the configured port.
    pub fn moved(&self) -> bool {
        self.control_port != self.configured_port
    }
}

/// How many ports past the configured one the server tries (1716 to 1725
/// by default; KDE Connect's file transfers use 1739 to 1764).
pub const TRIES: u16 = 10;

/// The ports to try, the configured one first.
pub fn ports_to_try(configured: u16) -> impl Iterator<Item = u16> {
    (0..TRIES).filter_map(move |n| configured.checked_add(n))
}

/// Record what the server bound (its side), owner-only as the state folder is.
pub fn write(state_dir: &Path, listening: &Listening) -> io::Result<()> {
    let json = serde_json::to_string(listening).map_err(io::Error::other)?;
    crate::secure_fs::write_private(&state_dir.join(LISTENING_FILE), json.as_bytes())
}

/// What `listening.json` holds, if it reads.
pub fn read(state_dir: &Path) -> Option<Listening> {
    serde_json::from_str(&std::fs::read_to_string(state_dir.join(LISTENING_FILE)).ok()?).ok()
}

/// The port phones reach now: the file's, while the server that wrote it is
/// the one running ([`port_for`]); the configured one otherwise.
pub fn control_port(state_dir: &Path, configured: u16) -> u16 {
    let running =
        super::server::server_pid(state_dir).filter(|_| super::server::is_running(state_dir));
    port_for(read(state_dir), running, configured)
}

/// [`control_port`]'s rule: a file left by another run (a pid that is not
/// the running server's), or one for another configured port, is not
/// believed.
pub fn port_for(file: Option<Listening>, running_pid: Option<u32>, configured: u16) -> u16 {
    match (file, running_pid) {
        (Some(f), Some(pid)) if f.pid == pid && f.configured_port == configured => f.control_port,
        _ => configured,
    }
}

/// Whether the running server moved off `configured` (the desktop's notice).
pub fn moved(state_dir: &Path, configured: u16) -> Option<Listening> {
    let running =
        super::server::server_pid(state_dir).filter(|_| super::server::is_running(state_dir))?;
    read(state_dir).filter(|f| f.pid == running && f.configured_port == configured && f.moved())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(pid: u32, port: u16, configured: u16) -> Listening {
        Listening {
            pid,
            control_port: port,
            configured_port: configured,
        }
    }

    #[test]
    fn the_running_servers_port_is_the_one_phones_reach() {
        assert_eq!(port_for(Some(file(7, 1717, 1716)), Some(7), 1716), 1717);
    }

    #[test]
    fn a_file_from_another_run_or_config_is_not_believed() {
        assert_eq!(
            port_for(Some(file(7, 1717, 1716)), Some(8), 1716),
            1716,
            "another process"
        );
        assert_eq!(
            port_for(Some(file(7, 1717, 1716)), None, 1716),
            1716,
            "no server"
        );
        assert_eq!(
            port_for(Some(file(7, 1717, 1716)), Some(7), 2000),
            2000,
            "config changed"
        );
        assert_eq!(port_for(None, Some(7), 1716), 1716);
    }

    #[test]
    fn ten_ports_are_tried_from_the_configured_one() {
        let ports: Vec<u16> = ports_to_try(1716).collect();
        assert_eq!(ports.first(), Some(&1716));
        assert_eq!(ports.len(), TRIES as usize);
        assert_eq!(ports.last(), Some(&1725));
        assert!(
            ports.iter().all(|p| !(1739..=1764).contains(p)),
            "KDE's transfer ports"
        );
        assert_eq!(
            ports_to_try(u16::MAX).collect::<Vec<_>>(),
            vec![u16::MAX],
            "no wrap"
        );
    }

    #[test]
    fn it_round_trips_through_the_state_folder() {
        let dir = std::env::temp_dir().join(format!("pd-listening-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = file(42, 1718, 1716);
        write(&dir, &f).unwrap();
        assert_eq!(read(&dir), Some(f));
        assert!(f.moved());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
