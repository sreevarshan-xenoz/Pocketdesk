//! The phones' battery (roadmap T8b): a phone in a session reports
//! `kdeconnect.battery {currentCharge, isCharging, thresholdEvent}`. The
//! server keeps the newest report per phone in memory, writes them to
//! `<state>/phone_status.json` at most every [`WRITE_EVERY`], and, with
//! `phone_battery_alerts` on, warns once when a phone runs low.
//!
//! The battery is status, not an action, so no permission gates it; only
//! a paired phone's report is taken.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use pocketdesk_core::error::Result;
use pocketdesk_core::local::phone_status::{BatteryReport, PhoneBattery};
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};

/// The packet a phone reports its battery with.
pub const BATTERY: &str = "kdeconnect.battery";

/// A phone not charging at or below this is low.
pub const LOW_AT: u8 = 15;
/// A phone warned about is warned again only after it was above this.
pub const REARM_ABOVE: u8 = 20;
/// The fewest minutes between two warnings for one phone.
pub const WARN_EVERY: Duration = Duration::from_secs(10 * 60);
/// How often the reports are written, at most.
pub const WRITE_EVERY: Duration = Duration::from_secs(5);
/// The most phones whose reports are kept.
const MAX_PHONES: usize = 64;

/// `phone_battery_alerts` from the config, set at start.
static ALERTS: AtomicBool = AtomicBool::new(true);

/// Warn when a phone runs low (true) or only record its battery.
pub fn configure(alerts: bool) {
    ALERTS.store(alerts, Ordering::Relaxed);
}

/// When to warn that a phone's battery is low: once as it falls to
/// [`LOW_AT`] (or the phone says it is low) while not charging, then not
/// again until it has been above [`REARM_ABOVE`], and never twice within
/// [`WARN_EVERY`].
#[derive(Default)]
pub struct LowBatteryWatch {
    phones: HashMap<String, Watch>,
}

struct Watch {
    armed: bool,
    last: Option<Instant>,
}

impl LowBatteryWatch {
    /// Whether `report` from `device` at `now` calls for a warning.
    pub fn report(&mut self, device: &str, report: &BatteryReport, now: Instant) -> bool {
        if !self.phones.contains_key(device) && self.phones.len() >= MAX_PHONES {
            return false;
        }
        let watch = self.phones.entry(device.to_string()).or_insert(Watch {
            armed: true,
            last: None,
        });
        let low = !report.charging && (report.charge <= LOW_AT || report.low);
        if !low {
            // Only the charge re-arms it: the flags are the phone's word.
            if report.charge > REARM_ABOVE {
                watch.armed = true;
            }
            return false;
        }
        let too_soon = watch
            .last
            .is_some_and(|last| now.saturating_duration_since(last) < WARN_EVERY);
        if !watch.armed || too_soon {
            return false;
        }
        watch.armed = false;
        watch.last = Some(now);
        true
    }
}

/// The reports kept in memory, and whether the file is behind them.
#[derive(Default)]
struct Reports {
    batteries: BTreeMap<String, PhoneBattery>,
    dirty: bool,
    watch: LowBatteryWatch,
}

impl Reports {
    /// Keep `report` from `device`; the warning to show, if any.
    fn take(
        &mut self,
        device: &str,
        report: &BatteryReport,
        alerts: bool,
        now_unix: u64,
        now: Instant,
    ) -> Option<String> {
        if self.batteries.contains_key(device) || self.batteries.len() < MAX_PHONES {
            self.batteries.insert(
                device.to_string(),
                PhoneBattery {
                    charge: report.charge,
                    charging: report.charging,
                    updated_unix: now_unix,
                },
            );
            self.dirty = true;
        }
        let warn = self.watch.report(device, report, now);
        (alerts && warn).then(|| {
            let short: String = device.chars().take(8).collect();
            format!("Phone {short}: battery low ({} %)", report.charge)
        })
    }
}

static REPORTS: LazyLock<Mutex<Reports>> = LazyLock::new(Default::default);

#[derive(Debug, Default)]
pub struct PhoneBatteryPlugin;

#[async_trait::async_trait]
impl Plugin for PhoneBatteryPlugin {
    fn name(&self) -> &'static str {
        "phone_battery"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[BATTERY]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type != BATTERY {
            return Ok(());
        }
        // Only a paired phone's: with `pairing_required = false` anyone
        // could otherwise fill the reports and raise warnings under
        // made-up ids.
        if !crate::plugins::pair::is_paired_device(sender.device_id()) {
            tracing::debug!(
                "A battery report from unpaired {} was ignored",
                sender.device_id()
            );
            return Ok(());
        }
        let Some(report) = BatteryReport::from_body(&packet.body) else {
            tracing::debug!("A battery report from {} was not read", sender.device_id());
            return Ok(());
        };
        let warning = REPORTS.lock().unwrap_or_else(|e| e.into_inner()).take(
            sender.device_id(),
            &report,
            ALERTS.load(Ordering::Relaxed),
            pocketdesk_core::local::pin::now_unix(),
            Instant::now(),
        );
        if let Some(warning) = warning {
            tracing::info!("{warning}");
            std::thread::spawn(move || {
                crate::platform::notify("Pocketdesk", &warning, Duration::from_secs(15));
            });
        }
        Ok(())
    }
}

/// Daemon task: keep `<state>/phone_status.json` up to date with the
/// reports, at most every [`WRITE_EVERY`]. Reports from an earlier run are
/// kept until their phone reports again.
pub async fn run_writer() {
    let Ok(dir) = crate::state::state_dir() else {
        tracing::warn!("No state directory: phones' batteries are not kept");
        return;
    };
    {
        let mut reports = REPORTS.lock().unwrap_or_else(|e| e.into_inner());
        for (device, battery) in pocketdesk_core::local::phone_status::read(&dir) {
            reports.batteries.entry(device).or_insert(battery);
        }
    }
    loop {
        tokio::time::sleep(WRITE_EVERY).await;
        let batteries = {
            let mut reports = REPORTS.lock().unwrap_or_else(|e| e.into_inner());
            if !reports.dirty {
                continue;
            }
            reports.dirty = false;
            reports.batteries.clone()
        };
        if let Err(e) = pocketdesk_core::local::phone_status::write(&dir, &batteries) {
            tracing::warn!("Could not keep the phones' batteries: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_is_kept_and_a_low_one_warns_only_with_the_setting_on() {
        let mut reports = Reports::default();
        let now = Instant::now();
        assert_eq!(
            reports.take("phone-a", &report(54, true), true, 100, now),
            None
        );
        assert_eq!(
            reports.batteries["phone-a"],
            PhoneBattery {
                charge: 54,
                charging: true,
                updated_unix: 100
            }
        );
        assert!(reports.dirty);
        let warning = reports.take("phone-a", &report(12, false), true, 160, now);
        assert_eq!(
            warning.as_deref(),
            Some("Phone phone-a: battery low (12 %)")
        );
        assert_eq!(reports.batteries["phone-a"].updated_unix, 160);
        assert_eq!(
            reports.take("phone-b", &report(12, false), false, 160, now),
            None
        );
        assert_eq!(
            reports.batteries["phone-b"].charge, 12,
            "kept with the setting off"
        );
    }

    struct Phone {
        id: &'static str,
        sent: Mutex<Vec<NetworkPacket>>,
    }

    #[async_trait::async_trait]
    impl DeviceSender for Phone {
        fn device_id(&self) -> &str {
            self.id
        }
        fn connection_id(&self) -> &str {
            self.id
        }
        async fn send_packet(&self, packet: &NetworkPacket) -> Result<()> {
            self.sent.lock().unwrap().push(packet.clone());
            Ok(())
        }
    }

    /// Only a paired phone's report is kept (or warned about): with
    /// `pairing_required = false` anyone could otherwise fill the reports
    /// and raise warnings under made-up ids. Nothing is answered.
    #[tokio::test]
    async fn only_a_paired_phones_report_is_kept_and_none_is_answered() {
        let dir = pocketdesk_core::local::paths::use_state_dir(
            std::env::temp_dir().join(format!("pd-server-tests-{}", std::process::id())),
        );
        std::fs::create_dir_all(dir).unwrap();
        pocketdesk_core::protocol::kdeconnect::TrustStore::update(
            dir.join("trusted_devices.json"),
            |store| store.trust_device("battery-paired"),
        )
        .unwrap();
        let report = NetworkPacket::new(BATTERY)
            .with_body(serde_json::json!({"currentCharge": 41, "isCharging": false}));
        for id in ["battery-paired", "battery-stranger"] {
            let phone = Phone {
                id,
                sent: Mutex::default(),
            };
            PhoneBatteryPlugin
                .handle_packet(&report, &phone)
                .await
                .unwrap();
            assert!(phone.sent.lock().unwrap().is_empty(), "{id} answered");
        }
        let reports = REPORTS.lock().unwrap();
        assert_eq!(reports.batteries["battery-paired"].charge, 41);
        assert!(!reports.batteries.contains_key("battery-stranger"));
    }

    #[test]
    fn at_most_so_many_phones_are_kept() {
        let mut reports = Reports::default();
        let now = Instant::now();
        for n in 0..MAX_PHONES + 5 {
            reports.take(&format!("phone-{n}"), &report(50, false), true, 1, now);
        }
        assert_eq!(reports.batteries.len(), MAX_PHONES);
        reports.take("phone-0", &report(49, false), true, 2, now);
        assert_eq!(
            reports.batteries["phone-0"].charge, 49,
            "a kept phone still updates"
        );
    }

    fn report(charge: u8, charging: bool) -> BatteryReport {
        BatteryReport {
            charge,
            charging,
            low: false,
        }
    }

    #[test]
    fn a_phone_falling_low_is_warned_about_once_until_it_recovers() {
        let mut watch = LowBatteryWatch::default();
        let start = Instant::now();
        assert!(!watch.report("a", &report(40, false), start));
        assert!(!watch.report("a", &report(16, false), start));
        assert!(
            watch.report("a", &report(15, false), start),
            "at the threshold"
        );
        assert!(!watch.report("a", &report(14, false), start), "once");
        // Charging does not re-arm it: a lying phone could flip the flag.
        assert!(!watch.report("a", &report(14, true), start));
        assert!(!watch.report("a", &report(13, false), start));
        // Back above 20 % it is armed again, but not within ten minutes.
        assert!(!watch.report("a", &report(21, true), start));
        assert!(!watch.report("a", &report(10, false), start + Duration::from_secs(60)));
        assert!(watch.report("a", &report(10, false), start + WARN_EVERY));
    }

    #[test]
    fn charging_or_the_phones_own_flag_decide_and_phones_are_apart() {
        let mut watch = LowBatteryWatch::default();
        let now = Instant::now();
        assert!(!watch.report("a", &report(5, true), now), "charging");
        let said_low = BatteryReport {
            charge: 30,
            charging: false,
            low: true,
        };
        assert!(
            watch.report("b", &said_low, now),
            "the phone says it is low"
        );
        assert!(
            watch.report("a", &report(5, false), now),
            "a is its own phone"
        );
    }
}
