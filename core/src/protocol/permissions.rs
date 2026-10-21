//! What a paired device may do (roadmap F5; spec
//! `docs/superpowers/specs/2026-10-13-approvals-permissions-design.md`).
//!
//! Stored per device in `trusted_devices.json` beside its trust. An entry
//! keeps only what differs from the defaults, so a capability added later
//! takes its default on devices paired earlier.

use serde::{Deserialize, Serialize};

/// One thing a device may or may not do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Pointer and keys: stream input and KDE mousepad/presenter.
    Input,
    Clipboard,
    /// Sending files to the desktop, and the phone's own folders and photos
    /// here.
    Files,
    /// Browsing this computer's folders from the phone and downloading from
    /// them (split from Files in M2b, so a phone may send files without
    /// reading the owner's).
    Browse,
    /// The desktop's sound played on the phone.
    Audio,
    /// The phone's microphone fed to the desktop.
    Mic,
    /// Lock, sleep, shut down.
    Power,
    /// Run a command on the desktop.
    Exec,
    /// Show the phone's notifications on this computer (roadmap N5a).
    Notifications,
    /// Show the phone's texts here and send replies through it (roadmap N5b).
    Texts,
    /// Open a link on the other end (roadmap T8a).
    Links,
    /// Show what the phone plays here, and control it from here (roadmap T8d).
    Media,
    /// Show the phone's own screen here (T10k).
    Cast,
    /// See this computer's named tasks and start them (ecosystem 1c).
    Tasks,
    /// Be rung from this computer when lost (N1). The one permission about
    /// what the computer sends, not what the phone may do: no packet from a
    /// phone needs it.
    Ring,
    /// Be muted from this computer, and tell it whether the phone is in Do
    /// Not Disturb (D2).
    Mute,
    /// Lock this computer when the phone leaves (D9): about what the
    /// computer does, as Ring is; off unless chosen.
    AwayLock,
    /// Play the sound the phone's apps play here, with its screen (K5); off
    /// unless chosen: it plays on this computer's speakers.
    PhoneSound,
    /// Have the phone dial a number (L4): its dialer opens with it, the
    /// owner presses Call. About what the computer sends, as Ring is.
    Dial,
    /// Send commands to the phone's automation apps (L6). About what the
    /// computer sends, as Ring is.
    Command,
}

impl Capability {
    pub const ALL: [Capability; 20] = [
        Capability::Input,
        Capability::Clipboard,
        Capability::Files,
        Capability::Browse,
        Capability::Audio,
        Capability::Mic,
        Capability::Power,
        Capability::Exec,
        Capability::Notifications,
        Capability::Texts,
        Capability::Links,
        Capability::Media,
        Capability::Cast,
        Capability::Tasks,
        Capability::Ring,
        Capability::Mute,
        Capability::AwayLock,
        Capability::PhoneSound,
        Capability::Dial,
        Capability::Command,
    ];

    /// What a device may do when its owner never chose (spec P1): today's
    /// behaviour, except running commands, which a pairing PIN should not
    /// hand out, and locking this computer when it leaves, which only the
    /// owner turns on (D9).
    pub fn default_allowed(self) -> bool {
        !matches!(
            self,
            Capability::Exec | Capability::AwayLock | Capability::PhoneSound
        )
    }
}

/// The capability a control packet uses (spec P3), or `None` for packets a
/// paired device may always send: identity, pairing, and status queries
/// (battery, monitors, windows, the notification request, find-my-device).
/// A phone's own notifications need [`Capability::Notifications`].
///
/// One table for every plugin: a plugin that adds a packet type adds a row
/// here, or it is always allowed. The privacy grab takes the desk's own
/// keyboard and mouse away, so it counts as [`Capability::Power`]. A
/// notification reply is put on the desktop's clipboard, so it counts as
/// [`Capability::Clipboard`].
///
/// An always-allowed type must have no effect beyond answering the phone or
/// ringing this desk. Waking another machine through this one is Power.
pub fn capability_for(packet_type: &str) -> Option<Capability> {
    Some(match packet_type {
        // A second screen (T10j) changes the desktop's layout: Input.
        "kdeconnect.mousepad.request"
        | "kdeconnect.presenter"
        | "kdeconnect.pocketdesk.virtual_screen" => Capability::Input,
        "kdeconnect.clipboard"
        | "kdeconnect.clipboard.connect"
        | "kdeconnect.pocketdesk.clipboard_image"
        | "kdeconnect.pocketdesk.clipboard_image_fetch"
        | "kdeconnect.pocketdesk.notification_reply" => Capability::Clipboard,
        // Browsing this computer, and a folder from it (M2b). A file fetched
        // by its path needs Browse too, which the share plugin checks: the
        // same packet fetches the desktop's offers, which are Files.
        "kdeconnect.filebrowse.request" | "kdeconnect.pocketdesk.folder_fetch" => {
            Capability::Browse
        }
        "kdeconnect.share.request"
        | "kdeconnect.pocketdesk.file_fetch"
        | "kdeconnect.pocketdesk.file_offer_declined"
        // The phone's storage on the computer (T10g).
        | crate::protocol::phone_files::LIST
        | crate::protocol::phone_files::LISTING
        | crate::protocol::phone_files::FETCH
        // Its recent photos (C7).
        | crate::protocol::phone_photos::LIST
        | crate::protocol::phone_photos::PHOTOS
        | crate::protocol::phone_photos::FETCH
        | crate::protocol::phone_photos::TAKE => Capability::Files,
        "kdeconnect.pocketdesk.audio" => Capability::Audio,
        // Waking another machine through this one is a power action too.
        "kdeconnect.pocketdesk.power"
        | "kdeconnect.pocketdesk.privacy"
        | "kdeconnect.pocketdesk.wol" => Capability::Power,
        "kdeconnect.pocketdesk.exec" => Capability::Exec,
        "kdeconnect.pocketdesk.tasks" | "kdeconnect.pocketdesk.task_run" => Capability::Tasks,
        "kdeconnect.notification"
        // Answers to the phone's notifications (T10c).
        | crate::protocol::notification_action::NOTIFICATION_REPLY
        | crate::protocol::notification_action::NOTIFICATION_ACTION
        // And how they went, from the phone (E1).
        | crate::protocol::notification_action::NOTIFICATION_ANSWERED => Capability::Notifications,
        "kdeconnect.sms.messages" | "kdeconnect.pocketdesk.sms_sent" => Capability::Texts,
        crate::protocol::link::LINK => Capability::Links,
        // A number for the phone to dial (L4), the computer's own packet.
        crate::protocol::dial::DIAL => Capability::Dial,
        // A command for the phone's automation apps (L6), the computer's own.
        crate::protocol::command::COMMAND => Capability::Command,
        crate::protocol::phone_media::PHONE_MEDIA => Capability::Media,
        crate::protocol::quiet::QUIET_STATE => Capability::Mute,
        _ => return None,
    })
}

/// One device's permissions. `None` is "the default" ([`Capability::default_allowed`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicePermissions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clipboard: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browse: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texts: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cast: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tasks: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ring: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mute: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub away_lock: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone_sound: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dial: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<bool>,
    /// Hold each session until the owner approves it at the desktop (spec P5).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ask_first: bool,
}

impl DevicePermissions {
    fn slot(&mut self, capability: Capability) -> &mut Option<bool> {
        match capability {
            Capability::Input => &mut self.input,
            Capability::Clipboard => &mut self.clipboard,
            Capability::Files => &mut self.files,
            Capability::Browse => &mut self.browse,
            Capability::Audio => &mut self.audio,
            Capability::Mic => &mut self.mic,
            Capability::Power => &mut self.power,
            Capability::Exec => &mut self.exec,
            Capability::Notifications => &mut self.notifications,
            Capability::Texts => &mut self.texts,
            Capability::Links => &mut self.links,
            Capability::Media => &mut self.media,
            Capability::Cast => &mut self.cast,
            Capability::Tasks => &mut self.tasks,
            Capability::Ring => &mut self.ring,
            Capability::Mute => &mut self.mute,
            Capability::AwayLock => &mut self.away_lock,
            Capability::PhoneSound => &mut self.phone_sound,
            Capability::Dial => &mut self.dial,
            Capability::Command => &mut self.command,
        }
    }

    pub fn allows(&self, capability: Capability) -> bool {
        let value = match capability {
            Capability::Input => self.input,
            Capability::Clipboard => self.clipboard,
            Capability::Files => self.files,
            Capability::Browse => self.browse,
            Capability::Audio => self.audio,
            Capability::Mic => self.mic,
            Capability::Power => self.power,
            Capability::Exec => self.exec,
            Capability::Notifications => self.notifications,
            Capability::Texts => self.texts,
            Capability::Links => self.links,
            Capability::Media => self.media,
            Capability::Cast => self.cast,
            Capability::Tasks => self.tasks,
            Capability::Ring => self.ring,
            Capability::Mute => self.mute,
            Capability::AwayLock => self.away_lock,
            Capability::PhoneSound => self.phone_sound,
            Capability::Dial => self.dial,
            Capability::Command => self.command,
        };
        value.unwrap_or(capability.default_allowed())
    }

    /// Allow or deny [capability]. A choice equal to the default is stored
    /// as "default", so it follows the default if that ever changes.
    pub fn set(&mut self, capability: Capability, allowed: bool) {
        *self.slot(capability) = (allowed != capability.default_allowed()).then_some(allowed);
    }

    /// Nothing differs from the defaults; the store keeps no entry.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// What another of the owner's computers may do when it pairs (2f):
    /// the clipboard, files and links, nothing else, and every screen
    /// session held for the owner. The owner can change each, as for a
    /// phone.
    pub fn computer() -> Self {
        let mut permissions = Self::default();
        for capability in Capability::ALL {
            permissions.set(
                capability,
                matches!(
                    capability,
                    Capability::Clipboard | Capability::Files | Capability::Links
                ),
            );
        }
        permissions.ask_first = true;
        permissions
    }

    /// What a phone that is not the owner's may do when it pairs (M2b),
    /// as the Pair page's "Someone else's" and every guest: send files and
    /// links, nothing else (not browse this computer's files), and every
    /// session held for the owner, since a session that may not drive still
    /// shows the screen (M2 review). The owner can change each later.
    pub fn someone_else() -> Self {
        let mut permissions = Self::default();
        for capability in Capability::ALL {
            permissions.set(
                capability,
                matches!(capability, Capability::Files | Capability::Links),
            );
        }
        permissions.ask_first = true;
        permissions
    }

    /// These permissions kept only where `narrow` allows too, and held for
    /// the owner if either is (M2 review: pairing an owner's phone again as
    /// someone else's never turns on what the owner had turned off).
    pub fn narrowed_to(&self, narrow: &Self) -> Self {
        let mut permissions = Self::default();
        for capability in Capability::ALL {
            permissions.set(
                capability,
                self.allows(capability) && narrow.allows(capability),
            );
        }
        permissions.ask_first = self.ask_first || narrow.ask_first;
        permissions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paired_computer_may_move_the_clipboard_files_and_links_only() {
        let computer = DevicePermissions::computer();
        for capability in Capability::ALL {
            assert_eq!(
                computer.allows(capability),
                matches!(
                    capability,
                    Capability::Clipboard | Capability::Files | Capability::Links
                ),
                "{capability:?}"
            );
        }
        assert!(computer.ask_first, "its screen sessions wait for the owner");
    }

    #[test]
    fn downloading_from_the_desktop_needs_the_files_permission() {
        assert_eq!(
            capability_for("kdeconnect.pocketdesk.file_fetch"),
            Some(Capability::Files)
        );
        assert_eq!(
            capability_for("kdeconnect.pocketdesk.file_offer_declined"),
            Some(Capability::Files)
        );
        // A whole folder from the remote browser: an unlisted type would be
        // allowed for every phone.
        assert_eq!(
            capability_for("kdeconnect.pocketdesk.folder_fetch"),
            Some(Capability::Browse)
        );
    }

    #[test]
    fn the_phones_media_needs_the_media_permission_on_by_default() {
        assert_eq!(
            capability_for(crate::protocol::phone_media::PHONE_MEDIA),
            Some(Capability::Media)
        );
        let mut permissions = DevicePermissions::default();
        assert!(permissions.allows(Capability::Media));
        permissions.set(Capability::Media, false);
        assert!(!permissions.allows(Capability::Media));
    }

    #[test]
    fn a_link_needs_the_links_permission_on_by_default() {
        assert_eq!(
            capability_for(crate::protocol::link::LINK),
            Some(Capability::Links)
        );
        let mut permissions = DevicePermissions::default();
        assert!(permissions.allows(Capability::Links));
        permissions.set(Capability::Links, false);
        assert!(!permissions.allows(Capability::Links));
    }

    #[test]
    fn a_device_with_no_choices_may_do_everything_but_run_commands() {
        let none = DevicePermissions::default();
        for capability in Capability::ALL {
            assert_eq!(
                none.allows(capability),
                // Nor lock this computer when it leaves, which only the
                // owner turns on (D9).
                !matches!(
                    capability,
                    Capability::Exec | Capability::AwayLock | Capability::PhoneSound
                ),
                "{capability:?}"
            );
        }
        assert!(!none.ask_first);
    }

    #[test]
    fn a_choice_equal_to_the_default_is_stored_as_default() {
        let mut permissions = DevicePermissions::default();
        permissions.set(Capability::Clipboard, false);
        permissions.set(Capability::Exec, true);
        assert!(!permissions.allows(Capability::Clipboard));
        assert!(permissions.allows(Capability::Exec));
        permissions.set(Capability::Clipboard, true);
        permissions.set(Capability::Exec, false);
        assert!(permissions.is_default());
    }

    #[test]
    fn packet_types_map_to_the_capability_they_use() {
        let cases = [
            ("kdeconnect.mousepad.request", Some(Capability::Input)),
            ("kdeconnect.presenter", Some(Capability::Input)),
            ("kdeconnect.clipboard", Some(Capability::Clipboard)),
            ("kdeconnect.clipboard.connect", Some(Capability::Clipboard)),
            (
                super::super::clipboard::CLIPBOARD_IMAGE,
                Some(Capability::Clipboard),
            ),
            (
                super::super::clipboard::CLIPBOARD_IMAGE_FETCH,
                Some(Capability::Clipboard),
            ),
            ("kdeconnect.share.request", Some(Capability::Files)),
            ("kdeconnect.filebrowse.request", Some(Capability::Browse)),
            ("kdeconnect.pocketdesk.audio", Some(Capability::Audio)),
            ("kdeconnect.pocketdesk.power", Some(Capability::Power)),
            ("kdeconnect.pocketdesk.privacy", Some(Capability::Power)),
            ("kdeconnect.pocketdesk.wol", Some(Capability::Power)),
            ("kdeconnect.pocketdesk.exec", Some(Capability::Exec)),
            ("kdeconnect.notification", Some(Capability::Notifications)),
            (
                "kdeconnect.notification.reply",
                Some(Capability::Notifications),
            ),
            (
                "kdeconnect.notification.action",
                Some(Capability::Notifications),
            ),
            ("kdeconnect.notification.request", None),
            ("kdeconnect.sms.messages", Some(Capability::Texts)),
            ("kdeconnect.pocketdesk.sms_sent", Some(Capability::Texts)),
            // Always allowed for a paired device: identity, pairing, status.
            ("kdeconnect.identity", None),
            ("kdeconnect.pair", None),
            ("kdeconnect.battery.request", None),
            ("kdeconnect.pocketdesk.monitors", None),
            (
                "kdeconnect.pocketdesk.virtual_screen",
                Some(Capability::Input),
            ),
        ];
        for (packet_type, capability) in cases {
            assert_eq!(capability_for(packet_type), capability, "{packet_type}");
        }
    }

    #[test]
    fn the_file_form_keeps_only_what_differs() {
        let mut permissions = DevicePermissions::default();
        permissions.set(Capability::Power, false);
        permissions.ask_first = true;
        let json = serde_json::to_value(&permissions).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "power": false, "ask_first": true })
        );
        let back: DevicePermissions = serde_json::from_value(json).unwrap();
        assert_eq!(back, permissions);
        let empty: DevicePermissions = serde_json::from_str("{}").unwrap();
        assert!(empty.is_default());
    }

    #[test]
    fn someone_elses_phone_may_send_files_and_links_only() {
        let permissions = DevicePermissions::someone_else();
        for capability in Capability::ALL {
            assert_eq!(
                permissions.allows(capability),
                matches!(capability, Capability::Files | Capability::Links),
                "{capability:?}"
            );
        }
        assert!(permissions.ask_first, "its sessions wait for the owner");
        assert!(!permissions.allows(Capability::Browse));
    }

    #[test]
    fn narrowing_keeps_what_the_owner_turned_off() {
        let mut owner = DevicePermissions::default();
        owner.set(Capability::Links, false);
        let narrowed = owner.narrowed_to(&DevicePermissions::someone_else());
        assert!(narrowed.allows(Capability::Files));
        assert!(!narrowed.allows(Capability::Links), "stays off");
        assert!(!narrowed.allows(Capability::Input));
        assert!(narrowed.ask_first);
    }

    #[test]
    fn browsing_this_computer_is_its_own_permission() {
        assert_eq!(
            capability_for("kdeconnect.filebrowse.request"),
            Some(Capability::Browse)
        );
        assert_eq!(
            capability_for("kdeconnect.pocketdesk.folder_fetch"),
            Some(Capability::Browse)
        );
        assert_eq!(
            capability_for("kdeconnect.share.request"),
            Some(Capability::Files)
        );
        assert!(
            Capability::Browse.default_allowed(),
            "a phone paired before keeps browsing"
        );
    }
}
