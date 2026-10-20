//! The phones' battery as they last reported it (roadmap T8b).
//!
//! A phone in a session sends `kdeconnect.battery {currentCharge,
//! isCharging, thresholdEvent}`; the server keeps the newest report per
//! phone and writes them to `<state>/phone_status.json`, which the desktop
//! app and the local API read. Only the server writes the file.
//!
//! L9: the same report carries the phone's state beside its battery (its
//! free storage, its network's kind and bars, its ringer), each optional,
//! so a KDE device's report or an older phone's reads as before.

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
    /// What else it said of itself (L9).
    pub state: PhoneState,
}

/// The kind of network a phone is on (L9): never its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Network {
    Wifi,
    Mobile,
    Ethernet,
    None,
}

/// A phone's ringer (L9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ringer {
    Normal,
    Vibrate,
    Silent,
}

/// What a phone says of itself beside its battery (L9), each part left out
/// when it did not say or said something that does not read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_free_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_total_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<Network>,
    /// 0–4 bars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ringer: Option<Ringer>,
}

impl PhoneState {
    /// The state in a `kdeconnect.battery` body. Free storage above the
    /// total, bars above 4 and unknown words are left out.
    pub fn from_body(body: &serde_json::Value) -> Self {
        let word = |key: &str| body.get(key).and_then(|v| v.as_str());
        let storage_total_mb = body.get("storageTotalMb").and_then(|v| v.as_u64());
        let storage_free_mb = body
            .get("storageFreeMb")
            .and_then(|v| v.as_u64())
            .filter(|free| storage_total_mb.is_none_or(|total| *free <= total));
        Self {
            storage_free_mb,
            storage_total_mb,
            network: match word("network") {
                Some("wifi") => Some(Network::Wifi),
                Some("mobile") => Some(Network::Mobile),
                Some("ethernet") => Some(Network::Ethernet),
                Some("none") => Some(Network::None),
                _ => None,
            },
            signal: body
                .get("signal")
                .and_then(|v| v.as_u64())
                .filter(|bars| *bars <= 4)
                .map(|bars| bars as u8),
            ringer: match word("ringer") {
                Some("normal") => Some(Ringer::Normal),
                Some("vibrate") => Some(Ringer::Vibrate),
                Some("silent") => Some(Ringer::Silent),
                _ => None,
            },
        }
    }

    /// Whether the phone said nothing of itself.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
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
            state: PhoneState::from_body(body),
        })
    }
}

/// A phone's last battery report and when it came.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneBattery {
    pub charge: u8,
    pub charging: bool,
    pub updated_unix: u64,
    /// What else it said of itself then (L9), its fields beside these.
    #[serde(flatten)]
    pub state: PhoneState,
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
                low: false,
                state: PhoneState::default(),
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
    fn the_phones_state_is_read_beside_its_battery_and_what_does_not_read_is_left_out() {
        let report = BatteryReport::from_body(&json!({
            "currentCharge": 61, "isCharging": false,
            "storageFreeMb": 12300, "storageTotalMb": 128000,
            "network": "mobile", "signal": 2, "ringer": "vibrate",
        }))
        .unwrap();
        assert_eq!(
            report.state,
            PhoneState {
                storage_free_mb: Some(12_300),
                storage_total_mb: Some(128_000),
                network: Some(Network::Mobile),
                signal: Some(2),
                ringer: Some(Ringer::Vibrate),
            }
        );
        // A KDE device's report, or an older phone's: no state.
        assert!(
            BatteryReport::from_body(&json!({"currentCharge": 61, "isCharging": false}))
                .unwrap()
                .state
                .is_empty()
        );
        let odd = PhoneState::from_body(&json!({
            "storageFreeMb": 900, "storageTotalMb": 100,
            "network": "SSID-name", "signal": 5, "ringer": "loud",
        }));
        assert_eq!(
            odd,
            PhoneState {
                storage_total_mb: Some(100),
                ..Default::default()
            }
        );
        assert_eq!(
            PhoneState::from_body(&json!({"signal": -1, "storageFreeMb": "lots"})),
            PhoneState::default()
        );
        // Free storage alone is kept: its total may not be known.
        assert_eq!(
            PhoneState::from_body(&json!({"storageFreeMb": 500})).storage_free_mb,
            Some(500)
        );
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
                ..Default::default()
            },
        );
        batteries.insert(
            "phone-b".to_string(),
            PhoneBattery {
                charge: 80,
                charging: false,
                updated_unix: 200,
                state: PhoneState {
                    storage_free_mb: Some(12_300),
                    storage_total_mb: Some(128_000),
                    network: Some(Network::Wifi),
                    signal: Some(3),
                    ringer: Some(Ringer::Silent),
                },
            },
        );
        write(&dir, &batteries).unwrap();
        assert_eq!(read(&dir), batteries);
        // A file from before L9 reads, its phones with no state.
        std::fs::write(
            dir.join(PHONE_STATUS_FILE),
            r#"{"batteries":{"old":{"charge":5,"charging":true,"updated_unix":7}}}"#,
        )
        .unwrap();
        let old = read(&dir);
        assert_eq!(old["old"].charge, 5);
        assert!(old["old"].state.is_empty());
        std::fs::write(dir.join(PHONE_STATUS_FILE), "not json").unwrap();
        assert!(read(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
