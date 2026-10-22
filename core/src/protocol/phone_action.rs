//! What the computer asks the phone to do to itself (O2): take a screenshot,
//! or lock its screen. The phone does it through its control service (C8)
//! with its own switch on; the screenshot reaches the computer as T10h's
//! new screenshots do.

use serde_json::json;

/// Computer → phone: `{action}`. Under the Phone actions permission.
pub const PHONE_ACTION: &str = "kdeconnect.pocketdesk.phone_action";

/// What the phone is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhoneAction {
    /// Take a screenshot, as the phone's own buttons do.
    Screenshot,
    /// Lock its screen.
    Lock,
}

impl PhoneAction {
    pub const ALL: [PhoneAction; 2] = [PhoneAction::Screenshot, PhoneAction::Lock];

    /// The word the packet and the request files carry.
    pub fn as_str(self) -> &'static str {
        match self {
            PhoneAction::Screenshot => "screenshot",
            PhoneAction::Lock => "lock",
        }
    }

    /// The action `word` names, or `None`.
    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.as_str() == word)
    }
}

/// The packet's body for `action`.
pub fn body(action: PhoneAction) -> serde_json::Value {
    json!({ "action": action.as_str() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O2: each action reads back from its word, and nothing else does.
    #[test]
    fn an_action_reads_back_from_its_word() {
        for action in PhoneAction::ALL {
            assert_eq!(PhoneAction::parse(action.as_str()), Some(action));
            assert_eq!(body(action)["action"], action.as_str());
        }
        assert_eq!(PhoneAction::parse("Lock"), None);
        assert_eq!(PhoneAction::parse("unlock"), None);
    }
}
