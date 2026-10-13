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
}

impl Capability {
    pub const ALL: [Capability; 7] = [
        Capability::Input,
        Capability::Clipboard,
        Capability::Files,
        Capability::Audio,
        Capability::Mic,
        Capability::Power,
        Capability::Exec,
    ];

    /// What a device may do when its owner never chose (spec P1): today's
    /// behaviour, except running commands, which a pairing PIN should not
    /// hand out.
    pub fn default_allowed(self) -> bool {
        self != Capability::Exec
    }
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
