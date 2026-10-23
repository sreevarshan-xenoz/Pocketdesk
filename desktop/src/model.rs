//! What the app shows, read from the files the server keeps — plain data,
//! no window, so it is tested directly.

use std::path::Path;

use pocketdesk_core::local::approval::{self, ApprovalRequest, Verdict};
use pocketdesk_core::local::paths::{LIVE_FILE, LOG_DIR, TRUST_FILE};
use pocketdesk_core::local::received::{self, ReceivedFile};
use pocketdesk_core::local::sent::{self, SentFile};
use pocketdesk_core::local::{server, sessions};
use pocketdesk_core::protocol::kdeconnect::TrustStore;
use pocketdesk_core::protocol::permissions::{Capability, DevicePermissions};

/// A paired phone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    /// U3: the owner's name for it, else what it calls itself.
    pub name: Option<String>,
    /// Whether [`Device::name`] is the owner's, not the phone's own claim.
    pub renamed: bool,
    /// Seconds left on a time-boxed grant; `None` is permanent.
    pub grant_remaining: Option<u64>,
    /// Whether it has a live streaming session right now.
    pub connected: bool,
    /// Whether a file can be offered to it now: holding a control link
    /// files can ride (T10d), which a screen or sound session keeps and a
    /// remote does not.
    pub takes_files: bool,
    /// Whether it keeps a link now, a session's or the background one, so
    /// a ring can reach it (N1).
    pub linked: bool,
    /// What the owner lets it do (roadmap F5).
    pub permissions: DevicePermissions,
    /// Its last battery report (T8b), if it ever sent one.
    pub battery: Option<pocketdesk_core::local::phone_status::PhoneBattery>,
    /// Its Do Not Disturb as it last said (D2), if it ever did.
    pub quiet: Option<pocketdesk_core::local::phone_quiet::PhoneQuiet>,
    /// Which of its features it has on, as it last said (L7).
    pub switches: Option<pocketdesk_core::local::phone_switches::PhoneSwitches>,
    /// Another of the owner's computers, paired as one (2f).
    pub computer: bool,
}

/// Everything the window shows, read once per refresh.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub running: bool,
    /// When it was read (unix seconds), for what ages (N2's music).
    pub now_unix: u64,
    pub devices: Vec<Device>,
    pub sessions: Vec<sessions::MirrorSession>,
    /// Every phone drives, as the running server says (N7).
    pub shared_control: bool,
    /// Sessions the server is holding for the owner's answer (F5).
    pub approvals: Vec<ApprovalRequest>,
    /// Files received from phones, newest first (F7).
    pub received: Vec<ReceivedRow>,
    /// Files sent to phones and how each went, newest first (F7b).
    pub sent: Vec<SentFile>,
    /// What each phone in a session plays, as it last said (T8d).
    pub phone_media:
        std::collections::BTreeMap<String, pocketdesk_core::local::phone_media::PhoneMediaState>,
    /// The other computers this one paired with whose link is up (2h), by
    /// id and name: files can go to them.
    pub linked_computers: Vec<(String, String)>,
    /// Files moving now, as the running server last wrote (C1).
    pub under_way: Vec<pocketdesk_core::local::transfers::Transfer>,
}

/// A received file, and whether it is still where it was saved (checked
/// once per refresh, not on every repaint).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedRow {
    pub file: ReceivedFile,
    pub exists: bool,
}

/// The state under `state_dir` at `now_unix`.
pub fn snapshot(state_dir: &Path, now_unix: u64) -> Snapshot {
    snapshot_with(state_dir, now_unix, server::is_running(state_dir))
}

/// [`snapshot`] with the liveness answer supplied, so it is testable without
/// a server process.
pub fn snapshot_with(state_dir: &Path, now_unix: u64, running: bool) -> Snapshot {
    // A mirror is believed only while the server that writes it is alive
    // and has written it recently; a crash leaves the last one behind.
    let mirror = std::fs::read_to_string(state_dir.join(LIVE_FILE))
        .ok()
        .and_then(|raw| sessions::parse_mirror(&raw))
        .filter(|m| running && m.is_fresh(now_unix));
    let shared_control = mirror.as_ref().is_some_and(|m| m.shared_control);
    let takes_files = |id: &str| mirror.as_ref().is_some_and(|m| m.files_reach(id));
    let sessions = mirror.clone().map(|m| m.sessions).unwrap_or_default();
    let batteries = pocketdesk_core::local::phone_status::read(state_dir);
    let quiet = pocketdesk_core::local::phone_quiet::read(state_dir);
    let mut switches = pocketdesk_core::local::phone_switches::read(state_dir);
    let devices = match TrustStore::load_or_create(state_dir.join(TRUST_FILE)) {
        Ok(store) => store
            .trusted_devices()
            .into_iter()
            .map(|id| {
                let linked = mirror
                    .as_ref()
                    .is_some_and(|m| m.linked.iter().any(|p| p.device_id == id));
                Device {
                    name: store.name(&id).map(str::to_string),
                    // The owner's, even when it matches the phone's own (U3 review).
                    renamed: store.chosen_name(&id).is_some(),
                    grant_remaining: store.grant_remaining(&id),
                    connected: sessions.iter().any(|s| s.device_id.as_deref() == Some(&id)),
                    takes_files: takes_files(&id),
                    // A control link carries the ring (N1 review).
                    linked,
                    permissions: store.permissions(&id),
                    battery: batteries.get(&id).copied(),
                    // Only a phone linked now (D2 review): it says when its quiet
                    // changes, so an old report from a phone away is not now's.
                    quiet: if linked {
                        quiet.get(&id).copied()
                    } else {
                        None
                    },
                    computer: store.is_computer(&id),
                    switches: switches.remove(&id),
                    id,
                }
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    // Like the mirror: only a running server is still waiting for an answer.
    let approvals = if running {
        approval::read_pending(state_dir, now_unix)
    } else {
        Vec::new()
    };
    let received = received::read(state_dir)
        .into_iter()
        .rev()
        .map(|file| ReceivedRow {
            exists: Path::new(&file.path).exists(),
            file,
        })
        .collect();
    let mut sent = sent::read(state_dir);
    sent.reverse();
    Snapshot {
        running,
        now_unix,
        devices,
        sessions,
        shared_control,
        approvals,
        received,
        sent,
        // Only a running server's: what played in an earlier run is not
        // playing now.
        phone_media: if running {
            pocketdesk_core::local::phone_media::read(state_dir)
        } else {
            Default::default()
        },
        linked_computers: if running {
            pocketdesk_core::local::computers::read(state_dir)
                .computers
                .into_iter()
                .filter(|c| c.link == "linked")
                .map(|c| (c.device_id, c.name))
                .collect()
        } else {
            Vec::new()
        },
        under_way: if running {
            pocketdesk_core::local::transfers::under_way(state_dir, now_unix)
        } else {
            Vec::new()
        },
    }
}

/// A file under way by its place: "Holiday/beach.jpg" for a file of a
/// folder send (C1).
pub fn transfer_title(transfer: &pocketdesk_core::local::transfers::Transfer) -> String {
    match &transfer.folder {
        Some(folder) => format!("{folder}/{}", transfer.name),
        None => transfer.name.clone(),
    }
}

/// How far a file under way is, for its bar: 0 to 1.
pub fn transfer_fraction(transfer: &pocketdesk_core::local::transfers::Transfer) -> f32 {
    if transfer.size == 0 {
        return 0.0;
    }
    (transfer.done.min(transfer.size) as f64 / transfer.size as f64) as f32
}

/// What a file under way's bar says: "42 % · 3.1 MB/s", the rate left out
/// until there is one, and the bytes when the size is not known.
pub fn transfer_words(transfer: &pocketdesk_core::local::transfers::Transfer) -> String {
    let rate = if transfer.rate > 0 {
        format!(" · {}/s", crate::words::human_size(transfer.rate))
    } else {
        String::new()
    };
    if transfer.size == 0 {
        return format!("{}{rate}", crate::words::human_size(transfer.done));
    }
    let percent = transfer.done.min(transfer.size) * 100 / transfer.size;
    format!("{percent} %{rate}")
}

/// What the Sessions tab shows about who drives (roadmap N7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlCell {
    /// This session's phone drives.
    Driving,
    /// Another phone drives or no one does: a button hands control here.
    Give(u64),
    /// One phone, or a server too old to say.
    Nothing,
}

pub fn control_cell(
    session: &sessions::MirrorSession,
    sessions_live: usize,
    shared_control: bool,
) -> ControlCell {
    // A sound session, a cast or the phone's sound alone gives no input:
    // made the driver, it would only block the phone that does (N5 review).
    if sessions_live < 2
        || session.id == 0
        || shared_control
        || session.sound_only
        || session.cast
        || session.phone_sound_only
    {
        ControlCell::Nothing
    } else if session.driving {
        ControlCell::Driving
    } else {
        ControlCell::Give(session.id)
    }
}

/// Where a phone is now, for the Overview (N6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// In a session with this computer.
    Session,
    /// Not in a session, but a background feature keeps a link.
    Linked,
    /// Neither: nothing reaches it now.
    Away,
}

/// One phone on the Overview (N6): where it is, its last session, and
/// whether each quick action can reach it now, or why not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneRow<'a> {
    pub device: &'a Device,
    pub place: Place,
    /// When its last session was recorded (unix seconds), if one is.
    pub last_session: Option<u64>,
    pub ring: Result<(), &'static str>,
    /// D2: Mute for an hour, or Unmute.
    pub mute: Result<(), &'static str>,
    pub file: Result<(), &'static str>,
    pub link: Result<(), &'static str>,
    /// A text to its clipboard (D3's Send to…, D5's tray).
    pub clipboard: Result<(), &'static str>,
    /// What it plays, when that may show (N2: in a session, or linked
    /// with the phone's background reporter on).
    pub playing: Option<&'a pocketdesk_core::local::phone_media::PhoneMediaState>,
}

/// What a phone plays in a line: "Song · Artist (App)", the parts it has.
pub fn media_line(media: &pocketdesk_core::protocol::phone_media::PhoneMedia) -> String {
    let what = [media.title.as_str(), media.artist.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    match (what.is_empty(), media.app.is_empty()) {
        (false, false) => format!("{what} ({})", media.app),
        (false, true) => what,
        (true, false) => media.app.clone(),
        (true, true) => "something".to_string(),
    }
}

/// Whether a text can go to `device`'s clipboard now: one rule for the
/// Clipboard page and the tray (D5 review).
pub fn clipboard_reach(device: &Device) -> Result<(), &'static str> {
    if !device.permissions.allows(Capability::Clipboard) {
        Err("Clipboard is off for it, on the Devices page.")
    } else if !device.linked {
        Err(
            "Not linked now: a text reaches it while it keeps a link to this computer, which a session or a background feature opens.",
        )
    } else {
        Ok(())
    }
}

/// The Overview's phones (N6): paired phones only (other computers have
/// their own section in Settings), with `last` from [`last_sessions`].
pub fn phone_rows<'a>(
    snapshot: &'a Snapshot,
    last: &std::collections::HashMap<String, u64>,
) -> Vec<PhoneRow<'a>> {
    snapshot
        .devices
        .iter()
        .filter(|d| !d.computer)
        .map(|device| {
            let place = if device.connected {
                Place::Session
            } else if device.linked {
                Place::Linked
            } else {
                Place::Away
            };
            let allows = |capability| device.permissions.allows(capability);
            PhoneRow {
                device,
                place,
                last_session: last.get(&device.id).copied(),
                ring: if !allows(Capability::Ring) {
                    Err("Ring is off for this phone, on the Devices page.")
                } else if !device.linked {
                    Err("Not linked now: it rings while it keeps a link to this computer, which a session or \"Ring from the computer\" in Pocketdesk's Settings on the phone opens.")
                } else {
                    Ok(())
                },
                mute: if !allows(Capability::Mute) {
                    Err("Mute is off for this phone, on the Devices page.")
                } else if !device.linked {
                    Err("Not linked now: it is muted while it keeps a link to this computer, which \"Let the computer mute this phone\" in Pocketdesk's Settings on the phone opens.")
                } else {
                    Ok(())
                },
                file: if !allows(Capability::Files) {
                    Err("Files are off for this phone, on the Devices page.")
                } else if !device.takes_files {
                    Err("Files reach it in a screen or sound session (not the remote alone), or at home with \"Files from the computer in the background\" on in Pocketdesk's Settings on the phone.")
                } else {
                    Ok(())
                },
                link: if !allows(Capability::Links) {
                    Err("Links are off for this phone, on the Devices page.")
                } else if !device.linked {
                    // The server pushes a link over the control link only.
                    Err("Not linked now: a link reaches it while it keeps a link to this computer, which a session or a background feature opens.")
                } else {
                    Ok(())
                },
                clipboard: clipboard_reach(device),
                playing: snapshot.phone_media.get(&device.id).filter(|state| {
                    crate::phone_player::may_show(
                        state,
                        device.connected,
                        device.linked,
                        snapshot.now_unix,
                    )
                }),
            }
        })
        .collect()
}

/// What a phone's Do Not Disturb is now, for the Overview (D2): `None` when
/// it is not quiet, or never said. A mute from here says how long is left.
pub fn quiet_text(
    quiet: Option<&pocketdesk_core::local::phone_quiet::PhoneQuiet>,
    now_unix: u64,
) -> Option<String> {
    let quiet = quiet.filter(|q| q.quiet)?;
    Some(match quiet.until_unix {
        Some(until) if quiet.by_computer && until > now_unix => {
            let minutes = (until - now_unix).div_ceil(60);
            format!("muted from here, {minutes} min left")
        }
        _ => "Do Not Disturb".to_string(),
    })
}

/// What a phone last said of itself beside its battery (L9), for the
/// Overview: "Wi-Fi, 3 of 4 bars · 12.3 GB free of 128 GB · on silent".
/// `None` when it said nothing of it (a KDE device, an older phone).
pub fn phone_state_text(
    state: &pocketdesk_core::local::phone_status::PhoneState,
) -> Option<String> {
    use pocketdesk_core::local::phone_status::{Network, Ringer};
    let mut parts = Vec::new();
    let bars = state.signal.map(|bars| format!(", {bars} of 4 bars"));
    match state.network {
        Some(Network::Wifi) => parts.push(format!("Wi-Fi{}", bars.unwrap_or_default())),
        Some(Network::Mobile) => parts.push(format!("mobile data{}", bars.unwrap_or_default())),
        Some(Network::Ethernet) => parts.push("Ethernet".to_string()),
        Some(Network::None) => parts.push("no network".to_string()),
        None => {}
    }
    match (state.storage_free_mb, state.storage_total_mb) {
        (Some(free), Some(total)) => parts.push(format!(
            "{} free of {}",
            storage_words(free),
            storage_words(total)
        )),
        (Some(free), None) => parts.push(format!("{} free", storage_words(free))),
        _ => {}
    }
    match state.ringer {
        Some(Ringer::Silent) => parts.push("on silent".to_string()),
        Some(Ringer::Vibrate) => parts.push("on vibrate".to_string()),
        Some(Ringer::Normal) => parts.push("ringer on".to_string()),
        None => {}
    }
    if parts.is_empty() {
        return None;
    }
    // The first word capitalised: it starts the line.
    let line = parts.join(" · ");
    let mut chars = line.chars();
    Some(match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => line,
    })
}

/// How long a phone's state is shown after its report: it resends every
/// five minutes while reached, so twice that is a report that stopped.
pub const STATE_FRESH_SECS: u64 = 600;

/// Whether the Overview shows a phone's state (L9 review): only while the
/// phone is reached, and its report is recent.
pub fn state_is_now(
    battery: &pocketdesk_core::local::phone_status::PhoneBattery,
    place: Place,
    now_unix: u64,
) -> bool {
    place != Place::Away && now_unix.saturating_sub(battery.updated_unix) <= STATE_FRESH_SECS
}

/// A phone's storage in decimal units, as its own Settings says it (L9
/// review): "128 GB", "12.3 GB", "800 MB". Free space comes rounded down
/// to 100 MB, so 0 is "under 100 MB".
fn storage_words(mb: u64) -> String {
    if mb == 0 {
        return "under 100 MB".to_string();
    }
    if mb < 1000 {
        return format!("{mb} MB");
    }
    let gb = mb as f64 / 1000.0;
    if gb >= 100.0 {
        format!("{gb:.0} GB")
    } else {
        format!("{gb:.1} GB")
    }
}

/// A feature both sides must have on (L7): its name in the phone's report,
/// what it is, the phone's switch for it, this computer's permission for
/// it, and what Android must let the app do.
pub struct Feature {
    pub key: &'static str,
    pub what: &'static str,
    pub phone_switch: &'static str,
    /// The permission's label on the Devices page (a test there holds it).
    pub permission_name: &'static str,
    pub android: &'static str,
}

impl Feature {
    /// This computer's permission for it, from core's one table (M2), which
    /// the phone's mirror reads too.
    pub fn permission(&self) -> Capability {
        pocketdesk_core::local::phone_switches::capability(self.key)
            .expect("every feature has a permission")
    }
}

/// The features the Devices page lines up, in the phone's order
/// (`phone_switches::FEATURES`; a test holds the two together).
pub const FEATURES: &[Feature] = &[
    Feature {
        key: "notifications",
        what: "Its notifications here",
        phone_switch: "This phone's notifications on my computer",
        permission_name: "Show its notifications here",
        android: "notification access",
    },
    Feature {
        key: "answers",
        what: "Answering them from here",
        phone_switch: "Let the computer answer notifications",
        permission_name: "Show its notifications here",
        android: "notification access",
    },
    Feature {
        key: "texts",
        what: "Its texts here",
        phone_switch: "Show texts on my computer",
        permission_name: "Show its texts and send replies",
        android: "the SMS permission",
    },
    Feature {
        key: "clipboard",
        what: "The clipboard without a session",
        phone_switch: "Computer's clipboard in the background",
        permission_name: "Share the clipboard",
        android: "notification access",
    },
    Feature {
        key: "files",
        what: "Files from here without a session",
        phone_switch: "Files from the computer in the background",
        permission_name: "Send files",
        android: "notification access and its notifications",
    },
    Feature {
        key: "folders",
        what: "Its folders here",
        phone_switch: "Folders the computer may open",
        permission_name: "Send files",
        android: "notification access",
    },
    Feature {
        key: "photos",
        what: "Its recent photos here",
        phone_switch: "Recent photos on the computer",
        permission_name: "Send files",
        android: "notification access and photo access",
    },
    Feature {
        key: "screenshots",
        what: "Its new screenshots here",
        phone_switch: "Send new screenshots",
        permission_name: "Send files",
        android: "notification access and photo access",
    },
    Feature {
        key: "camera",
        what: "Photos asked from here",
        phone_switch: "Photos the computer asks for",
        permission_name: "Send files",
        android: "notification access and its notifications",
    },
    Feature {
        key: "links",
        what: "Links from here",
        phone_switch: "Links from the computer",
        permission_name: "Open its links here",
        android: "its notifications",
    },
    Feature {
        key: "ring",
        what: "Ringing it from here",
        phone_switch: "Ring from the computer",
        permission_name: "Ring this phone from here",
        android: "notification access and its notifications",
    },
    Feature {
        key: "media",
        what: "Its music here",
        phone_switch: "My media on the computer",
        permission_name: "Show and control its media here",
        android: "notification access",
    },
    Feature {
        key: "media_background",
        what: "Its music here without a session",
        phone_switch: "My media on the computer in the background",
        permission_name: "Show and control its media here",
        android: "notification access",
    },
    Feature {
        key: "computer_media",
        what: "This computer's music in its media controls",
        phone_switch: "Computer's music in my media controls",
        permission_name: "Hear and change this computer's sound, and its media",
        android: "notification access and its notifications",
    },
    Feature {
        key: "mute",
        what: "Muting it from here",
        phone_switch: "Let the computer mute this phone",
        permission_name: "Mute this phone from here",
        android: "notification access",
    },
    Feature {
        key: "control",
        what: "Controlling it from its screen here",
        phone_switch: "Let the computer control this phone",
        permission_name: "Show its screen or camera here",
        android: "the Pocketdesk control Accessibility service",
    },
    Feature {
        key: "dial",
        what: "Calling numbers from here",
        phone_switch: "Calls from the computer",
        permission_name: "Call numbers from here",
        android: "its notifications",
    },
    Feature {
        key: "commands",
        what: "Commands to its automation apps",
        phone_switch: "Commands from the computer to my automation apps",
        permission_name: "Send commands to its automation apps",
        android: "notification access",
    },
    Feature {
        key: "phone_actions",
        what: "Screenshots of it and locking it from here",
        phone_switch: "Let the computer take screenshots and lock this phone",
        permission_name: "Take screenshots of it and lock it from here",
        android: "the Pocketdesk control Accessibility service and notification access",
    },
];

/// Whether O2's screenshot and lock reach a phone now: [`linked`] to this
/// computer, with its Phone actions permission ([`allowed`]) on; else why
/// not, as the Overview and the tray say it.
pub fn phone_action_reach(linked: bool, allowed: bool) -> Result<(), &'static str> {
    if !allowed {
        Err("This phone's \"Take screenshots of it and lock it from here\" is off in Devices.")
    } else if !linked {
        Err("The phone is not linked now: a session, or one of its background features, links it.")
    } else {
        Ok(())
    }
}

/// One feature as both sides stand (L7): this computer's permission (and
/// its Settings, L7 review), the phone's switch and Android's grant as the
/// phone said them (`None`: not said, or Android needs nothing), and what
/// to change when the phone wants it and something holds it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchRow {
    pub what: &'static str,
    pub here: bool,
    pub phone: Option<bool>,
    pub android: Option<bool>,
    pub advice: Option<String>,
}

/// The features this computer's Settings turn off (L7 review), in core
/// since M2, where the phone's mirror reads it too.
pub use pocketdesk_core::local::phone_switches::settings_off;

/// The Devices page's rows for a phone: every feature, with advice for
/// each side that holds back one the phone has on, this computer's first.
/// One the phone has off is the owner's choice, not a fault (L7 review),
/// so it gets none.
pub fn switch_rows(
    permissions: &DevicePermissions,
    switches: Option<&pocketdesk_core::local::phone_switches::PhoneSwitches>,
    settings_off: &[(&'static str, &'static str)],
) -> Vec<SwitchRow> {
    FEATURES
        .iter()
        .map(|f| {
            let setting = settings_off
                .iter()
                .find(|(key, _)| *key == f.key)
                .map(|(_, label)| *label);
            let allowed = permissions.allows(f.permission());
            let here = allowed && setting.is_none();
            let said = switches.and_then(|s| s.switches.get(f.key));
            let phone = said.map(|s| s.on);
            let android = said.and_then(|s| s.android);
            let mut fixes = Vec::new();
            if phone != Some(false) {
                if !allowed {
                    fixes.push(format!(
                        "turn on \"{}\" in this phone's permissions above",
                        f.permission_name
                    ));
                }
                if let Some(label) = setting {
                    fixes.push(format!("turn on \"{label}\" in Settings"));
                }
                if android == Some(false) && !f.android.is_empty() {
                    fixes.push(format!("give Pocketdesk {} on the phone", f.android));
                }
            }
            let advice = (!fixes.is_empty()).then(|| {
                let mut words = fixes.join(", and ");
                words[..1].make_ascii_uppercase();
                words.push('.');
                words
            });
            SwitchRow {
                what: f.what,
                here,
                phone,
                android,
                advice,
            }
        })
        .collect()
}

/// How long the Overview says a phone said no to a mute.
pub const REFUSAL_SHOWN_SECS: u64 = 120;

/// Whether the phone said no to a mute in the last two minutes (D2 review):
/// its "Let the computer mute this phone" is off.
pub fn mute_refused(
    quiet: Option<&pocketdesk_core::local::phone_quiet::PhoneQuiet>,
    now_unix: u64,
) -> bool {
    quiet
        .and_then(|q| q.refused_unix)
        .is_some_and(|at| at <= now_unix && now_unix - at <= REFUSAL_SHOWN_SECS)
}

/// Whether the phone is in its owner's own Do Not Disturb (D2 review): the
/// computer does not mute it then, so Mute for an hour is not offered.
pub fn quiet_by_owner(quiet: Option<&pocketdesk_core::local::phone_quiet::PhoneQuiet>) -> bool {
    quiet.is_some_and(|q| q.quiet && !q.by_computer)
}

/// Whether the Overview offers Unmute rather than Mute for an hour: a mute
/// asked for from a computer, still running.
pub fn muted_from_here(
    quiet: Option<&pocketdesk_core::local::phone_quiet::PhoneQuiet>,
    now_unix: u64,
) -> bool {
    quiet.is_some_and(|q| q.quiet && q.by_computer && q.until_unix.is_some_and(|u| u > now_unix))
}

/// How much of the session log's end is read for [`last_sessions`]: its
/// newest records, so a long log costs no more.
pub const SESSION_LOG_TAIL: u64 = 256 * 1024;

/// Each phone's newest session in the session log's `lines` (N6): its
/// `unix_secs` by `device_id`. Lines that are not records, a partial first
/// line from a read of the tail among them, are skipped.
pub fn last_sessions(lines: &str) -> std::collections::HashMap<String, u64> {
    let mut last = std::collections::HashMap::new();
    for line in lines.lines() {
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        // A refused attempt is no session: its id is only what the
        // connecting client claimed (N6 review).
        if record.get("outcome").and_then(|v| v.as_str()) == Some("rejected") {
            continue;
        }
        let (Some(id), Some(at)) = (
            record.get("device_id").and_then(|v| v.as_str()),
            record.get("unix_secs").and_then(|v| v.as_u64()),
        ) else {
            continue;
        };
        let newest = last.entry(id.to_string()).or_insert(at);
        *newest = (*newest).max(at);
    }
    last
}

/// What a live session is, as its chip says (J3).
pub fn session_kind(session: &pocketdesk_core::local::sessions::MirrorSession) -> &'static str {
    if session.cast {
        "its screen or camera here"
    } else if session.phone_sound_only {
        "its sound here"
    } else if session.control_only {
        "control only"
    } else if session.sound_only {
        "sound only"
    } else {
        "screen"
    }
}

/// A session from the log, as the Sessions page lists it (J3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentSession {
    /// The phone it was with, as it said; `None` when it never said.
    pub device_id: Option<String>,
    pub ended_unix: u64,
    /// `None` when the record does not say (J3 review: it read "for 0s").
    pub duration_secs: Option<u64>,
    /// How it connected, or that it was refused or failed, in words.
    pub how: &'static str,
    pub refused: bool,
    /// How many refusals in a row this line stands for: a phone refused as
    /// it reconnects is refused every few seconds (J3 review).
    pub times: u32,
    /// The phone's median latency from capture to arrival, when it
    /// measured it.
    pub e2e_ms: Option<u64>,
}

/// Who a recent session was with, in words. A refused one is named only
/// by the id it claimed: that is all a refused client proved, and a
/// stranger may claim a paired phone's (J3 review; N6's last sessions skip
/// them for the same reason).
pub fn recent_who(recent: &RecentSession, devices: &[Device]) -> String {
    match recent.device_id.as_deref() {
        Some(id) if recent.refused => {
            format!("A device claiming to be {}", crate::words::short(id))
        }
        Some(id) => device_label(devices, id),
        None => "A device that did not say who it is".to_string(),
    }
}

/// The newest `n` sessions in the session log's `lines`, newest first
/// (J3). Lines that are not records, a cut first line among them, are
/// skipped.
pub fn recent_sessions(lines: &str, n: usize) -> Vec<RecentSession> {
    let mut all: Vec<RecentSession> = lines
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|record| {
            let outcome = record.get("outcome")?.as_str()?;
            Some(RecentSession {
                device_id: record
                    .get("device_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                ended_unix: record.get("unix_secs")?.as_u64()?,
                duration_secs: record.get("duration_secs").and_then(|v| v.as_u64()),
                how: match outcome {
                    "lan_direct" => "on this network",
                    "wan_punched" => "over the internet, direct",
                    "wan_relayed" => "over the internet, through a relay",
                    "rejected" => "refused",
                    "failed" => "failed",
                    _ => "connected",
                },
                refused: outcome == "rejected",
                times: 1,
                e2e_ms: record
                    .get("e2e_tail")
                    .and_then(|t| t.get("p50_ms"))
                    .and_then(|v| v.as_u64()),
            })
        })
        .collect();
    // The log is in order of ending already; sorted anyway, stably, from
    // its end, so two that ended in one second keep the later first (J3
    // review).
    all.reverse();
    all.sort_by_key(|s| std::cmp::Reverse(s.ended_unix));
    // Refusals of one device in a row are one line, so a phone retrying
    // does not push the real sessions out of the list.
    let mut lines: Vec<RecentSession> = Vec::new();
    for session in all {
        match lines.last_mut() {
            Some(last)
                if last.refused && session.refused && last.device_id == session.device_id =>
            {
                last.times += 1;
            }
            _ => lines.push(session),
        }
    }
    lines.truncate(n);
    lines
}

/// The session log's size and change time, so it is read again only when
/// it changes (N6 review); `None` when there is no log.
pub fn session_log_stamp(state_dir: &Path) -> Option<(u64, std::time::SystemTime)> {
    let meta =
        std::fs::metadata(state_dir.join(pocketdesk_core::local::paths::SESSIONS_FILE)).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

/// The end of the session log under `state_dir`, at most
/// [`SESSION_LOG_TAIL`] bytes; empty when there is none.
pub fn session_log_tail(state_dir: &Path) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) =
        std::fs::File::open(state_dir.join(pocketdesk_core::local::paths::SESSIONS_FILE))
    else {
        return String::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(len.saturating_sub(SESSION_LOG_TAIL)));
    let mut tail = Vec::new();
    let _ = file.take(SESSION_LOG_TAIL).read_to_end(&mut tail);
    String::from_utf8_lossy(&tail).into_owned()
}

/// The phones a file can be sent to: holding a control link files can
/// ride, a screen or sound session's or a background one (T10d).
pub fn send_targets(snapshot: &Snapshot) -> Vec<&Device> {
    snapshot.devices.iter().filter(|d| d.takes_files).collect()
}

/// Everything a file can be sent to now, by id: the phones files reach,
/// then the other computers whose link is up (2h).
pub fn send_target_ids(snapshot: &Snapshot) -> Vec<String> {
    send_targets(snapshot)
        .iter()
        .map(|d| d.id.clone())
        .chain(snapshot.linked_computers.iter().map(|(id, _)| id.clone()))
        .collect()
}

/// How a send target is named: a computer by its name, a phone by its
/// name (U3), else by the first characters of its id.
pub fn target_label(snapshot: &Snapshot, id: &str) -> String {
    match snapshot.linked_computers.iter().find(|(c, _)| c == id) {
        Some((_, name)) => format!("{name} (computer)"),
        None => device_label(&snapshot.devices, id),
    }
}

/// A paired device by its name (U3), else by the first characters of its
/// id: one label for every page. A name another device shares (two phones
/// of one model keep Android's default) gets its short id too, so a picker
/// never lists the same words twice (U3 review).
pub fn device_label(devices: &[Device], id: &str) -> String {
    let Some(name) = devices
        .iter()
        .find(|d| d.id == id)
        .and_then(|d| d.name.as_deref())
    else {
        return crate::words::short(id);
    };
    let shared = devices
        .iter()
        .filter(|d| d.name.as_deref() == Some(name))
        .count()
        > 1;
    if shared {
        format!("{name} · {}", crate::words::short(id))
    } else {
        name.to_string()
    }
}

/// Name the paired device `id` (U3); blank clears the owner's name, back
/// to the phone's own.
pub fn rename_device(state_dir: &Path, id: &str, name: &str) -> pocketdesk_core::error::Result<()> {
    TrustStore::update(state_dir.join(TRUST_FILE), |store| {
        store.rename(id, Some(name))
    })
}

/// The phone a send goes to: the one the owner chose, while it is still in
/// a session; with none chosen, the first phone that is. A chosen phone
/// that left is an error, never a quiet switch to another phone.
pub fn target_for_send(snapshot: &Snapshot, chosen: Option<&str>) -> Result<String, String> {
    let targets = send_target_ids(snapshot);
    match chosen {
        Some(id) if targets.iter().any(|t| t == id) => Ok(id.to_string()),
        Some(_) => Err(
            "The phone or computer you chose is no longer connected. Choose one, then send again."
                .to_string(),
        ),
        None => targets.first().cloned().ok_or_else(|| {
            "No phone or computer is connected where files can reach it.".to_string()
        }),
    }
}

/// Where a send goes when several are chosen (P6): those of `chosen` that
/// can take files now, in the page's order, and those that cannot any more,
/// to be said; none chosen is the first that can. An error when none of the
/// chosen can.
pub fn targets_for_send(
    snapshot: &Snapshot,
    chosen: &[String],
) -> Result<(Vec<String>, Vec<String>), String> {
    let targets = send_target_ids(snapshot);
    if chosen.is_empty() {
        return targets
            .first()
            .map(|first| (vec![first.clone()], Vec::new()))
            .ok_or_else(|| {
                "No phone or computer is connected where files can reach it.".to_string()
            });
    }
    let go: Vec<String> = targets
        .iter()
        .filter(|t| chosen.contains(t))
        .cloned()
        .collect();
    let mut gone: Vec<String> = Vec::new();
    for id in chosen {
        if !targets.contains(id) && !gone.contains(id) {
            gone.push(id.clone());
        }
    }
    if go.is_empty() {
        return Err(
            "The phones or computers you chose are no longer connected. Choose again, then send."
                .to_string(),
        );
    }
    Ok((go, gone))
}

/// "Ann's phone", "Ann's phone and Desk (computer)", "A, B and C".
pub fn targets_words(snapshot: &Snapshot, ids: &[String]) -> String {
    let labels: Vec<String> = ids.iter().map(|id| target_label(snapshot, id)).collect();
    match labels.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Where files dropped on the window go (J5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropTo {
    /// The one phone or computer that can take them.
    Send(String),
    /// Several can: ask which, among these.
    Ask(Vec<String>),
    /// None can now.
    Nobody,
}

/// Where a drop goes: straight to the only target, a question with
/// several, and nowhere with none (J5: a drop had gone to the Files page's
/// last choice without a word).
pub fn drop_to(snapshot: &Snapshot) -> DropTo {
    let mut targets = send_target_ids(snapshot);
    match targets.len() {
        0 => DropTo::Nobody,
        1 => DropTo::Send(targets.remove(0)),
        _ => DropTo::Ask(targets),
    }
}

/// "1 file", "3 files", "1 folder", "2 files and 1 folder".
pub fn files_and_folders(files: usize, folders: usize) -> String {
    let count = |n: usize, one: &str, many: &str| match n {
        1 => format!("1 {one}"),
        n => format!("{n} {many}"),
    };
    match (files, folders) {
        (files, 0) => count(files, "file", "files"),
        (0, folders) => count(folders, "folder", "folders"),
        (files, folders) => format!(
            "{} and {}",
            count(files, "file", "files"),
            count(folders, "folder", "folders")
        ),
    }
}

/// Files dropped while a question waits join it, each path once (J5
/// review: the same file dropped twice went twice).
pub fn join_drops(waiting: &mut Vec<std::path::PathBuf>, dropped: Vec<std::path::PathBuf>) {
    for path in dropped {
        if !waiting.contains(&path) {
            waiting.push(path);
        }
    }
}

/// Queue the link `url` for the phone `device_id` (T8a), which shows it as
/// a notification that opens it; what to tell the owner.
pub fn queue_link(
    state_dir: &Path,
    device_id: &str,
    url: &str,
    now_unix: u64,
) -> Result<String, String> {
    let url = url.trim();
    // The server would drop it: say so now.
    let allowed = TrustStore::read(state_dir.join(TRUST_FILE))
        .map(|store| store.permissions(device_id).allows(Capability::Links))
        .unwrap_or(true);
    if !allowed {
        return Err("Not sent: Links is off for this phone (Devices tab).".to_string());
    }
    pocketdesk_core::local::links::request(state_dir, device_id, url, now_unix)
        .map(|_| "Link sent: it waits on the phone as a notification.".to_string())
        .map_err(|e| format!("Not sent: {e}."))
}

/// Ask the server to send the file at `path` to `device_id`; the file's
/// name, or why not.
/// Ask the server to send `command` to `device_id`'s player (T8d), or why
/// not.
pub fn press_phone_media(
    state_dir: &Path,
    device_id: &str,
    command: &str,
    now_unix: u64,
) -> Result<(), String> {
    let allowed = TrustStore::read(state_dir.join(TRUST_FILE))
        .map(|store| store.permissions(device_id).allows(Capability::Media))
        .unwrap_or(true);
    if !allowed {
        return Err("Not sent: Media is off for this phone (Devices tab).".to_string());
    }
    pocketdesk_core::local::phone_media::press(state_dir, device_id, command, now_unix)
        .map(|_| ())
        .map_err(|e| format!("Not sent: {e}."))
}

pub fn queue_send(
    state_dir: &Path,
    device_id: &str,
    path: &Path,
    now_unix: u64,
) -> Result<String, String> {
    // A file, or a folder the server sends file by file (T6b).
    if !path.is_file() && !path.is_dir() {
        return Err(format!("{} is not a file or a folder.", path.display()));
    }
    // The server resolves the path, from its own working folder: name the
    // file as this app sees it.
    let path = &std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    pocketdesk_core::local::outbox::request(
        state_dir,
        device_id,
        &path.display().to_string(),
        now_unix,
    )
    .map_err(|e| format!("Could not queue {name}: {e}"))?;
    Ok(name)
}

/// Allow or deny one capability for `id`, in the store the server reads.
pub fn set_permission(
    state_dir: &Path,
    id: &str,
    capability: Capability,
    allowed: bool,
) -> pocketdesk_core::error::Result<()> {
    update_permissions(state_dir, id, |p| p.set(capability, allowed))
}

/// Give `id` a profile's permissions in one write (P9 review: a switch at a
/// time could stop half way): those its page shows, and `ask_first`; the
/// others as they are, and AwayLock only where `away_lock` may be set.
/// Whether a permission fixed at a session's start was turned off, so its
/// session must end.
pub fn apply_profile(
    state_dir: &Path,
    id: &str,
    profile: &DevicePermissions,
    shown: impl Fn(Capability) -> bool,
) -> pocketdesk_core::error::Result<bool> {
    let mut narrowed = false;
    update_permissions(state_dir, id, |p| {
        for capability in Capability::ALL {
            if !shown(capability) {
                continue;
            }
            let allowed = profile.allows(capability);
            if p.allows(capability) && !allowed && fixed_at_session_start(capability) {
                narrowed = true;
            }
            p.set(capability, allowed);
        }
        p.ask_first = profile.ask_first;
    })?;
    Ok(narrowed)
}

/// Whether a running session keeps its own copy of this permission, taken at
/// its start; the control channel re-reads the others for every packet.
pub fn fixed_at_session_start(capability: Capability) -> bool {
    matches!(
        capability,
        Capability::Input | Capability::Audio | Capability::Mic | Capability::PhoneSound
    )
}

/// Hold each of `id`'s sessions for approval at this desktop, or not.
pub fn set_ask_first(
    state_dir: &Path,
    id: &str,
    ask_first: bool,
) -> pocketdesk_core::error::Result<()> {
    update_permissions(state_dir, id, |p| p.ask_first = ask_first)
}

/// The owner's answer to a held session.
pub fn answer_approval(
    state_dir: &Path,
    id: &str,
    verdict: Verdict,
    now_unix: u64,
) -> std::io::Result<()> {
    approval::answer(state_dir, id, verdict, now_unix)
}

// Load, change, save under the store's lock: the server writes the same file
// (pairing, grants, "Always allow"), so the copy read at the last refresh may
// be stale, and two writers must not lose each other's change.
fn update_permissions(
    state_dir: &Path,
    id: &str,
    change: impl FnOnce(&mut DevicePermissions),
) -> pocketdesk_core::error::Result<()> {
    TrustStore::update(state_dir.join(TRUST_FILE), |store| {
        let mut permissions = store.permissions(id);
        change(&mut permissions);
        store.set_permissions(id, permissions)
    })
}

/// The newest daily server log (`<state>/logs/pocketdesk.<date>.log`,
/// written by `start --background`), if there is one.
pub fn newest_log(state_dir: &Path) -> Option<std::path::PathBuf> {
    std::fs::read_dir(state_dir.join(LOG_DIR))
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("pocketdesk.") && n.ends_with(".log"))
        })
        .max()
}

/// What the newest log is now: its path, size and modification time. The
/// Logs page reads it again only when this changes (J1: it had read the
/// whole file every second).
pub fn log_stamp(state_dir: &Path) -> Option<(std::path::PathBuf, u64, std::time::SystemTime)> {
    let path = newest_log(state_dir)?;
    let meta = std::fs::metadata(&path).ok()?;
    Some((path, meta.len(), meta.modified().ok()?))
}

/// How much of a log's end is read: a day's debug log runs to megabytes.
const LOG_TAIL_BYTES: u64 = 256 * 1024;

/// The last `lines` lines of the newest daily server log, read from at most
/// its last 256 KiB; a line the read cut in two is left out.
pub fn log_tail(state_dir: &Path, lines: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Some(mut file) = newest_log(state_dir).and_then(|p| std::fs::File::open(p).ok()) else {
        return Vec::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    // One byte early, so the first piece read is cut only if the line it
    // ends was: read on a line's first byte, it is the empty piece before it.
    let start = len.saturating_sub(LOG_TAIL_BYTES).saturating_sub(1);
    let mut raw = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_to_end(&mut raw).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&raw);
    let mut all: Vec<&str> = text.lines().collect();
    if start > 0 && !all.is_empty() {
        all.remove(0);
    }
    all[all.len().saturating_sub(lines)..]
        .iter()
        .map(|l| l.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_pages_features_are_the_phones() {
        let ours: Vec<&str> = super::FEATURES.iter().map(|f| f.key).collect();
        assert_eq!(ours, pocketdesk_core::local::phone_switches::FEATURES);
    }

    /// O2: a screenshot and a lock reach a linked phone with its permission
    /// on; the permission is said first, since linking it would not help.
    #[test]
    fn a_phone_action_reaches_a_linked_phone_it_may_ask() {
        assert_eq!(super::phone_action_reach(true, true), Ok(()));
        assert!(
            super::phone_action_reach(true, false)
                .unwrap_err()
                .contains("Devices")
        );
        assert!(
            super::phone_action_reach(false, false)
                .unwrap_err()
                .contains("Devices")
        );
        assert!(
            super::phone_action_reach(false, true)
                .unwrap_err()
                .contains("not linked")
        );
    }

    #[test]
    fn each_feature_names_the_side_that_holds_it_back() {
        use pocketdesk_core::local::phone_switches::{PhoneSwitches, from_body};
        use pocketdesk_core::protocol::permissions::{Capability, DevicePermissions};
        let mut permissions = DevicePermissions::default();
        permissions.set(Capability::Links, false);
        let said = PhoneSwitches {
            switches: from_body(&serde_json::json!({"switches": {
                "notifications": {"on": true, "android": false},
                "links": {"on": false},
                "clipboard": {"on": true},
            }}))
            .unwrap(),
            updated_unix: 1,
        };
        let rows = super::switch_rows(&permissions, Some(&said), &[]);
        let row = |what: &str| rows.iter().find(|r| r.what == what).unwrap().clone();
        assert_eq!(
            row("Its notifications here").advice.as_deref(),
            Some("Give Pocketdesk notification access on the phone.")
        );
        assert_eq!(
            row("Links from here").advice,
            None,
            "off on the phone is the owner's choice, not a fault"
        );
        let clipboard = row("The clipboard without a session");
        assert_eq!(clipboard.advice, None, "both on, Android not said");
        assert_eq!(
            (clipboard.here, clipboard.phone, clipboard.android),
            (true, Some(true), None)
        );
        // A feature the phone did not mention: nothing said of its side.
        assert_eq!(row("Its texts here").phone, None);
        assert_eq!(row("Its texts here").advice, None);
        // No report yet: only this computer's side.
        let none = super::switch_rows(&permissions, None, &[]);
        assert!(none.iter().all(|r| r.phone.is_none()));
        assert_eq!(
            none.iter()
                .find(|r| r.what == "Links from here")
                .unwrap()
                .advice
                .as_deref(),
            Some("Turn on \"Open its links here\" in this phone's permissions above.")
        );
        // A computer-wide setting holds it back too (L7 review).
        let config = pocketdesk_core::local::config::Config {
            phone_notifications: false,
            ..Default::default()
        };
        let off = super::settings_off(&config);
        let rows = super::switch_rows(&DevicePermissions::default(), Some(&said), &off);
        let notifications = rows
            .iter()
            .find(|r| r.what == "Its notifications here")
            .unwrap();
        assert!(!notifications.here);
        assert_eq!(
            notifications.advice.as_deref(),
            Some(
                "Turn on \"Phone notifications\" in Settings, and give Pocketdesk notification access on the phone."
            )
        );
        // A phone as it comes: every feature off but links, no fault shown.
        let default_phone = PhoneSwitches {
            switches: pocketdesk_core::local::phone_switches::FEATURES
                .iter()
                .map(|k| {
                    (
                        k.to_string(),
                        pocketdesk_core::local::phone_switches::Switch {
                            on: *k == "links",
                            android: Some(true),
                        },
                    )
                })
                .collect(),
            updated_unix: 1,
        };
        assert!(
            super::switch_rows(&DevicePermissions::default(), Some(&default_phone), &[])
                .iter()
                .all(|r| r.advice.is_none())
        );
    }

    #[test]
    fn a_phones_state_is_shown_only_while_it_is_reached_and_recent() {
        use super::{Place, STATE_FRESH_SECS, state_is_now};
        let battery = pocketdesk_core::local::phone_status::PhoneBattery {
            updated_unix: 1000,
            ..Default::default()
        };
        assert!(state_is_now(
            &battery,
            Place::Linked,
            1000 + STATE_FRESH_SECS
        ));
        assert!(state_is_now(&battery, Place::Session, 1000));
        assert!(!state_is_now(&battery, Place::Away, 1000), "away");
        assert!(
            !state_is_now(&battery, Place::Linked, 1001 + STATE_FRESH_SECS),
            "stopped reporting"
        );
    }

    #[test]
    fn a_phones_state_reads_as_one_line_of_what_it_said() {
        use pocketdesk_core::local::phone_status::{Network, PhoneState, Ringer};
        assert_eq!(super::phone_state_text(&PhoneState::default()), None);
        let full = PhoneState {
            storage_free_mb: Some(12_300),
            storage_total_mb: Some(128_000),
            network: Some(Network::Wifi),
            signal: Some(3),
            ringer: Some(Ringer::Silent),
        };
        assert_eq!(
            super::phone_state_text(&full).as_deref(),
            Some("Wi-Fi, 3 of 4 bars · 12.3 GB free of 128 GB · on silent")
        );
        let mobile = PhoneState {
            network: Some(Network::Mobile),
            ringer: Some(Ringer::Vibrate),
            ..Default::default()
        };
        assert_eq!(
            super::phone_state_text(&mobile).as_deref(),
            Some("Mobile data · on vibrate")
        );
        let bare = PhoneState {
            network: Some(Network::None),
            storage_free_mb: Some(500),
            ringer: Some(Ringer::Normal),
            ..Default::default()
        };
        assert_eq!(
            super::phone_state_text(&bare).as_deref(),
            Some("No network · 500 MB free · ringer on")
        );
        // Nearly full reads as such, not "0 B" (L9 review).
        let full_up = PhoneState {
            storage_free_mb: Some(0),
            storage_total_mb: Some(64_000),
            network: Some(Network::Mobile),
            signal: Some(2),
            ..Default::default()
        };
        assert_eq!(
            super::phone_state_text(&full_up).as_deref(),
            Some("Mobile data, 2 of 4 bars · under 100 MB free of 64.0 GB")
        );
        let wired = PhoneState {
            network: Some(Network::Ethernet),
            storage_total_mb: Some(256_000),
            ..Default::default()
        };
        assert_eq!(super::phone_state_text(&wired).as_deref(), Some("Ethernet"));
    }

    use super::*;

    fn under_way(size: u64, done: u64, rate: u64) -> pocketdesk_core::local::transfers::Transfer {
        pocketdesk_core::local::transfers::Transfer {
            id: "t1".to_string(),
            direction: pocketdesk_core::local::transfers::Direction::ToPhone,
            device_id: "phone".to_string(),
            name: "beach.jpg".to_string(),
            folder: None,
            size,
            done,
            rate,
            started_unix: 1,
        }
    }

    #[test]
    fn a_file_under_way_says_how_far_and_how_fast() {
        assert_eq!(transfer_words(&under_way(1_000, 420, 0)), "42 %");
        assert_eq!(
            transfer_words(&under_way(10_000_000, 5_000_000, 3_250_000)),
            "50 % · 3.1 MB/s"
        );
        assert_eq!(
            transfer_words(&under_way(0, 2048, 0)),
            "2.0 KB",
            "no size known: the bytes so far"
        );
        assert_eq!(transfer_fraction(&under_way(1_000, 250, 0)), 0.25);
        assert_eq!(transfer_fraction(&under_way(1_000, 5_000, 0)), 1.0);
        assert_eq!(transfer_fraction(&under_way(0, 5, 0)), 0.0);
    }

    #[test]
    fn a_file_of_a_folder_is_named_with_its_folder() {
        let mut transfer = under_way(1, 0, 0);
        assert_eq!(transfer_title(&transfer), "beach.jpg");
        transfer.folder = Some("Holiday".to_string());
        assert_eq!(transfer_title(&transfer), "Holiday/beach.jpg");
    }
    use pocketdesk_core::local::paths::{LIVE_FILE, LOG_DIR, TRUST_FILE};
    use pocketdesk_core::protocol::kdeconnect::TrustStore;
    use std::time::Duration;

    pub(crate) fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-model-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn phone(id: &str) -> Device {
        Device {
            id: id.to_string(),
            ..Default::default()
        }
    }

    /// N6: the newest record per phone wins; other lines are skipped.
    #[test]
    fn each_phone_s_last_session_is_read_from_the_log() {
        let log = concat!(
            r#"{"unix_secs":100,"outcome":"ended","device_id":"a"}"#,
            "\n",
            "not json\n",
            r#"{"unix_secs":300,"outcome":"ended","device_id":"b"}"#,
            "\n",
            r#"{"unix_secs":200,"outcome":"completed","device_id":"a"}"#,
            "
",
            // A refused attempt is no session (N6 review): its id is only
            // what the connecting client claimed.
            r#"{"unix_secs":250,"outcome":"rejected","device_id":"a"}"#,
            "\n",
            r#"{"unix_secs":400,"outcome":"ended"}"#,
            "\n",
            // A partial first line, as a read from the file's tail leaves.
        );
        let last = last_sessions(&format!("_secs\":9}}\n{log}"));
        assert_eq!(last.get("a"), Some(&200));
        assert_eq!(last.get("b"), Some(&300));
        assert_eq!(last.len(), 2);
    }

    /// J3 review: a refused session is never named as the paired phone
    /// whose id it claimed.
    #[test]
    fn a_refused_session_is_named_only_by_its_claim() {
        let devices = [Device {
            id: "0123456789abcdef".to_string(),
            name: Some("Test Phone".to_string()),
            ..Default::default()
        }];
        let recent = |refused, id: Option<&str>| RecentSession {
            device_id: id.map(str::to_string),
            ended_unix: 1,
            duration_secs: Some(1),
            how: "on this network",
            refused,
            times: 1,
            e2e_ms: None,
        };
        assert_eq!(
            recent_who(&recent(false, Some("0123456789abcdef")), &devices),
            "Test Phone"
        );
        assert_eq!(
            recent_who(&recent(true, Some("0123456789abcdef")), &devices),
            "A device claiming to be 0123456789ab…"
        );
        assert_eq!(
            recent_who(&recent(false, None), &devices),
            "A device that did not say who it is"
        );
    }

    /// J3: a live session's kind chip.
    #[test]
    fn a_live_sessions_kind_in_words() {
        let session =
            |cast, control_only, sound_only| pocketdesk_core::local::sessions::MirrorSession {
                cast,
                control_only,
                sound_only,
                phone_sound_only: false,
                id: 1,
                device_id: None,
                peer: String::new(),
                transport: "lan".to_string(),
                started_unix: 0,
                driving: false,
            };
        assert_eq!(session_kind(&session(false, false, false)), "screen");
        assert_eq!(
            session_kind(&session(true, false, false)),
            "its screen or camera here"
        );
        assert_eq!(session_kind(&session(false, true, false)), "control only");
        assert_eq!(session_kind(&session(false, false, true)), "sound only");
        let alone = pocketdesk_core::local::sessions::MirrorSession {
            phone_sound_only: true,
            ..session(false, false, false)
        };
        assert_eq!(session_kind(&alone), "its sound here");
    }

    /// J3: the Sessions page's recent sessions, newest first, in words.
    #[test]
    fn recent_sessions_are_the_logs_newest_in_words() {
        let log = concat!(
            r#"{"unix_secs":100,"outcome":"lan_direct","duration_secs":60,"device_id":"a","e2e_tail":{"count":9,"mean_ms":40,"p50_ms":38,"p90_ms":50,"p95_ms":55,"p99_ms":70,"max_ms":90}}"#,
            "\n",
            "not json\n",
            r#"{"unix_secs":300,"outcome":"wan_relayed","duration_secs":5,"device_id":"b","e2e_tail":null}"#,
            "\n",
            r#"{"unix_secs":200,"outcome":"rejected","duration_secs":0,"device_id":"c"}"#,
            "\n",
            r#"{"unix_secs":400,"outcome":"wan_punched","duration_secs":7}"#,
            "\n",
            r#"{"unix_secs":500,"outcome":"failed","duration_secs":1,"device_id":"a"}"#,
            "\n",
            r#"{"unix_secs":600,"outcome":"completed","duration_secs":2,"device_id":"a"}"#,
            "\n",
        );
        // A partial first line, as a read from the file's tail leaves.
        let all = recent_sessions(&format!("_secs\":9}}\n{log}"), 10);
        let ended: Vec<u64> = all.iter().map(|s| s.ended_unix).collect();
        assert_eq!(ended, [600, 500, 400, 300, 200, 100], "newest first");
        let how: Vec<&str> = all.iter().map(|s| s.how).collect();
        assert_eq!(
            how,
            [
                "connected",
                "failed",
                "over the internet, direct",
                "over the internet, through a relay",
                "refused",
                "on this network",
            ]
        );
        assert!(all[4].refused);
        assert!(!all[5].refused);
        assert_eq!(all[5].device_id.as_deref(), Some("a"));
        assert_eq!(all[5].duration_secs, Some(60));
        assert_eq!(
            recent_sessions(r#"{"unix_secs":1,"outcome":"failed"}"#, 1)[0].duration_secs,
            None,
            "a record without one"
        );
        assert_eq!(all[5].e2e_ms, Some(38), "the median, when measured");
        assert_eq!(all[3].e2e_ms, None);
        assert_eq!(all[2].device_id, None, "a session with no id");
        // At most N, the newest.
        let two = recent_sessions(log, 2);
        assert_eq!(two.len(), 2);
        assert_eq!(two[0].ended_unix, 600);
        assert!(recent_sessions("", 10).is_empty());
    }

    /// J3 review: two sessions that ended in one second, the later first.
    #[test]
    fn sessions_ending_in_one_second_keep_their_order() {
        let log = concat!(
            r#"{"unix_secs":100,"outcome":"lan_direct","duration_secs":1,"device_id":"first"}"#,
            "
",
            r#"{"unix_secs":100,"outcome":"lan_direct","duration_secs":1,"device_id":"second"}"#,
        );
        let both = recent_sessions(log, 10);
        assert_eq!(both[0].device_id.as_deref(), Some("second"));
        assert_eq!(both[1].device_id.as_deref(), Some("first"));
    }

    /// J3 review: a device refused again and again is one line, and the
    /// sessions before it stay in the list.
    #[test]
    fn refusals_in_a_row_are_one_line() {
        let mut log = String::from(
            r#"{"unix_secs":100,"outcome":"lan_direct","duration_secs":60,"device_id":"a"}"#,
        );
        log.push('\n');
        for at in 200..206 {
            log.push_str(&format!(
                r#"{{"unix_secs":{at},"outcome":"rejected","duration_secs":0,"device_id":"b"}}"#
            ));
            log.push('\n');
        }
        let two = recent_sessions(&log, 2);
        assert_eq!(two.len(), 2);
        assert!(two[0].refused);
        assert_eq!(two[0].times, 6);
        assert_eq!(two[0].ended_unix, 205, "the newest refusal's time");
        assert_eq!(two[1].device_id.as_deref(), Some("a"), "still listed");
    }

    #[test]
    fn the_overview_lists_phones_with_where_they_are() {
        let mut snapshot = Snapshot::default();
        let mut in_session = phone("s");
        in_session.connected = true;
        in_session.linked = true;
        let mut linked = phone("l");
        linked.linked = true;
        let mut computer = phone("c");
        computer.computer = true;
        snapshot.devices = vec![in_session, linked, phone("away"), computer];
        let last = std::collections::HashMap::from([("away".to_string(), 50)]);
        let rows = phone_rows(&snapshot, &last);
        let places: Vec<_> = rows
            .iter()
            .map(|r| (r.device.id.as_str(), r.place))
            .collect();
        assert_eq!(
            places,
            [
                ("s", Place::Session),
                ("l", Place::Linked),
                ("away", Place::Away)
            ],
            "phones only, computers have their own section"
        );
        assert_eq!(rows[2].last_session, Some(50));
        assert_eq!(rows[0].last_session, None);
    }

    /// What a phone plays, from its background reporter or not (N2).
    fn playing(background: bool, at: u64) -> pocketdesk_core::local::phone_media::PhoneMediaState {
        pocketdesk_core::local::phone_media::PhoneMediaState {
            media: pocketdesk_core::protocol::phone_media::PhoneMedia {
                title: "Song".to_string(),
                app: "Music".to_string(),
                is_playing: true,
                can_play_pause: true,
                can_next: true,
                background,
                ..Default::default()
            },
            updated_unix: at,
            art: None,
        }
    }

    /// N2: a linked phone's music shows with its background reporter on,
    /// a phone in a session's as before, and none away or stale.
    #[test]
    fn a_phone_s_music_shows_in_a_session_or_linked_from_the_background() {
        let mut snapshot = Snapshot {
            now_unix: 1_000,
            ..Default::default()
        };
        let mut session = phone("session");
        session.connected = true;
        session.linked = true;
        let mut linked = phone("linked");
        linked.linked = true;
        let mut in_app = phone("in-app");
        in_app.linked = true;
        let away = phone("away");
        let mut stale = phone("stale");
        stale.linked = true;
        snapshot.devices = vec![session, linked, in_app, away, stale];
        snapshot
            .phone_media
            .insert("session".to_string(), playing(false, 990));
        snapshot
            .phone_media
            .insert("linked".to_string(), playing(true, 990));
        snapshot
            .phone_media
            .insert("in-app".to_string(), playing(false, 990));
        snapshot
            .phone_media
            .insert("away".to_string(), playing(true, 990));
        snapshot
            .phone_media
            .insert("stale".to_string(), playing(true, 10));
        let rows = phone_rows(&snapshot, &Default::default());
        let shown: Vec<bool> = rows.iter().map(|r| r.playing.is_some()).collect();
        assert_eq!(shown, [true, true, false, false, false]);
    }

    #[test]
    fn each_action_says_why_it_cannot_reach_a_phone() {
        let mut snapshot = Snapshot::default();
        let mut ready = phone("ready");
        ready.linked = true;
        ready.takes_files = true;
        ready.permissions.set(Capability::Ring, true);
        ready.permissions.set(Capability::Links, true);
        ready.permissions.set(Capability::Files, true);
        let mut off = ready.clone();
        off.id = "off".to_string();
        off.permissions.set(Capability::Ring, false);
        off.permissions.set(Capability::Links, false);
        off.permissions.set(Capability::Files, false);
        off.permissions.set(Capability::Mute, false);
        let mut away = phone("away");
        away.permissions = ready.permissions.clone();
        snapshot.devices = vec![ready, off, away];
        let rows = phone_rows(&snapshot, &Default::default());
        assert!(rows[0].ring.is_ok() && rows[0].file.is_ok() && rows[0].link.is_ok());
        assert!(rows[1].ring.unwrap_err().contains("Devices"));
        assert!(rows[1].file.unwrap_err().contains("Devices"));
        assert!(rows[1].link.unwrap_err().contains("Devices"));
        assert!(rows[2].ring.unwrap_err().contains("Not linked"));
        assert!(
            rows[2]
                .file
                .unwrap_err()
                .contains("in a screen or sound session")
        );
        assert!(rows[2].link.unwrap_err().contains("Not linked"));
        assert!(rows[0].mute.is_ok());
        assert!(rows[1].mute.unwrap_err().contains("Devices"));
        assert!(rows[2].mute.unwrap_err().contains("Not linked"));
    }

    /// D2: a phone's Do Not Disturb, and whether Unmute is offered.
    #[test]
    fn a_phone_s_quiet_is_said_and_a_mute_from_here_counts_down() {
        use pocketdesk_core::local::phone_quiet::PhoneQuiet;
        let own = PhoneQuiet {
            quiet: true,
            until_unix: Some(1_000 + 25 * 60 - 30),
            by_computer: true,
            updated_unix: 1_000,
            refused_unix: None,
        };
        assert_eq!(
            quiet_text(Some(&own), 1_000).as_deref(),
            Some("muted from here, 25 min left")
        );
        assert!(muted_from_here(Some(&own), 1_000));
        // Its end passed (the phone not heard since): the phone's word stands.
        assert_eq!(
            quiet_text(Some(&own), 3_000).as_deref(),
            Some("Do Not Disturb")
        );
        assert!(!muted_from_here(Some(&own), 3_000));
        let owners = PhoneQuiet {
            by_computer: false,
            until_unix: None,
            ..own
        };
        assert_eq!(
            quiet_text(Some(&owners), 1_000).as_deref(),
            Some("Do Not Disturb")
        );
        assert!(!muted_from_here(Some(&owners), 1_000));
        assert!(quiet_by_owner(Some(&owners)));
        let said_no = PhoneQuiet {
            refused_unix: Some(1_000),
            ..owners
        };
        assert!(mute_refused(Some(&said_no), 1_000 + REFUSAL_SHOWN_SECS));
        assert!(!mute_refused(Some(&said_no), 1_001 + REFUSAL_SHOWN_SECS));
        assert!(!mute_refused(Some(&owners), 1_000));
        assert!(!quiet_by_owner(Some(&own)));
        let loud = PhoneQuiet {
            quiet: false,
            ..owners
        };
        assert_eq!(quiet_text(Some(&loud), 1_000), None);
        assert_eq!(quiet_text(None, 1_000), None);
    }

    /// N6 review: the server pushes a link only over a control link.
    #[test]
    fn a_link_needs_the_phone_s_control_link_even_in_a_session() {
        let mut snapshot = Snapshot::default();
        let mut session_only = phone("s");
        session_only.connected = true;
        session_only.permissions.set(Capability::Links, true);
        snapshot.devices = vec![session_only];
        let rows = phone_rows(&snapshot, &Default::default());
        assert!(rows[0].link.unwrap_err().contains("Not linked"));
    }

    const ONE_SESSION: &str = r#"{"updated_unix":100,"sessions":[{"device_id":"a","peer":"1.2.3.4:5","transport":"lan","started_unix":90}]}"#;
    /// [`ONE_SESSION`] with the control link a screen session keeps.
    const ONE_SESSION_LINKED: &str = r#"{"updated_unix":100,"sessions":[{"device_id":"a","peer":"1.2.3.4:5","transport":"lan","started_unix":90}],"linked":[{"device_id":"a","files":true}]}"#;

    #[test]
    fn a_dead_server_shows_no_sessions_even_with_a_mirror_left_behind() {
        let dir = temp_dir("dead");
        std::fs::write(dir.join(LIVE_FILE), ONE_SESSION).unwrap();
        let snap = snapshot_with(&dir, 101, false);
        assert!(!snap.running && snap.sessions.is_empty());
    }

    #[test]
    fn a_stale_mirror_shows_no_sessions_even_while_running() {
        let dir = temp_dir("stale");
        std::fs::write(dir.join(LIVE_FILE), ONE_SESSION).unwrap();
        assert!(snapshot_with(&dir, 200, true).sessions.is_empty());
        assert_eq!(snapshot_with(&dir, 101, true).sessions.len(), 1);
    }

    /// N1: a phone is linked, so a ring reaches it, while it holds a
    /// control link (a session's or the background one).
    #[test]
    fn a_phone_with_a_control_link_is_linked() {
        let dir = temp_dir("linked");
        let mut store = TrustStore::load_or_create(dir.join(TRUST_FILE)).unwrap();
        for id in ["a", "b", "c"] {
            store.trust_device(id).unwrap();
        }
        std::fs::write(
            dir.join(LIVE_FILE),
            r#"{"updated_unix":100,"sessions":[{"device_id":"a","peer":"192.0.2.7:5","transport":"lan","started_unix":90}],"linked":[{"device_id":"b","files":false}]}"#,
        )
        .unwrap();
        let snap = snapshot_with(&dir, 101, true);
        let linked = |id: &str| snap.devices.iter().find(|d| d.id == id).unwrap().linked;
        assert!(!linked("a"), "a session's video alone carries no ring");
        assert!(linked("b"), "a control link");
        assert!(!linked("c"));
    }

    #[test]
    fn devices_list_permanent_and_granted_and_mark_the_connected_one() {
        let dir = temp_dir("devices");
        let mut store = TrustStore::load_or_create(dir.join(TRUST_FILE)).unwrap();
        store.trust_device("a").unwrap();
        store
            .trust_device_with_ttl("b", Duration::from_secs(60))
            .unwrap();
        std::fs::write(dir.join(LIVE_FILE), ONE_SESSION).unwrap();
        let snap = snapshot_with(&dir, 101, true);
        let a = snap.devices.iter().find(|d| d.id == "a").unwrap();
        let b = snap.devices.iter().find(|d| d.id == "b").unwrap();
        assert!(a.connected && a.grant_remaining.is_none());
        assert!(!b.connected && b.grant_remaining.is_some_and(|s| s <= 60));
    }

    #[test]
    fn a_permission_set_here_shows_in_the_next_snapshot_and_reaches_the_store() {
        use pocketdesk_core::protocol::permissions::Capability;
        let dir = temp_dir("perms");
        TrustStore::load_or_create(dir.join(TRUST_FILE))
            .unwrap()
            .trust_device("a")
            .unwrap();
        let before = snapshot_with(&dir, 101, false);
        assert!(
            !before.devices[0].permissions.allows(Capability::Exec),
            "exec starts off"
        );

        set_permission(&dir, "a", Capability::Exec, true).unwrap();
        set_permission(&dir, "a", Capability::Clipboard, false).unwrap();
        let after = snapshot_with(&dir, 102, false);
        let a = &after.devices[0].permissions;
        assert!(a.allows(Capability::Exec) && !a.allows(Capability::Clipboard));
        assert!(
            TrustStore::load_or_create(dir.join(TRUST_FILE))
                .unwrap()
                .permissions("a")
                .allows(Capability::Exec),
            "the server reads the same file"
        );
    }

    #[test]
    fn held_sessions_show_while_the_server_runs_and_an_answer_reaches_it() {
        let dir = temp_dir("approvals");
        let request = ApprovalRequest {
            id: "r1".to_string(),
            device_id: "a".to_string(),
            peer: "192.0.2.7:51000".to_string(),
            asked_unix: 100,
        };
        approval::write_pending(&dir, std::slice::from_ref(&request)).unwrap();
        assert_eq!(snapshot_with(&dir, 101, true).approvals, [request]);
        assert!(
            snapshot_with(&dir, 101, false).approvals.is_empty(),
            "server gone"
        );
        assert!(
            snapshot_with(&dir, 131, true).approvals.is_empty(),
            "timed out"
        );

        answer_approval(&dir, "r1", Verdict::AllowOnce, 102).unwrap();
        assert_eq!(
            approval::take_answer(&dir, "r1", 102),
            Some(Verdict::AllowOnce)
        );
    }

    #[test]
    fn only_streamed_permissions_are_fixed_for_a_running_session() {
        let fixed: Vec<_> = Capability::ALL
            .into_iter()
            .filter(|c| fixed_at_session_start(*c))
            .collect();
        assert_eq!(
            fixed,
            [
                Capability::Input,
                Capability::Audio,
                Capability::Mic,
                Capability::PhoneSound
            ]
        );
    }

    #[test]
    fn received_files_are_listed_newest_first() {
        let dir = temp_dir("received");
        for n in 1..=3u64 {
            received::record(
                &dir,
                ReceivedFile {
                    name: format!("f{n}"),
                    path: format!("/d/f{n}"),
                    size: n,
                    received_unix: n,
                    from: "phone".into(),
                },
            )
            .unwrap();
        }
        let names: Vec<_> = snapshot_with(&dir, 10, false)
            .received
            .into_iter()
            .map(|row| row.file.name)
            .collect();
        assert_eq!(names, ["f3", "f2", "f1"]);
    }

    #[test]
    fn ask_first_is_set_per_device() {
        let dir = temp_dir("askfirst");
        TrustStore::load_or_create(dir.join(TRUST_FILE))
            .unwrap()
            .trust_device("a")
            .unwrap();
        set_ask_first(&dir, "a", true).unwrap();
        assert!(
            snapshot_with(&dir, 101, false).devices[0]
                .permissions
                .ask_first
        );
        set_ask_first(&dir, "a", false).unwrap();
        assert!(
            !snapshot_with(&dir, 101, false).devices[0]
                .permissions
                .ask_first
        );
    }

    #[test]
    fn the_log_tail_reads_the_newest_daily_file_last_lines() {
        let dir = temp_dir("logs");
        let logs = dir.join(LOG_DIR);
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("pocketdesk.2026-10-11.log"), "old\n").unwrap();
        let lines: String = (0..250).map(|i| format!("line {i}\n")).collect();
        std::fs::write(logs.join("pocketdesk.2026-10-12.log"), lines).unwrap();
        let tail = log_tail(&dir, 200);
        assert_eq!(tail.len(), 200);
        assert_eq!(tail.first().map(String::as_str), Some("line 50"));
        assert_eq!(tail.last().map(String::as_str), Some("line 249"));
        assert!(log_tail(&temp_dir("nologs"), 10).is_empty());
    }

    /// J1: a big log is read from its tail only, its cut first line dropped.
    #[test]
    fn a_big_log_is_read_from_its_tail() {
        let dir = temp_dir("biglog");
        let logs = dir.join(LOG_DIR);
        std::fs::create_dir_all(&logs).unwrap();
        // Some 600 KiB: more than the tail read.
        let lines: String = (0..20_000)
            .map(|i| format!("line {i:05} {}\n", "x".repeat(20)))
            .collect();
        std::fs::write(logs.join("pocketdesk.2026-10-20.log"), &lines).unwrap();
        let tail = log_tail(&dir, 400);
        assert_eq!(tail.len(), 400);
        assert!(tail.last().unwrap().starts_with("line 19999"));
        assert!(tail.first().unwrap().starts_with("line 19600"));
        // A tail too small for every line starts at a whole line.
        let few = log_tail(&dir, 1_000_000);
        assert!(
            few.first().unwrap().starts_with("line "),
            "no cut line: {:?}",
            few.first()
        );
        assert!(few.len() < 20_000, "only the tail was read");
        // Each line is 32 bytes, so the read starts on a line's first byte:
        // that whole line is kept, not dropped as cut (J1 review).
        assert!(
            few.first().unwrap().starts_with("line 11808"),
            "{:?}",
            few.first()
        );
    }

    /// J1: the log is read again only when its size or time changed.
    #[test]
    fn the_log_stamp_changes_with_the_file() {
        let dir = temp_dir("logstamp");
        let logs = dir.join(LOG_DIR);
        std::fs::create_dir_all(&logs).unwrap();
        assert_eq!(log_stamp(&dir), None);
        let file = logs.join("pocketdesk.2026-10-20.log");
        std::fs::write(&file, "a\n").unwrap();
        let first = log_stamp(&dir);
        assert!(first.is_some());
        assert_eq!(log_stamp(&dir), first, "unchanged");
        std::fs::write(&file, "a\nb\n").unwrap();
        assert_ne!(log_stamp(&dir), first, "grown");
    }

    #[test]
    fn only_a_phone_in_a_session_can_be_sent_a_file() {
        let dir = temp_dir("targets");
        let mut store = TrustStore::load_or_create(dir.join(TRUST_FILE)).unwrap();
        store.trust_device("a").unwrap();
        store.trust_device("b").unwrap();
        // A session alone (a remote) keeps no control link: no road for files.
        std::fs::write(dir.join(LIVE_FILE), ONE_SESSION).unwrap();
        assert!(send_targets(&snapshot_with(&dir, 101, true)).is_empty());
        std::fs::write(dir.join(LIVE_FILE), ONE_SESSION_LINKED).unwrap();
        let snap = snapshot_with(&dir, 101, true);
        let ids: Vec<_> = send_targets(&snap).iter().map(|d| d.id.clone()).collect();
        assert_eq!(ids, ["a"]);
    }

    #[test]
    fn a_press_is_queued_for_the_server_unless_media_is_off() {
        let dir = temp_dir("press");
        assert!(press_phone_media(&dir, "a", "next", 100).is_ok());
        let queued = pocketdesk_core::local::phone_media::take_presses(&dir, 101);
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].command, "next");
        assert!(press_phone_media(&dir, "a", "stop", 100).is_err());

        let mut store = TrustStore::load_or_create(dir.join(TRUST_FILE)).unwrap();
        store.trust_device("a").unwrap();
        let mut permissions = store.permissions("a");
        permissions.media = Some(false);
        store.set_permissions("a", permissions).unwrap();
        let refused = press_phone_media(&dir, "a", "next", 100).unwrap_err();
        assert!(refused.contains("Media"), "{refused}");
        assert!(pocketdesk_core::local::phone_media::take_presses(&dir, 101).is_empty());
    }

    #[test]
    fn a_link_is_queued_for_the_server_and_one_that_may_not_open_is_refused() {
        let dir = temp_dir("link");
        assert!(queue_link(&dir, "a", " https://example.org/page ", 100).is_ok());
        let queued = pocketdesk_core::local::links::take_all(&dir, 101);
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].url, "https://example.org/page", "trimmed");
        assert!(queue_link(&dir, "a", "javascript:alert(1)", 100).is_err());
        assert!(queue_link(&dir, "a", "", 100).is_err());

        let mut store = TrustStore::load_or_create(dir.join(TRUST_FILE)).unwrap();
        store.trust_device("a").unwrap();
        let mut permissions = store.permissions("a");
        permissions.links = Some(false);
        store.set_permissions("a", permissions).unwrap();
        let refused = queue_link(&dir, "a", "https://example.org/", 100).unwrap_err();
        assert!(refused.contains("Links"), "{refused}");
        assert!(pocketdesk_core::local::links::take_all(&dir, 101).is_empty());
    }

    #[test]
    fn a_send_queues_a_request_for_the_server_and_refuses_what_is_not_there() {
        let dir = temp_dir("send");
        let file = dir.join("report.pdf");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(
            queue_send(&dir, "a", &file, 100),
            Ok("report.pdf".to_string())
        );
        let queued = pocketdesk_core::local::outbox::take_all(&dir, 101);
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].device_id, "a");
        assert_eq!(queued[0].path, file.display().to_string());
        assert!(queue_send(&dir, "a", &dir.join("gone"), 100).is_err());
        assert!(pocketdesk_core::local::outbox::take_all(&dir, 101).is_empty());
        // A folder goes too (T6b): the server offers it file by file.
        let folder = dir.join("Trip");
        std::fs::create_dir_all(&folder).unwrap();
        assert_eq!(queue_send(&dir, "a", &folder, 100), Ok("Trip".to_string()));
        assert_eq!(pocketdesk_core::local::outbox::take_all(&dir, 101).len(), 1);
    }

    #[test]
    fn a_relative_path_is_queued_as_the_app_sees_it() {
        let dir = temp_dir("relative");
        // Tests run in the package folder, which holds Cargo.toml.
        assert!(queue_send(&dir, "a", std::path::Path::new("Cargo.toml"), 100).is_ok());
        let queued = pocketdesk_core::local::outbox::take_all(&dir, 101);
        let path = std::path::Path::new(&queued[0].path);
        assert!(path.is_absolute(), "{}", path.display());
        assert!(path.ends_with("Cargo.toml"));
    }

    #[test]
    fn a_send_goes_only_to_the_phone_chosen_while_it_is_in_a_session() {
        let dir = temp_dir("chosen");
        let mut store = TrustStore::load_or_create(dir.join(TRUST_FILE)).unwrap();
        store.trust_device("a").unwrap();
        store.trust_device("b").unwrap();
        std::fs::write(dir.join(LIVE_FILE), ONE_SESSION_LINKED).unwrap();
        let snap = snapshot_with(&dir, 101, true);
        assert_eq!(target_for_send(&snap, Some("a")), Ok("a".to_string()));
        assert_eq!(
            target_for_send(&snap, None),
            Ok("a".to_string()),
            "the one in a session"
        );
        assert!(
            target_for_send(&snap, Some("b")).is_err(),
            "b left its session: no silent switch to a"
        );
        let empty = snapshot_with(&temp_dir("chosen-none"), 101, true);
        assert!(target_for_send(&empty, None).is_err());

        // A linked computer (2h) is a target too, named by its name.
        let with_computer = Snapshot {
            linked_computers: vec![("desk-b".to_string(), "Desk".to_string())],
            ..Snapshot::default()
        };
        assert_eq!(
            target_for_send(&with_computer, None),
            Ok("desk-b".to_string())
        );
        assert_eq!(target_label(&with_computer, "desk-b"), "Desk (computer)");
        assert_eq!(target_label(&with_computer, "phone-a"), "phone-a");
    }

    /// J5 review: a dropped folder is called a folder.
    #[test]
    fn dropped_files_and_folders_in_words() {
        assert_eq!(files_and_folders(1, 0), "1 file");
        assert_eq!(files_and_folders(3, 0), "3 files");
        assert_eq!(files_and_folders(0, 1), "1 folder");
        assert_eq!(files_and_folders(2, 1), "2 files and 1 folder");
    }

    /// J5 review: a second drop joins the files waiting, each path once.
    #[test]
    fn dropped_files_join_the_waiting_ones_once() {
        use std::path::PathBuf;
        let mut waiting = vec![PathBuf::from("a.txt")];
        join_drops(
            &mut waiting,
            vec![PathBuf::from("b.txt"), PathBuf::from("a.txt")],
        );
        assert_eq!(waiting, [PathBuf::from("a.txt"), PathBuf::from("b.txt")]);
    }

    /// J5: a drop goes straight to the only target, asks with several.
    #[test]
    fn a_send_to_several_goes_to_those_still_there() {
        let phone = |id: &str, files: bool| Device {
            id: id.to_string(),
            takes_files: files,
            ..Default::default()
        };
        let snap = Snapshot {
            devices: vec![
                phone("phone-a", true),
                phone("phone-b", true),
                phone("phone-c", false),
            ],
            linked_computers: vec![("desk-d".to_string(), "Desk".to_string())],
            ..Snapshot::default()
        };
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            targets_for_send(&snap, &[]),
            Ok((ids(&["phone-a"]), vec![])),
            "none chosen: the first"
        );
        assert_eq!(
            targets_for_send(&snap, &ids(&["desk-d", "phone-a"])),
            Ok((ids(&["phone-a", "desk-d"]), vec![])),
            "in the page's order"
        );
        assert_eq!(
            targets_for_send(&snap, &ids(&["phone-b", "phone-c"])),
            Ok((ids(&["phone-b"]), ids(&["phone-c"]))),
            "one that left is said, the rest go"
        );
        assert!(targets_for_send(&snap, &ids(&["phone-c"])).is_err());
        assert!(targets_for_send(&Snapshot::default(), &[]).is_err());
        assert_eq!(targets_words(&snap, &ids(&["desk-d"])), "Desk (computer)");
        assert!(
            targets_words(&snap, &ids(&["phone-a", "phone-b", "desk-d"]))
                .ends_with(" and Desk (computer)")
        );
    }

    #[test]
    fn a_drop_asks_only_with_several_targets() {
        assert_eq!(drop_to(&Snapshot::default()), DropTo::Nobody);
        let phone = Device {
            id: "phone-a".to_string(),
            takes_files: true,
            ..Default::default()
        };
        let away = Device {
            id: "phone-b".to_string(),
            takes_files: false,
            ..Default::default()
        };
        let one = Snapshot {
            devices: vec![phone.clone(), away],
            ..Snapshot::default()
        };
        assert_eq!(drop_to(&one), DropTo::Send("phone-a".to_string()));
        let computer = Snapshot {
            linked_computers: vec![("desk-b".to_string(), "Desk".to_string())],
            ..Snapshot::default()
        };
        assert_eq!(drop_to(&computer), DropTo::Send("desk-b".to_string()));
        let both = Snapshot {
            devices: vec![phone],
            linked_computers: vec![("desk-b".to_string(), "Desk".to_string())],
            ..Snapshot::default()
        };
        assert_eq!(
            drop_to(&both),
            DropTo::Ask(vec!["phone-a".to_string(), "desk-b".to_string()])
        );
    }

    /// U3: a phone shows by its name, the owner's over its own, and by
    /// its id until it has one.
    #[test]
    fn a_phone_is_labelled_by_its_name_and_the_owners_rename_wins() {
        let dir = temp_dir("names");
        let mut store = TrustStore::load_or_create(dir.join(TRUST_FILE)).unwrap();
        store.trust_device("phone-a").unwrap();
        store.trust_device("0a1b2c3d-4e5f-6789").unwrap();
        store.note_claimed_name("phone-a", "Galaxy A71").unwrap();
        let snapshot = snapshot_with(&dir, 101, true);
        let a = snapshot.devices.iter().find(|d| d.id == "phone-a").unwrap();
        assert_eq!(a.name.as_deref(), Some("Galaxy A71"));
        assert!(!a.renamed, "its own claim");
        assert_eq!(target_label(&snapshot, "phone-a"), "Galaxy A71");
        assert_eq!(
            device_label(&snapshot.devices, "0a1b2c3d-4e5f-6789"),
            "0a1b2c3d-4e5…",
            "no name yet: its id"
        );

        rename_device(&dir, "phone-a", "Work phone").unwrap();
        let snapshot = snapshot_with(&dir, 101, true);
        let a = snapshot.devices.iter().find(|d| d.id == "phone-a").unwrap();
        assert_eq!(a.name.as_deref(), Some("Work phone"));
        assert!(a.renamed);
        rename_device(&dir, "phone-a", "  ").unwrap();
        let snapshot = snapshot_with(&dir, 101, true);
        assert_eq!(device_label(&snapshot.devices, "phone-a"), "Galaxy A71");

        // Two phones of one model: each name says which (U3 review).
        let mut store = TrustStore::load_or_create(dir.join(TRUST_FILE)).unwrap();
        store
            .note_claimed_name("0a1b2c3d-4e5f-6789", "Galaxy A71")
            .unwrap();
        let snapshot = snapshot_with(&dir, 101, true);
        assert_eq!(
            device_label(&snapshot.devices, "0a1b2c3d-4e5f-6789"),
            "Galaxy A71 · 0a1b2c3d-4e5…"
        );
        assert_eq!(
            device_label(&snapshot.devices, "phone-a"),
            "Galaxy A71 · phone-a"
        );
    }

    #[test]
    fn sent_files_are_listed_newest_first() {
        use pocketdesk_core::local::sent::{self, SentFile, SentState};
        let dir = temp_dir("sent");
        for n in 0..2u64 {
            sent::record(
                &dir,
                SentFile {
                    id: format!("s{n}"),
                    name: format!("f{n}"),
                    path: format!("/f{n}"),
                    size: n,
                    device_id: "a".to_string(),
                    sent_unix: n,
                    state: SentState::Sent,
                    note: None,
                },
            )
            .unwrap();
        }
        let names: Vec<_> = snapshot_with(&dir, 101, false)
            .sent
            .iter()
            .map(|f| f.name.clone())
            .collect();
        assert_eq!(names, ["f1", "f0"]);
    }

    fn session(id: u64, driving: bool) -> sessions::MirrorSession {
        sessions::MirrorSession {
            id,
            device_id: Some(format!("phone-{id}")),
            peer: "192.0.2.1:5".into(),
            transport: "lan".into(),
            started_unix: 1,
            driving,
            control_only: false,
            sound_only: false,
            phone_sound_only: false,
            cast: false,
        }
    }

    #[test]
    fn with_several_phones_the_driver_is_marked_and_the_others_can_be_given_control() {
        assert_eq!(
            control_cell(&session(1, true), 2, false),
            ControlCell::Driving
        );
        assert_eq!(
            control_cell(&session(2, false), 2, false),
            ControlCell::Give(2)
        );
    }

    /// A sound session or a cast gives no input: control is not theirs to take (N5 review).
    #[test]
    fn a_session_that_gives_no_input_is_never_offered_control() {
        let sound = sessions::MirrorSession {
            sound_only: true,
            phone_sound_only: false,
            ..session(2, false)
        };
        assert_eq!(control_cell(&sound, 2, false), ControlCell::Nothing);
        let cast = sessions::MirrorSession {
            cast: true,
            ..session(3, false)
        };
        assert_eq!(control_cell(&cast, 2, false), ControlCell::Nothing);
        // M7 review: the phone's sound alone takes no input either.
        let alone = sessions::MirrorSession {
            phone_sound_only: true,
            ..session(4, false)
        };
        assert_eq!(control_cell(&alone, 2, false), ControlCell::Nothing);
    }

    #[test]
    fn one_phone_or_an_old_server_shows_no_control() {
        assert_eq!(
            control_cell(&session(1, true), 1, false),
            ControlCell::Nothing
        );
        assert_eq!(
            control_cell(&session(0, false), 2, false),
            ControlCell::Nothing
        );
        assert_eq!(
            control_cell(&session(2, false), 2, true),
            ControlCell::Nothing,
            "nothing to hand over when every phone drives"
        );
    }
}
