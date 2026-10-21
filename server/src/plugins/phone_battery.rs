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
use pocketdesk_core::local::paths::TRUST_FILE;
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

/// When a phone has charged to a level (L3): once a charge, armed again
/// once it is unplugged or 5 % under the level.
#[derive(Default)]
struct ChargedWatch {
    /// Each phone's: whether the next time at the level tells.
    armed: std::collections::HashMap<String, bool>,
    /// The level the phones were watched for: a new one starts over.
    level: u8,
}

impl ChargedWatch {
    /// `device`'s `report`; whether it has just charged to `level` (0: off).
    fn report(&mut self, device: &str, report: &BatteryReport, level: u8) -> bool {
        // A new level is watched afresh (L3 review: turning it on, or down,
        // told at once of a phone already past it).
        if level != self.level {
            self.level = level;
            self.armed.clear();
        }
        if level == 0 {
            return false;
        }
        // A phone first seen past the level is no charge seen (L3 review:
        // each start told of a phone left on its charger).
        let first = !self.armed.contains_key(device);
        let armed = self.armed.entry(device.to_string()).or_insert(true);
        if first && report.charging && report.charge >= level.min(100) {
            *armed = false;
            return false;
        }
        let level = level.min(100);
        if !report.charging || report.charge.saturating_add(5) <= level {
            *armed = true;
            return false;
        }
        // At or above: a report can skip a level, as it comes on a change
        // or every five minutes.
        if report.charge >= level && *armed {
            *armed = false;
            return true;
        }
        false
    }
}

/// The reports kept in memory, and whether the file is behind them.
#[derive(Default)]
struct Reports {
    batteries: BTreeMap<String, PhoneBattery>,
    dirty: bool,
    watch: LowBatteryWatch,
    charged: ChargedWatch,
    /// Whether the last report taken fell low (warned or not, as the
    /// setting says), for the event log.
    last_low: bool,
}

impl Reports {
    /// Drop `device`'s report and its warning state.
    fn forget(&mut self, device: &str) {
        if self.batteries.remove(device).is_some() {
            self.dirty = true;
        }
        self.watch.phones.remove(device);
        self.charged.armed.remove(device);
    }

    /// Forget every phone not in `paired`: one removed or unpaired leaves
    /// no report behind, on disk or in the Devices tab.
    fn keep_only(&mut self, paired: &std::collections::HashSet<String>) {
        let gone: Vec<String> = self
            .batteries
            .keys()
            .chain(self.watch.phones.keys())
            .chain(self.charged.armed.keys())
            .filter(|id| !paired.contains(*id))
            .cloned()
            .collect();
        for id in gone {
            self.forget(&id);
        }
    }

    /// Keep `report` from `device`; the warning to show, if any: low, or
    /// charged to `charged_at` (L3; 0 off).
    fn take(
        &mut self,
        device: &str,
        report: &BatteryReport,
        alerts: bool,
        charged_at: u8,
        now_unix: u64,
        now: Instant,
    ) -> Option<String> {
        if !self.batteries.contains_key(device) && self.batteries.len() >= MAX_PHONES {
            // Full: the oldest report makes room, so a new phone is never
            // left out.
            let oldest = self
                .batteries
                .iter()
                .min_by_key(|(_, b)| b.updated_unix)
                .map(|(id, _)| id.clone());
            if let Some(oldest) = oldest {
                self.forget(&oldest);
            }
        }
        self.batteries.insert(
            device.to_string(),
            PhoneBattery {
                charge: report.charge,
                charging: report.charging,
                updated_unix: now_unix,
                state: report.state,
            },
        );
        self.dirty = true;
        let warn = self.watch.report(device, report, now);
        self.last_low = warn;
        let charged = self.charged.report(device, report, charged_at);
        let short: String = device.chars().take(8).collect();
        if alerts && warn {
            Some(format!("Phone {short}: battery low ({} %)", report.charge))
        } else if charged {
            Some(format!("Phone {short}: charged to {} %", report.charge))
        } else {
            None
        }
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
        take_from(sender.device_id(), &packet.body);
        Ok(())
    }
}

/// The charge last kept for `device`.
#[cfg(test)]
pub(crate) fn charge_of(device: &str) -> Option<u8> {
    REPORTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .batteries
        .get(device)
        .map(|b| b.charge)
}

/// Keep `device`'s battery report `body` (a paired phone's, or a paired KDE
/// Connect device's, 3c): kept, an event when it changed, a warning when
/// it runs low.
pub(crate) fn take_from(device: &str, body: &serde_json::Value) {
    let Some(report) = BatteryReport::from_body(body) else {
        tracing::debug!("A battery report from {device} was not read");
        return;
    };
    let (warning, changed, low) = {
        let mut reports = REPORTS.lock().unwrap_or_else(|e| e.into_inner());
        let before = reports
            .batteries
            .get(device)
            .map(|b| (b.charge, b.charging));
        let warning = reports.take(
            device,
            &report,
            ALERTS.load(Ordering::Relaxed),
            crate::events::current_config().charged_alert,
            pocketdesk_core::local::pin::now_unix(),
            Instant::now(),
        );
        let changed = before != Some((report.charge, report.charging));
        (warning, changed, reports.last_low)
    };
    if changed {
        crate::events::emit(
            "phone_battery",
            Some(device),
            serde_json::json!({ "charge": report.charge, "charging": report.charging }),
            serde_json::json!({}),
        );
    }
    if low {
        crate::events::emit(
            "phone_battery_low",
            Some(device),
            serde_json::json!({ "charge": report.charge }),
            serde_json::json!({}),
        );
    }
    // Paused (K2a): the Overview shows the battery; no warning pops up.
    if let Some(warning) = warning.filter(|_| !crate::paused::now()) {
        tracing::info!("{warning}");
        std::thread::spawn(move || {
            crate::platform::notify("Pocketdesk", &warning, Duration::from_secs(15));
        });
    }
}

/// Daemon task: keep `<state>/phone_status.json` up to date with the
/// reports, at most every [`WRITE_EVERY`]. Reports from an earlier run are
/// kept until their phone reports again; a phone no longer paired is
/// forgotten.
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
        // An unreadable store forgets no one.
        // Paired phones, and paired KDE Connect devices (3c); either store
        // unreadable forgets no one.
        let paired: Option<std::collections::HashSet<String>> =
            pocketdesk_core::protocol::kdeconnect::TrustStore::read(dir.join(TRUST_FILE))
                .ok()
                .zip(pocketdesk_core::local::kdeconnect_devices::try_devices(&dir).ok())
                .map(|(store, kde)| {
                    store
                        .trusted_devices()
                        .into_iter()
                        .chain(kde.into_iter().map(|d| d.device_id))
                        .collect()
                });
        let batteries = {
            let mut reports = REPORTS.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(paired) = &paired {
                reports.keep_only(paired);
                // L7: the switches' reports go with the batteries'.
                crate::plugins::phone_switches::prune(&dir, paired);
            }
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
    /// L3: charged to the level once a charge, at or above it, armed again
    /// unplugged or 5 % under; nothing with it off.
    #[test]
    fn charged_tells_once_a_charge() {
        let mut watch = ChargedWatch::default();
        let r = |charge: u8, charging: bool| BatteryReport {
            charge,
            charging,
            low: false,
            state: Default::default(),
        };
        assert!(!watch.report("p", &r(79, true), 80));
        assert!(watch.report("p", &r(82, true), 80), "a level skipped");
        assert!(!watch.report("p", &r(85, true), 80), "once");
        assert!(!watch.report("p", &r(78, true), 80), "still within 5 %");
        assert!(!watch.report("p", &r(81, true), 80));
        assert!(
            !watch.report("p", &r(81, false), 80),
            "unplugged: armed again"
        );
        assert!(watch.report("p", &r(81, true), 80));
        assert!(
            !watch.report("p", &r(70, true), 80),
            "5 % under: armed again"
        );
        assert!(watch.report("p", &r(80, true), 80));
        assert!(!watch.report("q", &r(100, true), 0), "off");
    }

    /// L3 review: a phone first seen past the level, or past a new level,
    /// tells nothing until a charge is seen.
    #[test]
    fn charged_waits_for_a_charge_it_sees() {
        let mut watch = ChargedWatch::default();
        let r = |charge: u8, charging: bool| BatteryReport {
            charge,
            charging,
            low: false,
            state: Default::default(),
        };
        assert!(
            !watch.report("p", &r(100, true), 100),
            "on its charger at start"
        );
        assert!(!watch.report("p", &r(100, true), 100));
        assert!(
            !watch.report("p", &r(95, true), 80),
            "the level turned down"
        );
        assert!(!watch.report("p", &r(100, false), 80), "unplugged");
        assert!(watch.report("p", &r(80, true), 80), "a charge seen");
    }

    use super::*;

    #[test]
    fn a_report_is_kept_and_a_low_one_warns_only_with_the_setting_on() {
        let mut reports = Reports::default();
        let now = Instant::now();
        assert_eq!(
            reports.take("phone-a", &report(54, true), true, 0, 100, now),
            None
        );
        assert_eq!(
            reports.batteries["phone-a"],
            PhoneBattery {
                charge: 54,
                charging: true,
                updated_unix: 100,
                ..Default::default()
            }
        );
        assert!(reports.dirty);
        // L9: what the phone says of itself is kept beside its battery.
        let mut with_state = report(54, true);
        with_state.state.ringer = Some(pocketdesk_core::local::phone_status::Ringer::Silent);
        reports.take("phone-a", &with_state, true, 0, 120, now);
        assert_eq!(
            reports.batteries["phone-a"].state.ringer,
            Some(pocketdesk_core::local::phone_status::Ringer::Silent)
        );
        let warning = reports.take("phone-a", &report(12, false), true, 0, 160, now);
        assert_eq!(
            warning.as_deref(),
            Some("Phone phone-a: battery low (12 %)")
        );
        assert_eq!(reports.batteries["phone-a"].updated_unix, 160);
        assert_eq!(
            reports.take("phone-b", &report(12, false), false, 0, 160, now),
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
    fn at_most_so_many_phones_are_kept_and_the_oldest_report_makes_room() {
        let mut reports = Reports::default();
        let now = Instant::now();
        for n in 0..MAX_PHONES {
            reports.take(
                &format!("phone-{n}"),
                &report(50, false),
                true,
                0,
                n as u64,
                now,
            );
        }
        reports.take("phone-new", &report(50, false), true, 0, 1000, now);
        assert_eq!(reports.batteries.len(), MAX_PHONES);
        assert!(
            reports.batteries.contains_key("phone-new"),
            "a new phone is kept"
        );
        assert!(
            !reports.batteries.contains_key("phone-0"),
            "the oldest went"
        );
        // The new phone is warned about too.
        assert!(
            reports
                .take("phone-new", &report(5, false), true, 0, 1001, now)
                .is_some()
        );
    }

    #[test]
    fn a_phone_no_longer_paired_is_forgotten() {
        let mut reports = Reports::default();
        let now = Instant::now();
        assert!(
            reports
                .take("phone-a", &report(5, false), true, 0, 1, now)
                .is_some()
        );
        reports.take("phone-b", &report(50, false), true, 0, 1, now);
        reports.dirty = false;
        let paired: std::collections::HashSet<String> = ["phone-b".to_string()].into();
        reports.keep_only(&paired);
        assert!(!reports.batteries.contains_key("phone-a"));
        assert!(reports.batteries.contains_key("phone-b"));
        assert!(reports.dirty, "the file must lose it too");
        reports.dirty = false;
        reports.keep_only(&paired);
        assert!(!reports.dirty, "nothing to forget, nothing to write");
        // Paired again, it starts afresh, so a low battery is warned about.
        assert!(
            reports
                .take("phone-a", &report(5, false), true, 0, 2, now)
                .is_some()
        );
    }

    fn report(charge: u8, charging: bool) -> BatteryReport {
        BatteryReport {
            charge,
            charging,
            low: false,
            state: Default::default(),
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
            state: Default::default(),
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
