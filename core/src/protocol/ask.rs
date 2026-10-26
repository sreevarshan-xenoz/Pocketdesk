//! A question the computer asks its owner's phone (BF3): `pocketdesk ask`
//! waits for a Yes or No answered on the phone. No is one tap; Yes takes the
//! phone's fingerprint or screen lock. Under the Ask permission, on for the
//! owner's phones and off for someone else's or a paired computer.

/// Computer → phone: `{id, question, expires_unix}`.
pub const ASK: &str = "kdeconnect.pocketdesk.ask";
/// Phone → computer: `{id, answer: "yes" | "no"}`.
pub const ASK_ANSWER: &str = "kdeconnect.pocketdesk.ask_answer";

/// The most characters a question keeps.
pub const MAX_QUESTION: usize = 300;
/// How long a question waits when not told: two minutes.
pub const DEFAULT_WAIT_SECS: u64 = 120;
/// How long a question may wait at most: an hour.
pub const MAX_WAIT_SECS: u64 = 3600;

/// An answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Answer {
    Yes,
    No,
}

impl Answer {
    pub fn word(self) -> &'static str {
        match self {
            Answer::Yes => "yes",
            Answer::No => "no",
        }
    }
}

/// `question` as it is shown: control characters and runs of white space
/// made one space, trimmed, at most [`MAX_QUESTION`] characters (cut with
/// `…`). `None` when nothing is left.
pub fn clean_question(question: &str) -> Option<String> {
    let flat: String = question
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if flat.is_empty() {
        return None;
    }
    if flat.chars().count() <= MAX_QUESTION {
        return Some(flat);
    }
    let kept: String = flat.chars().take(MAX_QUESTION - 1).collect();
    Some(format!("{}…", kept.trim_end()))
}

/// Whether `id` can name a question: short, of letters, digits and `-`.
pub fn is_ask_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The question's packet body.
pub fn ask_body(id: &str, question: &str, expires_unix: u64) -> serde_json::Value {
    serde_json::json!({ "id": id, "question": question, "expires_unix": expires_unix })
}

/// The question an `ask` body carries: its id, its words (cleaned) and when
/// it stops waiting; `None` for anything else.
pub fn question_of(body: &serde_json::Value) -> Option<(String, String, u64)> {
    let id = body.get("id")?.as_str().filter(|id| is_ask_id(id))?;
    let question = clean_question(body.get("question")?.as_str()?)?;
    let expires = body.get("expires_unix")?.as_u64()?;
    Some((id.to_string(), question, expires))
}

/// The answer's packet body.
pub fn answer_body(id: &str, answer: Answer) -> serde_json::Value {
    serde_json::json!({ "id": id, "answer": answer.word() })
}

/// The answer an `ask_answer` body carries; `None` for anything else.
pub fn answer_of(body: &serde_json::Value) -> Option<(String, Answer)> {
    let id = body.get("id")?.as_str().filter(|id| is_ask_id(id))?;
    let answer = match body.get("answer")?.as_str()? {
        "yes" => Answer::Yes,
        "no" => Answer::No,
        _ => return None,
    };
    Some((id.to_string(), answer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_question_is_one_line_and_kept_short() {
        assert_eq!(
            clean_question("  Deploy\n\tto  production?\u{7}  ").as_deref(),
            Some("Deploy to production?")
        );
        assert_eq!(clean_question(" \n\t "), None);
        let long = "a".repeat(MAX_QUESTION + 50);
        let cut = clean_question(&long).unwrap();
        assert_eq!(cut.chars().count(), MAX_QUESTION);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn a_question_and_its_answer_go_both_ways() {
        let body = ask_body("q-1", "Deploy?", 1_120);
        assert_eq!(
            question_of(&body),
            Some(("q-1".to_string(), "Deploy?".to_string(), 1_120))
        );
        assert_eq!(
            answer_of(&answer_body("q-1", Answer::Yes)),
            Some(("q-1".to_string(), Answer::Yes))
        );
        assert_eq!(
            answer_of(&json!({"id": "q-1", "answer": "no"})),
            Some(("q-1".to_string(), Answer::No))
        );
    }

    #[test]
    fn anything_else_is_not_a_question_or_an_answer() {
        assert_eq!(answer_of(&json!({"id": "q-1", "answer": "maybe"})), None);
        assert_eq!(answer_of(&json!({"id": "../x", "answer": "yes"})), None);
        assert_eq!(answer_of(&json!({"answer": "yes"})), None);
        assert_eq!(
            question_of(&json!({"id": "q", "question": " ", "expires_unix": 1})),
            None
        );
        assert_eq!(question_of(&json!({"id": "q", "question": "Go?"})), None);
        assert!(!is_ask_id(&"a".repeat(65)));
        assert!(!is_ask_id(""));
    }
}
