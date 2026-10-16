//! The phones' battery as they last reported it (roadmap T8b).
//!
//! A phone in a session sends `kdeconnect.battery {currentCharge,
//! isCharging, thresholdEvent}`; the server keeps the newest report per
//! phone and writes them to `<state>/phone_status.json`, which the desktop
//! app and the local API read. Only the server writes the file.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::paths::PHONE_STATUS_FILE;

/// A phone's battery report, as it came on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatteryReport {
    /// 0–100.
    pub charge: u8,
    pub charging: bool,
    /// The phone itself says its battery is low (`thresholdEvent` 1).
    pub low: bool,
}

impl BatteryReport {
    /// The report in a `kdeconnect.battery` body, or `None` for one with a
    /// charge outside 0–100 or no charging flag.
    pub fn from_body(body: &serde_json::Value) -> Option<Self> {
        let charge = body.get("currentCharge")?.as_u64()?;
        let charging = body.get("isCharging")?.as_bool()?;
        if charge > 100 {
            return None;
        }
        let low = body.get("thresholdEvent").and_then(|v| v.as_u64()) == Some(1);
        Some(Self {
            charge: charge as u8,
            charging,
            low,
        })
    }
}

/// A phone's last battery report and when it came.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneBattery {
    pub charge: u8,
    pub charging: bool,
    pub updated_unix: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct StatusFile {
    batteries: BTreeMap<String, PhoneBattery>,
}

/// Each phone's last battery report, by device id; empty when there is no
/// file, `None` when there is one that does not read (a reader that shows
/// them keeps what it had rather than showing none).
pub fn try_read(state_dir: &Path) -> Option<BTreeMap<String, PhoneBattery>> {
    match std::fs::read_to_string(state_dir.join(PHONE_STATUS_FILE)) {
        Ok(raw) => serde_json::from_str::<StatusFile>(&raw)
            .ok()
            .map(|file| file.batteries),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Some(BTreeMap::new()),
        Err(_) => None,
    }
}

/// Each phone's last battery report, by device id. A missing or unreadable
/// file is empty.
pub fn read(state_dir: &Path) -> BTreeMap<String, PhoneBattery> {
    std::fs::read_to_string(state_dir.join(PHONE_STATUS_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<StatusFile>(&raw).ok())
        .map(|file| file.batteries)
        .unwrap_or_default()
}

/// Replace the file with `batteries`, whole, so a reader never sees it
/// half-written.
pub fn write(state_dir: &Path, batteries: &BTreeMap<String, PhoneBattery>) -> io::Result<()> {
    let json = serde_json::to_string(&StatusFile {
        batteries: batteries.clone(),
    })
    .map_err(io::Error::other)?;
    let path = state_dir.join(PHONE_STATUS_FILE);
    let tmp = path.with_extension("json.tmp");
    std::fs::create_dir_all(state_dir)?;
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_report_is_read_from_its_body_and_a_bad_one_is_not() {
        assert_eq!(
            BatteryReport::from_body(&json!({"currentCharge": 54, "isCharging": true})),
            Some(BatteryReport {
                charge: 54,
                charging: true,
                low: false
            })
        );
        assert_eq!(
            BatteryReport::from_body(
                &json!({"currentCharge": 9, "isCharging": false, "thresholdEvent": 1})
            )
            .map(|r| r.low),
            Some(true)
        );
        for bad in [
            json!({"currentCharge": 101, "isCharging": false}),
            json!({"currentCharge": -1, "isCharging": false}),
            json!({"currentCharge": "50", "isCharging": false}),
            json!({"currentCharge": 50}),
            json!({"isCharging": true}),
            json!({}),
        ] {
            assert_eq!(BatteryReport::from_body(&bad), None, "{bad}");
        }
    }

    #[test]
    fn the_reports_are_written_and_read_back_and_a_missing_file_is_empty() {
        let dir = std::env::temp_dir().join(format!(
            "pocketdesk-phone-status-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(read(&dir).is_empty());
        let mut batteries = BTreeMap::new();
        batteries.insert(
            "phone-a".to_string(),
            PhoneBattery {
                charge: 54,
                charging: true,
                updated_unix: 100,
            },
        );
        write(&dir, &batteries).unwrap();
        assert_eq!(read(&dir), batteries);
        std::fs::write(dir.join(PHONE_STATUS_FILE), "not json").unwrap();
        assert!(read(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
