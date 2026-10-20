//! Which features each phone has on (L7): a paired phone reports
//! `kdeconnect.pocketdesk.switches`, and the server keeps the newest per
//! phone in `<state>/phone_switches.json` for the Devices page.
//!
//! Like the battery, it is status, not an action, so no permission gates
//! it; only a paired phone's report is taken. A phone no longer paired is
//! dropped at the next write.

use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex};

use pocketdesk_core::error::Result;
use pocketdesk_core::local::paths::TRUST_FILE;
use pocketdesk_core::local::phone_switches::{self, PhoneSwitches, SWITCHES};
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};

/// The most phones whose reports are kept.
const MAX_PHONES: usize = 64;

/// The reports, as last written; `None` until read from the file.
static REPORTS: LazyLock<Mutex<Option<BTreeMap<String, PhoneSwitches>>>> =
    LazyLock::new(Default::default);

#[derive(Debug, Default)]
pub struct PhoneSwitchesPlugin;

#[async_trait::async_trait]
impl Plugin for PhoneSwitchesPlugin {
    fn name(&self) -> &'static str {
        "phone_switches"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[SWITCHES]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type != SWITCHES {
            return Ok(());
        }
        let device = sender.device_id().to_string();
        if !crate::plugins::pair::is_paired_device(&device) {
            tracing::debug!("A switches report from unpaired {device} was ignored");
            return Ok(());
        }
        let Some(switches) = phone_switches::from_body(&packet.body) else {
            tracing::debug!("A switches report from {device} was not read");
            return Ok(());
        };
        let Ok(dir) = crate::state::state_dir() else {
            return Ok(());
        };
        // Off the runtime: a read and a write of small files.
        tokio::task::spawn_blocking(move || {
            keep(
                &dir,
                &device,
                switches,
                pocketdesk_core::local::pin::now_unix(),
            )
        })
        .await
        .ok();
        Ok(())
    }
}

/// Keep `device`'s `switches` and write the reports, those of phones no
/// longer paired left out; nothing is written when the report is the one
/// kept (apart from its age, which only a change needs).
fn keep(
    dir: &std::path::Path,
    device: &str,
    switches: BTreeMap<String, phone_switches::Switch>,
    now_unix: u64,
) {
    let mut reports = REPORTS.lock().unwrap_or_else(|e| e.into_inner());
    let phones = reports.get_or_insert_with(|| phone_switches::read(dir));
    if phones.get(device).is_some_and(|kept| {
        kept.switches == switches && now_unix.saturating_sub(kept.updated_unix) < 60
    }) {
        return;
    }
    if !phones.contains_key(device) && phones.len() >= MAX_PHONES {
        let oldest = phones
            .iter()
            .min_by_key(|(_, p)| p.updated_unix)
            .map(|(id, _)| id.clone());
        if let Some(oldest) = oldest {
            phones.remove(&oldest);
        }
    }
    phones.insert(
        device.to_string(),
        PhoneSwitches {
            switches,
            updated_unix: now_unix,
        },
    );
    // An unreadable store forgets no one.
    if let Ok(store) = pocketdesk_core::protocol::kdeconnect::TrustStore::read(dir.join(TRUST_FILE))
    {
        let paired: std::collections::HashSet<String> =
            store.trusted_devices().into_iter().collect();
        phones.retain(|id, _| paired.contains(id));
    }
    if let Err(e) = phone_switches::write(dir, phones) {
        tracing::warn!("Could not keep the phones' switches: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::local::phone_switches::Switch;

    #[test]
    fn a_paired_phones_report_is_kept_and_an_unpaired_ones_dropped() {
        let dir = std::env::temp_dir().join(format!(
            "pd-phone-switches-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        pocketdesk_core::protocol::kdeconnect::TrustStore::update(dir.join(TRUST_FILE), |store| {
            store.trust_device("phone-a")
        })
        .unwrap();
        // A report kept in an earlier run for a phone since unpaired.
        let mut old = BTreeMap::new();
        old.insert("gone".to_string(), PhoneSwitches::default());
        phone_switches::write(&dir, &old).unwrap();
        *REPORTS.lock().unwrap() = None;

        let mut switches = BTreeMap::new();
        switches.insert(
            "links".to_string(),
            Switch {
                on: true,
                android: None,
            },
        );
        keep(&dir, "phone-a", switches.clone(), 100);
        let kept = phone_switches::read(&dir);
        assert_eq!(kept.keys().collect::<Vec<_>>(), ["phone-a"]);
        assert_eq!(kept["phone-a"].switches, switches);
        assert_eq!(kept["phone-a"].updated_unix, 100);
        *REPORTS.lock().unwrap() = None;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
