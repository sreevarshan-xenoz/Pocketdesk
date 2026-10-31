//! How each answer to a phone's notification went (E1), kept by the server
//! in `<state>/notification_answers.json` (owner-only, the newest
//! [`MAX_KEPT`]) for the desktop app's Notifications page: waiting for the
//! phone, sent to it, handed to the app, or not sent and why.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::paths::NOTIFICATION_ANSWERS_FILE;
use crate::protocol::notification_action::Answered;

/// Answers kept, at most: the newest.
pub const MAX_KEPT: usize = 30;

/// Where an answer stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum State {
    /// Kept until its phone connects (within the answer's time).
    Waiting,
    /// On its way to the phone, no word back yet.
    Sent,
    /// The phone handed it to the app.
    Handed,
    /// Not sent, by this computer or the phone, and why.
    NotSent { reason: String },
}

/// One answer, by the desktop app's request id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub id: String,
    pub device_id: String,
    /// The notification's key on the phone.
    pub key: String,
    pub state: State,
    /// When it last changed.
    pub at_unix: u64,
    /// What the phone said of one handed over (N3: "opened on the phone",
    /// "offered on the phone: …"); none from a phone before N3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct AnswersFile {
    answers: Vec<Outcome>,
}

static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Every answer kept, oldest first; none when there is no file or it does
/// not read.
pub fn read(state_dir: &Path) -> Vec<Outcome> {
    std::fs::read_to_string(state_dir.join(NOTIFICATION_ANSWERS_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<AnswersFile>(&raw).ok())
        .map(|f| f.answers)
        .unwrap_or_default()
}

fn write(state_dir: &Path, answers: Vec<Outcome>) -> io::Result<()> {
    let path = state_dir.join(NOTIFICATION_ANSWERS_FILE);
    let tmp = path.with_extension("json.tmp");
    std::fs::create_dir_all(state_dir)?;
    let json = serde_json::to_string(&AnswersFile { answers }).map_err(io::Error::other)?;
    // What was answered, to whom: owner-only.
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    crate::atomic_file::replace(&tmp, &path)
}

/// How far along a state is: an answer only moves forward (E1 review), so
/// a state written late never replaces a later one.
fn rank(state: &State) -> u8 {
    match state {
        State::Waiting => 0,
        State::Sent => 1,
        State::Handed | State::NotSent { .. } => 2,
    }
}

/// Keep `outcome`: a new answer goes last, the oldest past [`MAX_KEPT`];
/// one already kept is updated where it is, and only forward.
pub fn record(state_dir: &Path, outcome: Outcome) -> io::Result<()> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut answers = read(state_dir);
    match answers.iter_mut().find(|a| a.id == outcome.id) {
        Some(kept) if rank(&kept.state) > rank(&outcome.state) => return Ok(()),
        Some(kept) => *kept = outcome,
        None => answers.push(outcome),
    }
    let len = answers.len();
    if len > MAX_KEPT {
        answers.drain(..len - MAX_KEPT);
    }
    write(state_dir, answers)
}

/// The phone `device_id` said how an answer went: kept only for an answer
/// sent to that phone and still waiting for word; whether it was.
pub fn phone_said(
    state_dir: &Path,
    device_id: &str,
    answered: &Answered,
    now_unix: u64,
) -> io::Result<bool> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut answers = read(state_dir);
    let Some(answer) = answers
        .iter_mut()
        .find(|a| a.id == answered.answer_id && a.device_id == device_id && a.state == State::Sent)
    else {
        return Ok(false);
    };
    if answered.ok {
        answer.said = answered.reason.clone();
    }
    answer.state = if answered.ok {
        State::Handed
    } else {
        State::NotSent {
            reason: answered
                .reason
                .clone()
                .unwrap_or_else(|| "the phone did not say why".to_string()),
        }
    };
    answer.at_unix = now_unix;
    write(state_dir, answers)?;
    Ok(true)
}

/// The newest answer to `device_id`'s notification `key`, if any.
pub fn latest<'a>(answers: &'a [Outcome], device_id: &str, key: &str) -> Option<&'a Outcome> {
    answers
        .iter()
        .rev()
        .find(|a| a.device_id == device_id && a.key == key)
}

/// Forget them all (at start: they describe a run that ended).
pub fn clear(state_dir: &Path) -> io::Result<()> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    match std::fs::remove_file(state_dir.join(NOTIFICATION_ANSWERS_FILE)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-answers-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn outcome(id: &str, state: State) -> Outcome {
        Outcome {
            id: id.to_string(),
            device_id: "phone-a".to_string(),
            key: "k".to_string(),
            state,
            at_unix: 100,
            said: None,
        }
    }

    #[test]
    fn an_answer_is_kept_replaced_and_capped() {
        let dir = temp_dir("keep");
        record(&dir, outcome("a", State::Waiting)).unwrap();
        record(&dir, outcome("a", State::Sent)).unwrap();
        assert_eq!(read(&dir), [outcome("a", State::Sent)]);
        for i in 0..MAX_KEPT + 5 {
            record(&dir, outcome(&format!("n{i}"), State::Sent)).unwrap();
        }
        let kept = read(&dir);
        assert_eq!(kept.len(), MAX_KEPT);
        assert_eq!(kept.last().unwrap().id, format!("n{}", MAX_KEPT + 4));
        assert_eq!(
            latest(&kept, "phone-a", "k").unwrap().id,
            format!("n{}", MAX_KEPT + 4)
        );
        clear(&dir).unwrap();
        assert!(read(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// E1 review: a state written late never takes an answer back, and an
    /// answer updated keeps its place.
    #[test]
    fn an_answer_only_moves_forward_and_keeps_its_place() {
        let dir = temp_dir("forward");
        record(&dir, outcome("a", State::Waiting)).unwrap();
        record(&dir, outcome("b", State::Waiting)).unwrap();
        record(&dir, outcome("a", State::Sent)).unwrap();
        record(&dir, outcome("a", State::Waiting)).unwrap();
        assert_eq!(
            read(&dir)[0],
            outcome("a", State::Sent),
            "not back to waiting"
        );
        record(&dir, outcome("a", State::Handed)).unwrap();
        record(&dir, outcome("a", State::Sent)).unwrap();
        let kept = read(&dir);
        assert_eq!(kept[0].state, State::Handed);
        assert_eq!(kept[1].id, "b", "updated where it was");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_the_answer_s_own_phone_says_how_it_went() {
        let dir = temp_dir("said");
        record(&dir, outcome("a", State::Sent)).unwrap();
        record(&dir, outcome("w", State::Waiting)).unwrap();
        let handed = Answered {
            answer_id: "a".to_string(),
            ok: true,
            reason: None,
        };
        assert!(
            !phone_said(&dir, "phone-b", &handed, 200).unwrap(),
            "another phone"
        );
        let waiting = Answered {
            answer_id: "w".to_string(),
            ..handed.clone()
        };
        assert!(
            !phone_said(&dir, "phone-a", &waiting, 200).unwrap(),
            "never sent"
        );
        assert!(phone_said(&dir, "phone-a", &handed, 200).unwrap());
        assert_eq!(read(&dir)[0].state, State::Handed);
        assert!(
            !phone_said(&dir, "phone-a", &handed, 300).unwrap(),
            "said once"
        );
        record(&dir, outcome("r", State::Sent)).unwrap();
        let refused = Answered {
            answer_id: "r".to_string(),
            ok: false,
            reason: Some("the app refused it".to_string()),
        };
        assert!(phone_said(&dir, "phone-a", &refused, 300).unwrap());
        assert_eq!(
            read(&dir).last().unwrap().state,
            State::NotSent {
                reason: "the app refused it".to_string()
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
