//! Home Assistant over MQTT (ecosystem phase 1e): the broker's address as
//! config.toml gives it, and the bridge's status, which the server writes
//! to `<state>/home_assistant.json` and the desktop app shows.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::paths::HOME_ASSISTANT_FILE;

/// The device a task start from Home Assistant is held, logged and
/// rate-limited as, in place of a phone's id.
/// No phone may take it (`valid_device_id` refuses it).
pub const HOME_ASSISTANT_DEVICE: &str = crate::protocol::kdeconnect::HOME_ASSISTANT_ID;

/// MQTT's own port.
pub const DEFAULT_PORT: u16 = 1883;

/// An MQTT broker to connect to: plain TCP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Broker {
    pub host: String,
    pub port: u16,
}

/// The broker `text` names: `mqtt://host[:port]` or `host[:port]`, port
/// 1883 when none is given; an IPv6 host in brackets. Why not, otherwise.
pub fn parse_broker(text: &str) -> Result<Broker, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("no broker address".to_string());
    }
    let lower = text.to_ascii_lowercase();
    let rest = if lower.starts_with("mqtts://") || lower.starts_with("ssl://") {
        return Err("TLS to the broker (mqtts://) is not supported yet".to_string());
    } else if let Some(scheme_end) = lower.find("://") {
        if &lower[..scheme_end] != "mqtt" && &lower[..scheme_end] != "tcp" {
            return Err(format!(
                "{text:?} is not an mqtt:// address (host, or mqtt://host:port)"
            ));
        }
        &text[scheme_end + 3..]
    } else {
        text
    };
    let rest = rest.trim_end_matches('/');
    if rest.contains(['/', '@', '?', '#']) || rest.chars().any(char::is_whitespace) {
        return Err(format!(
            "{text:?} is not a broker address (host, or mqtt://host:port)"
        ));
    }
    let (host, port) = if let Some(inner) = rest.strip_prefix('[') {
        let (host, after) = inner
            .split_once(']')
            .ok_or_else(|| format!("{text:?} has an unclosed ["))?;
        match after {
            "" => (host, None),
            port => (
                host,
                Some(
                    port.strip_prefix(':')
                        .ok_or_else(|| format!("{text:?} is not a broker address"))?,
                ),
            ),
        }
    } else {
        match rest.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (rest, None),
        }
    };
    if host.is_empty() {
        return Err(format!("{text:?} names no host"));
    }
    let port = match port {
        None => DEFAULT_PORT,
        Some(port) => port
            .parse::<u16>()
            .ok()
            .filter(|p| *p != 0)
            .ok_or_else(|| format!("{port:?} is not a port"))?,
    };
    Ok(Broker {
        host: host.to_string(),
        port,
    })
}

/// How the bridge to the broker stands, as the server last wrote it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub connected: bool,
    /// Why it is not connected, when it tried and failed.
    #[serde(default)]
    pub error: Option<String>,
    pub updated_unix: u64,
}

/// The status file, or `None` when there is none (the bridge is off) or it
/// does not read.
pub fn read_status(state_dir: &Path) -> Option<Status> {
    let raw = std::fs::read_to_string(state_dir.join(HOME_ASSISTANT_FILE)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Replace the status file, whole.
pub fn write_status(state_dir: &Path, status: &Status) -> io::Result<()> {
    let json = serde_json::to_string(status).map_err(io::Error::other)?;
    let path = state_dir.join(HOME_ASSISTANT_FILE);
    let tmp = path.with_extension("json.tmp");
    std::fs::create_dir_all(state_dir)?;
    std::fs::write(&tmp, json)?;
    crate::atomic_file::replace(&tmp, &path)
}

/// Remove the status file: the bridge is off.
pub fn clear_status(state_dir: &Path) {
    let _ = std::fs::remove_file(state_dir.join(HOME_ASSISTANT_FILE));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_broker_is_a_host_and_a_port() {
        let broker = |host: &str, port| Broker {
            host: host.into(),
            port,
        };
        assert_eq!(
            parse_broker("mqtt://192.0.2.5:1884"),
            Ok(broker("192.0.2.5", 1884))
        );
        assert_eq!(
            parse_broker("mqtt://broker.example.org"),
            Ok(broker("broker.example.org", 1883))
        );
        assert_eq!(
            parse_broker(" homeassistant.local "),
            Ok(broker("homeassistant.local", 1883))
        );
        assert_eq!(
            parse_broker("192.0.2.5:1999"),
            Ok(broker("192.0.2.5", 1999))
        );
        assert_eq!(
            parse_broker("MQTT://Broker.example.org/"),
            Ok(broker("Broker.example.org", 1883))
        );
        assert_eq!(
            parse_broker("tcp://192.0.2.5"),
            Ok(broker("192.0.2.5", 1883))
        );
        assert_eq!(
            parse_broker("mqtt://[2001:db8::5]:1884"),
            Ok(broker("2001:db8::5", 1884))
        );
        assert_eq!(
            parse_broker("[2001:db8::5]"),
            Ok(broker("2001:db8::5", 1883))
        );
        for bad in [
            "",
            "mqtts://broker.example.org",
            "http://broker.example.org",
            "mqtt://user:pw@broker.example.org",
            "mqtt://:1883",
            "broker.example.org:0",
            "broker.example.org:http",
            "mqtt://broker.example.org/topic",
            "[2001:db8::5",
            "broker example.org",
        ] {
            assert!(parse_broker(bad).is_err(), "{bad}");
        }
        assert!(parse_broker("mqtts://x").unwrap_err().contains("TLS"));
    }

    #[test]
    fn the_status_reads_back_and_goes_when_cleared() {
        let dir = std::env::temp_dir().join(format!("pd-ha-status-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(read_status(&dir), None);
        let status = Status {
            connected: false,
            error: Some("connection refused".into()),
            updated_unix: 7,
        };
        write_status(&dir, &status).unwrap();
        assert_eq!(read_status(&dir), Some(status));
        clear_status(&dir);
        assert_eq!(read_status(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
