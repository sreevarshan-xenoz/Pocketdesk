//! The tray's menu (D5), the same on Windows and Linux: Open, Pair a
//! phone…, each paired phone with what can be done for it now, and Quit.
//! Each tray turns [`Entry`]s into its own menu; what is enabled follows
//! the Overview's buttons (`model::phone_rows`), whose reasons the Overview
//! shows (a tray menu has no hover text).

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
    /// G2: the phone offers its camera; the photo lands here (F3).
    TakePhoto(String),
    /// N9: open the window that types on this phone (M4).
    Type(String),
    /// O2: a screenshot of the phone, or its screen locked.
    PhoneAction(String, pocketdesk_core::protocol::phone_action::PhoneAction),
    /// K2a: pause for these minutes, or until resumed with `None`.
    Pause(Option<u32>),
    /// K2a: end the pause.
    Resume,
    /// N2: a press for the phone's player (`playPause` or `next`).
    PhoneMedia(String, &'static str),
}

/// Whether O2's screenshot and lock reach the phone of `row` now.
fn phone_action_reach(row: &crate::model::PhoneRow) -> bool {
    crate::model::phone_action_reach(
        row.device.linked,
        row.device
            .permissions
            .allows(pocketdesk_core::protocol::permissions::Capability::PhoneActions),
    )
    .is_ok()
}

/// The longest song line the tray shows, in characters.
const SONG_CHARS: usize = 48;

/// The pause's menu, by what it holds (AU1: a bare "Pause" said so only on
/// hover); its hint names the rest.
pub const PAUSE_LABEL: &str = "Pause notifications";

/// The pauses offered (K2a): an hour, 8 hours, until resumed.
pub const PAUSES: [(&str, Option<u32>); 3] = [
    ("For an hour", Some(60)),
    ("For 8 hours", Some(480)),
    ("Until resumed", None),
];

/// `minutes` in words (K2a): "an hour", "8 hours", "90 minutes".
pub fn span(minutes: u32) -> String {
    match minutes {
        60 => "an hour".to_string(),
        m if m % 60 == 0 => format!("{} hours", m / 60),
        1 => "a minute".to_string(),
        m => format!("{m} minutes"),
    }
}

/// How a pause in force reads (K2a): "Paused, 42 min left" or "Paused
/// until resumed".
pub fn pause_text(pause: &pocketdesk_core::local::pause::Pause, now_unix: u64) -> String {
    match pause.minutes_left(now_unix) {
        Some(minutes) => format!("Paused, {minutes} min left"),
        None => "Paused until resumed".to_string(),
    }
}

/// `entries` with the pause after "Pair a phone" (K2a): while paused, how
/// long and Resume; else Pause with its three.
pub fn with_pause(
    mut entries: Vec<Entry>,
    pause: Option<&pocketdesk_core::local::pause::Pause>,
    now_unix: u64,
) -> Vec<Entry> {
    let at = entries
        .iter()
        .position(|e| {
            matches!(
                e,
                Entry::Item {
                    action: Some(TrayAction::Pair),
                    ..
                }
            )
        })
        .map_or(entries.len(), |i| i + 1);
    let added = match pause {
        Some(pause) => vec![
            Entry::Item {
                label: pause_text(pause, now_unix),
                action: None,
            },
            Entry::Item {
                label: "Resume".to_string(),
                action: Some(TrayAction::Resume),
            },
        ],
        None => vec![Entry::Phone {
            label: PAUSE_LABEL.to_string(),
            items: PAUSES
                .iter()
                .map(|(label, minutes)| Entry::Item {
                    label: (*label).to_string(),
                    action: Some(TrayAction::Pause(*minutes)),
                })
                .collect(),
        }],
    };
    entries.splice(at..at, added);
    entries
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
        item("Pair a phone", Some(TrayAction::Pair)),
    ];
    if !rows.is_empty() {
        entries.push(Entry::Separator);
    }
    for row in rows {
        let device = row.device;
        let id = || device.id.clone();
        let mut label = model::device_label(devices, &device.id);
        // A phone away has every item greyed: its title says why (D5
        // review), as the tray has no hover text.
        if row.place == model::Place::Away {
            label.push_str(" · Away");
        }
        // Why Mute is greyed or Unmute offered (E5): Do Not Disturb, or the
        // mute's minutes left.
        if let Some(quiet) = model::quiet_text(device.quiet.as_ref(), now_unix) {
            label.push_str(&format!(" · {quiet}"));
        }
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
        // N2: what it plays first, with Play/Pause and Next.
        let mut items = Vec::new();
        if let Some(playing) = row.playing {
            let media = &playing.media;
            let line = model::media_line(media);
            let line = if line.chars().count() > SONG_CHARS {
                format!("{}…", line.chars().take(SONG_CHARS - 1).collect::<String>())
            } else {
                line
            };
            items.push(item(
                &format!(
                    "{} {line}",
                    if media.is_playing {
                        "Playing:"
                    } else {
                        "Paused:"
                    }
                ),
                None,
            ));
            items.push(item(
                if media.is_playing { "Pause" } else { "Play" },
                media
                    .can_play_pause
                    .then(|| TrayAction::PhoneMedia(id(), "playPause")),
            ));
            items.push(item(
                "Next",
                media.can_next.then(|| TrayAction::PhoneMedia(id(), "next")),
            ));
            items.push(Entry::Separator);
        }
        entries.push(Entry::Phone {
            label,
            items: items
                .into_iter()
                .chain([
                    item("Ring", row.ring.is_ok().then(|| TrayAction::Ring(id()))),
                    item(
                        if muted { "Unmute" } else { "Mute for an hour" },
                        (row.mute.is_ok() && !owners)
                            .then(|| TrayAction::Mute(id(), if muted { 0 } else { 60 })),
                    ),
                    item(
                        "Send the clipboard",
                        row.clipboard
                            .is_ok()
                            .then(|| TrayAction::SendClipboard(id())),
                    ),
                    item(
                        "Send file…",
                        row.file.is_ok().then(|| TrayAction::SendFile(id())),
                    ),
                    // F3's ask reaches a phone files reach (G2).
                    item(
                        "Take a photo",
                        row.file.is_ok().then(|| TrayAction::TakePhoto(id())),
                    ),
                    // O2: as the Overview offers them, for a linked phone.
                    item(
                        "Screenshot of the phone",
                        phone_action_reach(row).then(|| {
                            TrayAction::PhoneAction(
                                id(),
                                pocketdesk_core::protocol::phone_action::PhoneAction::Screenshot,
                            )
                        }),
                    ),
                    item(
                        "Lock the phone",
                        phone_action_reach(row).then(|| {
                            TrayAction::PhoneAction(
                                id(),
                                pocketdesk_core::protocol::phone_action::PhoneAction::Lock,
                            )
                        }),
                    ),
                    // N9: M4's typing window, for a linked phone this
                    // computer may control; the window says what else it needs.
                    item(
                        "Type on the phone",
                        (row.device.linked
                            && row
                                .device
                                .permissions
                                .allows(pocketdesk_core::protocol::permissions::Capability::Cast))
                        .then(|| TrayAction::Type(id())),
                    ),
                ])
                .collect(),
        });
    }
    entries.push(Entry::Separator);
    entries.push(item("Quit", Some(TrayAction::Quit)));
    entries
}

/// The menu with `phone`'s "Send the clipboard" naming `key` (H4), the
/// key that sends it from anywhere (H3), so the key is found where the
/// action is. Other entries are left as they are.
pub fn with_key(mut entries: Vec<Entry>, phone: &str, key: &str) -> Vec<Entry> {
    for entry in &mut entries {
        if let Entry::Phone { items, .. } = entry {
            for item in items {
                if let Entry::Item {
                    label,
                    action: Some(TrayAction::SendClipboard(id)),
                } = item
                    && id == phone
                {
                    label.push_str(&format!(" ({key})"));
                }
            }
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    /// N2: a phone's music leads its items, with Play/Pause and Next as
    /// its player offers them.
    #[test]
    fn a_phone_s_music_leads_its_items() {
        use super::*;
        let mut device = crate::model::Device {
            id: "p".to_string(),
            linked: true,
            ..Default::default()
        };
        device.name = Some("Phone".to_string());
        let mut snapshot = crate::model::Snapshot {
            now_unix: 100,
            ..Default::default()
        };
        snapshot.devices = vec![device];
        snapshot.phone_media.insert(
            "p".to_string(),
            pocketdesk_core::local::phone_media::PhoneMediaState {
                media: pocketdesk_core::protocol::phone_media::PhoneMedia {
                    title: "A song whose name runs on and on and on past the tray".to_string(),
                    is_playing: true,
                    can_play_pause: true,
                    background: true,
                    ..Default::default()
                },
                updated_unix: 100,
                art: None,
            },
        );
        let rows = crate::model::phone_rows(&snapshot, &Default::default());
        let entries = menu(&rows, &snapshot.devices, 100);
        let Entry::Phone { items, .. } = &entries[3] else {
            panic!("the phone: {entries:?}");
        };
        let Entry::Item {
            label,
            action: None,
        } = &items[0]
        else {
            panic!("the song first: {items:?}");
        };
        assert!(label.starts_with("Playing: A song"), "{label}");
        assert!(
            label.chars().count() <= "Playing: ".len() + SONG_CHARS,
            "{label}"
        );
        assert_eq!(
            items[1],
            Entry::Item {
                label: "Pause".to_string(),
                action: Some(TrayAction::PhoneMedia("p".to_string(), "playPause")),
            }
        );
        assert_eq!(
            items[2],
            Entry::Item {
                label: "Next".to_string(),
                action: None,
            },
            "its player offers no Next"
        );
        assert_eq!(items[3], Entry::Separator);
    }

    /// K2a: the menu offers Pause, and while paused says so with Resume.
    #[test]
    fn the_menu_offers_a_pause_and_resume() {
        use super::*;
        let entries = with_pause(menu(&[], &[], 0), None, 1_000);
        let Entry::Phone { label, items } = &entries[2] else {
            panic!("Pause after Pair: {entries:?}");
        };
        assert_eq!(label, "Pause notifications");
        assert_eq!(items.len(), 3);
        let pause = pocketdesk_core::local::pause::Pause {
            since_unix: 1_000,
            until_unix: Some(1_000 + 2_520),
        };
        let entries = with_pause(menu(&[], &[], 0), Some(&pause), 1_000);
        assert_eq!(
            entries[2],
            Entry::Item {
                label: "Paused, 42 min left".to_string(),
                action: None
            }
        );
        assert_eq!(
            entries[3],
            Entry::Item {
                label: "Resume".to_string(),
                action: Some(TrayAction::Resume)
            }
        );
        let forever = pocketdesk_core::local::pause::Pause {
            since_unix: 1,
            until_unix: None,
        };
        assert_eq!(pause_text(&forever, 9_999_999), "Paused until resumed");
        assert_eq!(span(60), "an hour");
        assert_eq!(span(480), "8 hours");
        assert_eq!(span(90), "90 minutes");
        assert_eq!(span(1), "a minute");
    }

    use super::*;
    use crate::model::Snapshot;
    use pocketdesk_core::local::phone_quiet::PhoneQuiet;
    use pocketdesk_core::local::phone_status::PhoneBattery;
    use pocketdesk_core::protocol::permissions::Capability;

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
                ("Pair a phone".to_string(), true),
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
                ..Default::default()
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
        assert_eq!(
            labels(&menu)[4].0,
            "Old · Away",
            "away said; an hour-old battery left out"
        );
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
                    label: "Send the clipboard".to_string(),
                    action: Some(TrayAction::SendClipboard("ready".to_string()))
                },
                Entry::Item {
                    label: "Send file…".to_string(),
                    action: Some(TrayAction::SendFile("ready".to_string()))
                },
                Entry::Item {
                    label: "Take a photo".to_string(),
                    action: Some(TrayAction::TakePhoto("ready".to_string()))
                },
                Entry::Item {
                    label: "Screenshot of the phone".to_string(),
                    action: Some(TrayAction::PhoneAction(
                        "ready".to_string(),
                        pocketdesk_core::protocol::phone_action::PhoneAction::Screenshot
                    ))
                },
                Entry::Item {
                    label: "Lock the phone".to_string(),
                    action: Some(TrayAction::PhoneAction(
                        "ready".to_string(),
                        pocketdesk_core::protocol::phone_action::PhoneAction::Lock
                    ))
                },
                Entry::Item {
                    label: "Type on the phone".to_string(),
                    action: Some(TrayAction::Type("ready".to_string()))
                },
            ]
        );
        assert!(
            labels(items(phones[1])).iter().all(|(_, on)| !on),
            "away: nothing can reach it"
        );
    }

    /// H4: the key is named on its phone's Send the clipboard, nowhere else.
    #[test]
    fn the_key_is_named_on_its_phone_s_send_the_clipboard() {
        let send = |id: &str| Entry::Item {
            label: "Send the clipboard".to_string(),
            action: Some(TrayAction::SendClipboard(id.to_string())),
        };
        let ring = |id: &str| Entry::Item {
            label: "Ring".to_string(),
            action: Some(TrayAction::Ring(id.to_string())),
        };
        let menu = vec![
            Entry::Phone {
                label: "A".to_string(),
                items: vec![ring("a"), send("a")],
            },
            Entry::Phone {
                label: "B".to_string(),
                items: vec![send("b")],
            },
        ];
        let marked = with_key(menu, "a", "Win+Shift+C");
        assert_eq!(
            items(&marked[0])[1],
            Entry::Item {
                label: "Send the clipboard (Win+Shift+C)".to_string(),
                action: Some(TrayAction::SendClipboard("a".to_string())),
            }
        );
        assert_eq!(labels(items(&marked[0]))[0].0, "Ring");
        assert_eq!(labels(items(&marked[1]))[0].0, "Send the clipboard");
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

    #[test]
    fn a_phone_s_title_says_why_mute_is_greyed_or_unmute_offered() {
        let title_of = |quiet: PhoneQuiet| {
            let mut phone = Device {
                id: "p".to_string(),
                name: Some("Pixel".to_string()),
                linked: true,
                quiet: Some(quiet),
                ..Default::default()
            };
            phone.permissions.set(Capability::Mute, true);
            let snapshot = Snapshot {
                devices: vec![phone],
                ..Default::default()
            };
            let rows = model::phone_rows(&snapshot, &Default::default());
            match &menu(&rows, &snapshot.devices, 1_000)[3] {
                Entry::Phone { label, .. } => label.clone(),
                other => panic!("not a phone: {other:?}"),
            }
        };
        let quiet = PhoneQuiet {
            quiet: true,
            until_unix: None,
            by_computer: false,
            updated_unix: 1_000,
            refused_unix: None,
        };
        assert!(title_of(quiet).ends_with(" · Do Not Disturb"));
        assert!(
            title_of(PhoneQuiet {
                until_unix: Some(1_000 + 25 * 60),
                by_computer: true,
                ..quiet
            })
            .ends_with(" · muted from here, 25 min left")
        );
    }
}
