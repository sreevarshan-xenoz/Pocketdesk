//! Toasts only on the computer in use (N7): a phone asks each computer it
//! is linked to whether someone is using it, and a computer answers by how
//! long its keyboard and mouse have been quiet. A copy of a notification to
//! a computer not in use, while another is, says so ([`ELSEWHERE`]), and
//! that computer keeps it without a pop-up.

use serde_json::Value;

/// Phone → computer: `{}`, are you in use?
pub const ASK: &str = "kdeconnect.pocketdesk.in_use";

/// Computer → phone: `{inUse}`, or `{}` when it does not say (its owner
/// turned it off, or it cannot tell).
pub const ANSWER: &str = "kdeconnect.pocketdesk.in_use_answer";

/// A notification's field: it came while another computer of the owner's
/// was in use, so this one keeps it without a pop-up.
pub const ELSEWHERE: &str = "elsewhere";

/// Input within this long counts as the computer in use.
pub const IN_USE_SECS: u64 = 120;

/// The answer to [`ASK`]: whether input came within [`IN_USE_SECS`], when
/// the computer is `telling` and knows (`idle_secs`).
pub fn answer_body(telling: bool, idle_secs: Option<u64>) -> Value {
    match idle_secs.filter(|_| telling) {
        Some(idle) => serde_json::json!({ "inUse": idle < IN_USE_SECS }),
        None => serde_json::json!({}),
    }
}

/// What an [`ANSWER`] says: in use or not, or nothing.
pub fn in_use(body: &Value) -> Option<bool> {
    body.get("inUse").and_then(Value::as_bool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_computer_says_in_use_by_its_quiet_or_nothing() {
        assert_eq!(in_use(&answer_body(true, Some(5))), Some(true));
        assert_eq!(
            in_use(&answer_body(true, Some(IN_USE_SECS - 1))),
            Some(true)
        );
        assert_eq!(in_use(&answer_body(true, Some(IN_USE_SECS))), Some(false));
        assert_eq!(in_use(&answer_body(true, None)), None, "cannot tell");
        assert_eq!(in_use(&answer_body(false, Some(5))), None, "turned off");
    }
}
