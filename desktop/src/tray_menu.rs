//! The tray's menu (D5), the same on Windows and Linux: Open, Pair a
//! phone…, each paired phone with what can be done for it now, and Quit.
//! Each tray turns [`Entry`]s into its own menu; what is enabled follows
//! the Overview's buttons (`model::phone_rows`), whose reasons the Overview
//! shows (a tray menu has no hover text).

use pocketdesk_core::protocol::permissions::Capability;

use crate::model::{self, Device, PhoneRow};

/// What the user picked from the tray menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayAction {
    Open,
    Pair,
    Quit,
    Ring(String),
    /// A phone and minutes: 60 mutes for an hour, 0 ends a mute (D2).
    Mute(String, u32),
    /// The computer's clipboard now, read by the server.
    SendClipboard(String),
    /// The window, on Files with this phone chosen.
    SendFile(String),
}

/// One line of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// `None`: shown, but cannot be picked now.
    Item {
        label: String,
        action: Option<TrayAction>,
    },
    /// A phone and its items.
    Phone {
        label: String,
        items: Vec<Entry>,
    },
    Separator,
}

/// A battery older than this is left out of a phone's title: the tray has
/// no room to say how old it is.
const BATTERY_FRESH_SECS: u64 = 60 * 60;

/// The menu for `rows` (paired phones, from `model::phone_rows`).
pub fn menu(rows: &[PhoneRow], devices: &[Device], now_unix: u64) -> Vec<Entry> {
    let item = |label: &str, action: Option<TrayAction>| Entry::Item {
        label: label.to_string(),
        action,
    };
    let mut entries = vec![
        item("Open Pocketdesk", Some(TrayAction::Open)),
        item("Pair a phone…", Some(TrayAction::Pair)),
    ];
    if !rows.is_empty() {
        entries.push(Entry::Separator);
    }
    for row in rows {
        let device = row.device;
        let id = || device.id.clone();
        let mut label = model::device_label(devices, &device.id);
        if let Some(battery) = device
            .battery
            .as_ref()
            .filter(|b| now_unix.saturating_sub(b.updated_unix) < BATTERY_FRESH_SECS)
        {
            label.push_str(&format!(" · {} %", battery.charge));
            if battery.charging {
                label.push_str(", charging");
            }
        }
        // Mute as the Overview offers it: Unmute only a mute asked from
        // here, and nothing over the owner's own Do Not Disturb.
        let muted = model::muted_from_here(device.quiet.as_ref(), now_unix);
        let owners = !muted && model::quiet_by_owner(device.quiet.as_ref());
        let clipboard = device.linked && device.permissions.allows(Capability::Clipboard);
        entries.push(Entry::Phone {
            label,
            items: vec![
                item("Ring", row.ring.is_ok().then(|| TrayAction::Ring(id()))),
                item(
                    if muted { "Unmute" } else { "Mute for an hour" },
                    (row.mute.is_ok() && !owners)
                        .then(|| TrayAction::Mute(id(), if muted { 0 } else { 60 })),
                ),
                item(
                    "Send clipboard",
                    clipboard.then(|| TrayAction::SendClipboard(id())),
                ),
                item(
                    "Send file…",
                    row.file.is_ok().then(|| TrayAction::SendFile(id())),
                ),
            ],
        });
    }
    entries.push(Entry::Separator);
    entries.push(item("Quit", Some(TrayAction::Quit)));
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Snapshot;
    use pocketdesk_core::local::phone_quiet::PhoneQuiet;
    use pocketdesk_core::local::phone_status::PhoneBattery;

    fn labels(entries: &[Entry]) -> Vec<(String, bool)> {
        entries
            .iter()
            .map(|e| match e {
                Entry::Item { label, action } => (label.clone(), action.is_some()),
                Entry::Phone { label, .. } => (label.clone(), true),
                Entry::Separator => ("-".to_string(), false),
            })
            .collect()
    }

    fn items(entry: &Entry) -> &[Entry] {
        match entry {
            Entry::Phone { items, .. } => items,
            _ => panic!("not a phone: {entry:?}"),
        }
    }

    #[test]
    fn with_no_phone_the_menu_is_open_pair_and_quit() {
        let menu = menu(&[], &[], 0);
        assert_eq!(
            labels(&menu),
            [
                ("Open Pocketdesk".to_string(), true),
                ("Pair a phone…".to_string(), true),
                ("-".to_string(), false),
                ("Quit".to_string(), true),
            ]
        );
    }

    #[test]
    fn each_phone_has_its_battery_and_what_it_can_do_now() {
        let mut ready = Device {
            id: "ready".to_string(),
            name: Some("Pixel".to_string()),
            linked: true,
            takes_files: true,
            battery: Some(PhoneBattery {
                charge: 64,
                charging: true,
                updated_unix: 10_000 - 60,
            }),
            ..Default::default()
        };
        for capability in [
            Capability::Ring,
            Capability::Mute,
            Capability::Files,
            Capability::Clipboard,
        ] {
            ready.permissions.set(capability, true);
        }
        let mut away = ready.clone();
        away.id = "away".to_string();
        away.name = Some("Old".to_string());
        away.linked = false;
        away.takes_files = false;
        away.battery.as_mut().unwrap().updated_unix = 0;
        let snapshot = Snapshot {
            devices: vec![ready, away],
            ..Default::default()
        };
        let rows = model::phone_rows(&snapshot, &Default::default());
        let menu = menu(&rows, &snapshot.devices, 10_000);
        let phones: Vec<&Entry> = menu
            .iter()
            .filter(|e| matches!(e, Entry::Phone { .. }))
            .collect();
        assert_eq!(phones.len(), 2);
        assert_eq!(labels(&menu)[3].0, "Pixel · 64 %, charging");
        assert_eq!(labels(&menu)[4].0, "Old", "an hour-old battery is left out");
        assert_eq!(
            items(phones[0]),
            [
                Entry::Item {
                    label: "Ring".to_string(),
                    action: Some(TrayAction::Ring("ready".to_string()))
                },
                Entry::Item {
                    label: "Mute for an hour".to_string(),
                    action: Some(TrayAction::Mute("ready".to_string(), 60))
                },
                Entry::Item {
                    label: "Send clipboard".to_string(),
                    action: Some(TrayAction::SendClipboard("ready".to_string()))
                },
                Entry::Item {
                    label: "Send file…".to_string(),
                    action: Some(TrayAction::SendFile("ready".to_string()))
                },
            ]
        );
        assert!(
            labels(items(phones[1])).iter().all(|(_, on)| !on),
            "away: nothing can reach it"
        );
    }

    /// D2's rules: Unmute for a mute from here, nothing over the owner's own.
    #[test]
    fn mute_follows_the_overview() {
        let mut phone = Device {
            id: "p".to_string(),
            linked: true,
            ..Default::default()
        };
        phone.permissions.set(Capability::Mute, true);
        let mute_of = |quiet: Option<PhoneQuiet>| {
            let mut phone = phone.clone();
            phone.quiet = quiet;
            let snapshot = Snapshot {
                devices: vec![phone],
                ..Default::default()
            };
            let rows = model::phone_rows(&snapshot, &Default::default());
            let menu = menu(&rows, &snapshot.devices, 1_000);
            items(&menu[3])[1].clone()
        };
        assert_eq!(
            mute_of(Some(PhoneQuiet {
                quiet: true,
                until_unix: Some(2_000),
                by_computer: true,
                updated_unix: 1_000,
                refused_unix: None,
            })),
            Entry::Item {
                label: "Unmute".to_string(),
                action: Some(TrayAction::Mute("p".to_string(), 0))
            }
        );
        assert_eq!(
            mute_of(Some(PhoneQuiet {
                quiet: true,
                until_unix: None,
                by_computer: false,
                updated_unix: 1_000,
                refused_unix: None,
            })),
            Entry::Item {
                label: "Mute for an hour".to_string(),
                action: None
            },
            "the owner's own Do Not Disturb is left alone"
        );
    }
}
