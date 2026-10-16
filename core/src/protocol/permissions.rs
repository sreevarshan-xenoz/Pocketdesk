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
    /// Sending files to the desktop and browsing its files.
    Files,
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
}

impl Capability {
    pub const ALL: [Capability; 13] = [
        Capability::Input,
        Capability::Clipboard,
        Capability::Files,
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
    ];

    /// What a device may do when its owner never chose (spec P1): today's
    /// behaviour, except running commands, which a pairing PIN should not
    /// hand out.
    pub fn default_allowed(self) -> bool {
        self != Capability::Exec
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
        "kdeconnect.share.request"
        | "kdeconnect.filebrowse.request"
        | "kdeconnect.pocketdesk.file_fetch"
        | "kdeconnect.pocketdesk.folder_fetch"
        | "kdeconnect.pocketdesk.file_offer_declined"
        // The phone's storage on the computer (T10g).
        | crate::protocol::phone_files::LIST
        | crate::protocol::phone_files::LISTING
        | crate::protocol::phone_files::FETCH => Capability::Files,
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
        | crate::protocol::notification_action::NOTIFICATION_ACTION => Capability::Notifications,
        "kdeconnect.sms.messages" | "kdeconnect.pocketdesk.sms_sent" => Capability::Texts,
        crate::protocol::link::LINK => Capability::Links,
        crate::protocol::phone_media::PHONE_MEDIA => Capability::Media,
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
        }
    }

    pub fn allows(&self, capability: Capability) -> bool {
        let value = match capability {
            Capability::Input => self.input,
            Capability::Clipboard => self.clipboard,
            Capability::Files => self.files,
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
            Some(Capability::Files)
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
                capability != Capability::Exec,
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
            ("kdeconnect.filebrowse.request", Some(Capability::Files)),
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
}
