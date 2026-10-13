use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};

#[derive(Debug)]
pub struct BatteryPlugin;

impl Default for BatteryPlugin {
    fn default() -> Self {
        Self
    }
}

impl BatteryPlugin {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl Plugin for BatteryPlugin {
    fn name(&self) -> &'static str {
        "battery"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.battery.request"]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.battery"]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type.as_str() == "kdeconnect.battery.request" {
            let (charge, is_charging) = match read_battery().await {
                Some(state) => state,
                None => {
                    // No battery found — send explicit "no battery" response
                    let response =
                        NetworkPacket::new("kdeconnect.battery").with_body(serde_json::json!({
                            "currentCharge": 0,
                            "isCharging": false,
                            "noBattery": true,
                        }));
                    sender.send_packet(&response).await?;
                    return Ok(());
                }
            };
            let response = NetworkPacket::new("kdeconnect.battery").with_body(serde_json::json!({
                "currentCharge": charge,
                "isCharging": is_charging,
            }));
            sender.send_packet(&response).await?;
        }
        Ok(())
    }
}

/// `(charge %, charging)`, or `None` on a machine without a battery.
#[cfg(not(windows))]
async fn read_battery() -> Option<(u8, bool)> {
    match (read_battery_charge().await, read_is_charging().await) {
        (Some(charge), Some(charging)) => Some((charge, charging)),
        _ => None,
    }
}

/// `(charge %, charging)`, or `None` on a machine without a battery.
#[cfg(windows)]
async fn read_battery() -> Option<(u8, bool)> {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut status = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut status) }.ok()?;
    battery_from_power_status(
        status.ACLineStatus,
        status.BatteryFlag,
        status.BatteryLifePercent,
    )
}

/// Interpret `SYSTEM_POWER_STATUS`: flag 128 means no battery, 255 (flag or
/// percent) means unknown; charging is the flag's bit 8, or mains power on a
/// battery that is not yet full (some firmware never sets the bit).
#[cfg(windows)]
fn battery_from_power_status(ac_line: u8, flag: u8, percent: u8) -> Option<(u8, bool)> {
    const NO_BATTERY: u8 = 128;
    const UNKNOWN: u8 = 255;
    const CHARGING: u8 = 8;
    if flag == UNKNOWN || flag & NO_BATTERY != 0 || percent == UNKNOWN {
        return None;
    }
    let charging = flag & CHARGING != 0 || (ac_line == 1 && percent < 100);
    Some((percent.min(100), charging))
}

#[cfg(not(windows))]
/// Read battery charge percentage.
///
/// Tries UPower D-Bus first, then falls back to sysfs.
/// Returns None if no battery is found (desktop system).
async fn read_battery_charge() -> Option<u8> {
    // Try UPower first
    if let Some(charge) = read_upower_percentage().await {
        return Some(charge);
    }

    // Fall back to sysfs
    read_sysfs_capacity()
}

#[cfg(not(windows))]
/// Try to read battery percentage from UPower via gdbus.
async fn read_upower_percentage() -> Option<u8> {
    let output = tokio::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.freedesktop.UPower",
            "--object-path",
            "/org/freedesktop/UPower/devices/DisplayDevice",
            "--method",
            "org.freedesktop.DBus.Properties.Get",
            "org.freedesktop.UPower.Device",
            "Percentage",
        ])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Parse "(variant <double> 85.0)" → 85
    stdout
        .split_whitespace()
        .last()
        .and_then(|s| s.parse::<f64>().ok())
        .map(|v| v as u8)
}

#[cfg(not(windows))]
/// Read battery capacity from sysfs.
fn read_sysfs_capacity() -> Option<u8> {
    // Try BAT0, BAT1, BAT2
    for bat in &["BAT0", "BAT1", "BAT2"] {
        let path = format!("/sys/class/power_supply/{}/capacity", bat);
        if let Ok(content) = std::fs::read_to_string(&path)
            && let Ok(capacity) = content.trim().parse::<u8>()
        {
            return Some(capacity);
        }
    }
    None
}

#[cfg(not(windows))]
/// Check if battery is charging.
///
/// Tries UPower first, then sysfs.
/// Returns None if no battery is found.
async fn read_is_charging() -> Option<bool> {
    // Try UPower first
    if let Some(charging) = read_upower_charging().await {
        return Some(charging);
    }

    // Fall back to sysfs
    read_sysfs_status()
}

#[cfg(not(windows))]
/// Try to read charging status from UPower via gdbus.
async fn read_upower_charging() -> Option<bool> {
    let output = tokio::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.freedesktop.UPower",
            "--object-path",
            "/org/freedesktop/UPower/devices/DisplayDevice",
            "--method",
            "org.freedesktop.DBus.Properties.Get",
            "org.freedesktop.UPower.Device",
            "State",
        ])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // UPower State: 1=Charging, 2=Discharging, 3=Not charging
    stdout
        .contains("variant <uint32> 1")
        .then_some(true)
        .or_else(|| stdout.contains("variant <uint32>").then_some(false))
}

#[cfg(not(windows))]
/// Read charging status from sysfs.
fn read_sysfs_status() -> Option<bool> {
    for bat in &["BAT0", "BAT1", "BAT2"] {
        let path = format!("/sys/class/power_supply/{}/status", bat);
        if let Ok(content) = std::fs::read_to_string(&path) {
            let status = content.trim();
            return Some(status == "Charging");
        }
    }
    None
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn power_status_maps_to_charge_and_charging() {
        assert_eq!(battery_from_power_status(0, 1, 87), Some((87, false)));
        assert_eq!(battery_from_power_status(1, 8, 40), Some((40, true)));
        // On mains, not full, firmware without the charging bit.
        assert_eq!(battery_from_power_status(1, 1, 99), Some((99, true)));
        assert_eq!(battery_from_power_status(1, 1, 100), Some((100, false)));
    }

    #[test]
    fn desktops_and_unknown_states_report_no_battery() {
        assert_eq!(battery_from_power_status(1, 128, 255), None);
        assert_eq!(battery_from_power_status(255, 255, 255), None);
        assert_eq!(battery_from_power_status(1, 2, 255), None);
    }

    /// Live, read-only: this laptop reports a battery.
    #[tokio::test]
    #[ignore = "reads this machine's battery"]
    async fn this_machine_reports_its_battery() {
        let (charge, _charging) = read_battery().await.expect("a battery");
        assert!(charge <= 100);
    }
}
