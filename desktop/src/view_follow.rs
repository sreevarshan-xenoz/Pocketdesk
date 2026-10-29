//! The clipboard and files follow the window that shows another computer's
//! screen (PC3).
//!
//! **Clipboard:** while the window controls that computer, its focus is
//! said to the server ([`pocketdesk_core::local::view::FOCUS`]) as it comes
//! and goes, settled first ([`FocusEdges`]) so a flicker says nothing. The
//! server pushes this computer's clipboard there as the focus comes, and
//! asks for that computer's back as it goes.
//!
//! **Files:** files and folders dropped on the window are queued for that
//! computer as the Files page queues them (the outbox, which the server
//! sends on the link to it), and the window says where they landed, or
//! why not, from the sent list ([`DropBatch`]). Dragging out of the window
//! is later.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use pocketdesk_core::local::sent::{SentFile, SentState};

/// How long the focus holds before it is said: a flicker (a menu, a
/// notification taking it for a moment) says nothing.
pub const SETTLE: Duration = Duration::from_millis(300);

/// A change of the window's focus, as said to the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// The window gained the focus while it controls the other computer.
    Gain,
    /// It lost it (or stopped controlling) after a [`Edge::Gain`].
    Lose,
}

/// Turns each pass's "inside" (focused while it controls the other
/// computer) into edges for the server, each only once it has held for
/// [`SETTLE`]. A [`Edge::Lose`] comes only after a [`Edge::Gain`].
#[derive(Debug, Default)]
pub struct FocusEdges {
    /// What was said last: inside or not.
    said: bool,
    /// What the passes see now, and since when.
    seen: bool,
    since: Option<Instant>,
}

impl FocusEdges {
    /// One pass: `inside` at `now`. The edge to say, if any.
    pub fn observe(&mut self, inside: bool, now: Instant) -> Option<Edge> {
        if inside != self.seen || self.since.is_none() {
            self.seen = inside;
            self.since = Some(now);
        }
        let held = self
            .since
            .is_some_and(|since| now.saturating_duration_since(since) >= SETTLE);
        if self.seen == self.said || !held {
            return None;
        }
        self.said = self.seen;
        Some(if self.said { Edge::Gain } else { Edge::Lose })
    }

    /// Whether a change waits to settle: the window looks again after
    /// [`SETTLE`].
    pub fn pending(&self) -> bool {
        self.seen != self.said
    }

    /// The window closes or its view stops: a [`Edge::Lose`] when a
    /// [`Edge::Gain`] was said, at once; either way it starts afresh.
    pub fn leave(&mut self) -> Option<Edge> {
        let was = self.said;
        *self = Self::default();
        was.then_some(Edge::Lose)
    }
}

/// The paths dropped on the window, less any empty one (a drop some
/// systems give as bytes alone has none).
pub fn dropped_paths(dropped: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    dropped
        .into_iter()
        .filter(|p| !p.as_os_str().is_empty())
        .collect()
}

/// Why nothing dropped can go now, before any is queued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Files go on the link to the other computer, and it is down.
    LinkDown,
    /// "Files over the internet" is off here: the link is that road.
    InternetFilesOffHere,
}

/// Whether the dropped files can go to the computer whose link reads
/// `link` (`linked` when up), with "Files over the internet" `internet_files`
/// here.
pub fn refusal(link: &str, internet_files: bool) -> Option<Refusal> {
    if !internet_files {
        return Some(Refusal::InternetFilesOffHere);
    }
    if link != "linked" {
        return Some(Refusal::LinkDown);
    }
    None
}

/// The files and folders among `paths`: (files, folders).
pub fn count(paths: &[PathBuf]) -> (usize, usize) {
    let folders = paths.iter().filter(|p| p.is_dir()).count();
    (paths.len() - folders, folders)
}

/// One drop on its way: what was queued (the sent list's ids), what could
/// not be, and to whom.
#[derive(Debug, Clone)]
pub struct DropBatch {
    pub name: String,
    pub files: usize,
    pub folders: usize,
    pub ids: Vec<String>,
    /// Why each one not queued was not.
    pub not_queued: Vec<String>,
    pub at: Instant,
}

/// How long a drop waits for the server to take and send it before the
/// window says it did not: past the outbox's own wait, and a file's tries.
pub const DROP_WAIT: Duration = Duration::from_secs(180);

/// Queue each of `paths` for the computer `device_id` with `queue` (the
/// outbox's request: an id, or why not), into a batch for `name`.
pub fn queue_all(
    paths: &[PathBuf],
    name: &str,
    now: Instant,
    mut queue: impl FnMut(&Path) -> Result<String, String>,
) -> DropBatch {
    let (files, folders) = count(paths);
    let mut ids = Vec::new();
    let mut not_queued = Vec::new();
    for path in paths {
        match queue(path) {
            Ok(id) => ids.push(id),
            Err(why) => not_queued.push(why),
        }
    }
    DropBatch {
        name: name.to_string(),
        files,
        folders,
        ids,
        not_queued,
        at: now,
    }
}

/// How a drop ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landed {
    /// The ones sent.
    pub sent: usize,
    /// Why each of the others was not.
    pub failed: Vec<String>,
}

/// How `batch` stands against the sent list `sent` at `now`: `None` while
/// any is still on its way, else how it ended. One the list never names
/// within [`DROP_WAIT`] (the server did not take it) failed.
pub fn landed(batch: &DropBatch, sent: &[SentFile], now: Instant) -> Option<Landed> {
    let timed_out = now.saturating_duration_since(batch.at) >= DROP_WAIT;
    let mut out = Landed {
        sent: 0,
        failed: batch.not_queued.clone(),
    };
    for id in &batch.ids {
        match sent.iter().find(|f| &f.id == id).map(|f| &f.state) {
            Some(SentState::Sent) => out.sent += 1,
            Some(SentState::Failed(why)) => out.failed.push(why.clone()),
            Some(SentState::Offered) | None if !timed_out => return None,
            Some(SentState::Offered) => {
                out.failed.push("it did not finish in time".to_string());
            }
            None => out
                .failed
                .push("this computer's Pocketdesk server did not take it".to_string()),
        }
    }
    Some(out)
}

/// What the window says of a refusal before anything is queued.
pub fn refusal_words(name: &str, refusal: &Refusal) -> String {
    match refusal {
        Refusal::LinkDown => format!(
            "Not sent: the link to {name} is down. Files go to it on the link, once it is up again."
        ),
        Refusal::InternetFilesOffHere => "Not sent: \u{201c}Files over the internet\u{201d} is off on this computer (Settings), and files go to another computer that way.".to_string(),
    }
}

/// What the window says while a drop is on its way.
pub fn sending_words(batch: &DropBatch) -> String {
    format!(
        "Sending {} to {}\u{2026}",
        crate::model::files_and_folders(batch.files, batch.folders),
        batch.name
    )
}

/// One reason a send failed, as the window says it about `name`: the
/// server's words, made plain where they are known.
pub fn reason_words(name: &str, reason: &str) -> String {
    let lower = reason.to_ascii_lowercase();
    if lower.contains("files is turned off for that computer") {
        return format!("Files is off for this computer on {name} (its Devices page)");
    }
    if lower.contains("over the internet") {
        // Its refusal, or this computer's own switch.
        return if lower.starts_with("refused") {
            format!("\u{201c}Files over the internet\u{201d} is off on {name}")
        } else {
            "\u{201c}Files over the internet\u{201d} is off on this computer".to_string()
        };
    }
    if lower.contains("not linked")
        || lower.contains("link closed")
        || lower.contains("did not answer in time")
    {
        return format!("the link to {name} is down");
    }
    reason
        .trim()
        .trim_end_matches('.')
        .trim_start_matches("refused: ")
        .to_string()
}

/// What the window says once a drop ended: where the files landed, or why
/// not.
pub fn landed_words(batch: &DropBatch, landed: &Landed) -> String {
    let what = crate::model::files_and_folders(batch.files, batch.folders);
    let mut reasons: Vec<String> = Vec::new();
    for why in &landed.failed {
        let said = reason_words(&batch.name, why);
        if !reasons.contains(&said) {
            reasons.push(said);
        }
    }
    let total = batch.ids.len() + batch.not_queued.len();
    match (landed.sent, landed.failed.len()) {
        (_, 0) => format!("Sent {what} to {}'s Downloads.", batch.name),
        (0, _) => format!("Not sent to {}: {}.", batch.name, reasons.join("; ")),
        (sent, _) => format!(
            "Sent {sent} of {total} to {}'s Downloads; not sent: {}.",
            batch.name,
            reasons.join("; ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_focus_that_holds_is_said_once_as_it_comes_and_as_it_goes() {
        let t = Instant::now();
        let mut edges = FocusEdges::default();
        assert_eq!(edges.observe(false, t), None, "outside: nothing to say");
        assert_eq!(edges.observe(true, t + ms(10)), None, "not settled yet");
        assert!(edges.pending());
        assert_eq!(edges.observe(true, t + ms(200)), None);
        assert_eq!(edges.observe(true, t + ms(310)), Some(Edge::Gain));
        assert!(!edges.pending());
        assert_eq!(edges.observe(true, t + ms(900)), None, "said once");
        assert_eq!(edges.observe(false, t + ms(1000)), None);
        assert_eq!(edges.observe(false, t + ms(1300)), Some(Edge::Lose));
        assert_eq!(edges.observe(false, t + ms(5000)), None);
    }

    #[test]
    fn a_flicker_of_the_focus_says_nothing() {
        let t = Instant::now();
        let mut edges = FocusEdges::default();
        edges.observe(true, t);
        assert_eq!(edges.observe(true, t + SETTLE), Some(Edge::Gain));
        // A menu takes the focus for a moment, and gives it back.
        assert_eq!(edges.observe(false, t + ms(1000)), None);
        assert_eq!(edges.observe(true, t + ms(1100)), None);
        assert_eq!(edges.observe(true, t + ms(2000)), None, "still inside");
        assert!(!edges.pending());
        // The window opened unfocused and flickered in: nothing either.
        let mut edges = FocusEdges::default();
        edges.observe(true, t);
        assert_eq!(edges.observe(false, t + ms(100)), None);
        assert_eq!(
            edges.observe(false, t + ms(1000)),
            None,
            "no lose before a gain"
        );
    }

    #[test]
    fn leaving_says_lose_only_after_a_gain() {
        let t = Instant::now();
        let mut edges = FocusEdges::default();
        assert_eq!(edges.leave(), None);
        edges.observe(true, t);
        edges.observe(true, t + SETTLE);
        assert_eq!(edges.leave(), Some(Edge::Lose));
        assert_eq!(edges.leave(), None, "once");
        // Afresh: the next view gains again.
        edges.observe(true, t + ms(1000));
        assert_eq!(edges.observe(true, t + ms(1000) + SETTLE), Some(Edge::Gain));
    }

    #[test]
    fn a_drop_goes_only_with_the_link_up_and_files_over_the_internet_on() {
        assert_eq!(refusal("linked", true), None);
        assert_eq!(
            refusal("away: no internet endpoint", true),
            Some(Refusal::LinkDown)
        );
        assert_eq!(refusal("", true), Some(Refusal::LinkDown));
        assert_eq!(
            refusal("linked", false),
            Some(Refusal::InternetFilesOffHere),
            "the switch here first: turning on the link would not help"
        );
        assert!(refusal_words("Desk", &Refusal::LinkDown).contains("link to Desk is down"));
        assert!(
            refusal_words("Desk", &Refusal::InternetFilesOffHere)
                .contains("Files over the internet")
        );
    }

    #[test]
    fn only_dropped_paths_are_sent() {
        let dropped = [PathBuf::from("a.txt"), PathBuf::new()];
        assert_eq!(dropped_paths(dropped), [PathBuf::from("a.txt")]);
    }

    /// Each path dropped is one request; one that cannot be queued is said
    /// with the rest.
    #[test]
    fn each_dropped_path_is_one_request_for_that_computer() {
        let dir = std::env::temp_dir().join(format!("pd-pc3-drop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("photos")).unwrap();
        std::fs::write(dir.join("a.txt"), b"a").unwrap();
        let paths = vec![dir.join("a.txt"), dir.join("photos"), dir.join("gone.txt")];
        let mut asked = Vec::new();
        let t = Instant::now();
        let batch = queue_all(&paths, "Desk", t, |p| {
            asked.push(p.to_path_buf());
            if p.exists() {
                Ok(format!("id-{}", asked.len()))
            } else {
                Err("gone.txt is not a file or a folder.".to_string())
            }
        });
        assert_eq!(asked, paths, "every path, in order");
        assert_eq!(
            (batch.files, batch.folders),
            (2, 1),
            "a missing path counts as a file"
        );
        assert_eq!(batch.ids, ["id-1", "id-2"]);
        assert_eq!(batch.not_queued, ["gone.txt is not a file or a folder."]);
        assert_eq!(batch.name, "Desk");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn row(id: &str, state: SentState) -> SentFile {
        SentFile {
            id: id.to_string(),
            name: "a.txt".to_string(),
            path: "a.txt".to_string(),
            size: 1,
            device_id: "desk".to_string(),
            sent_unix: 0,
            state,
            note: None,
        }
    }

    fn batch(ids: &[&str], files: usize, folders: usize, at: Instant) -> DropBatch {
        DropBatch {
            name: "Desk".to_string(),
            files,
            folders,
            ids: ids.iter().map(|s| s.to_string()).collect(),
            not_queued: Vec::new(),
            at,
        }
    }

    #[test]
    fn a_drop_lands_once_every_request_settled() {
        let t = Instant::now();
        let b = batch(&["1", "2", "3"], 3, 0, t);
        let mut sent = vec![row("1", SentState::Sent), row("2", SentState::Offered)];
        assert_eq!(landed(&b, &sent, t + ms(500)), None, "two on their way");
        sent[1].state = SentState::Sent;
        sent.push(row("3", SentState::Sent));
        let done = landed(&b, &sent, t + ms(900)).unwrap();
        assert_eq!(
            done,
            Landed {
                sent: 3,
                failed: Vec::new()
            }
        );
        assert_eq!(landed_words(&b, &done), "Sent 3 files to Desk's Downloads.");
        // Rows of other sends are not this drop's.
        sent.push(row("other", SentState::Failed("no".into())));
        assert_eq!(landed(&b, &sent, t + ms(900)), Some(done));
    }

    #[test]
    fn a_drop_the_server_never_took_fails_after_the_wait() {
        let t = Instant::now();
        let b = batch(&["1"], 1, 0, t);
        assert_eq!(landed(&b, &[], t + ms(1000)), None);
        let late = landed(&b, &[], t + DROP_WAIT).unwrap();
        assert_eq!(late.sent, 0);
        assert!(landed_words(&b, &late).contains("server did not take it"));
        let stuck = landed(&b, &[row("1", SentState::Offered)], t + DROP_WAIT).unwrap();
        assert!(landed_words(&b, &stuck).contains("did not finish in time"));
    }

    #[test]
    fn the_toast_says_where_they_landed_or_why_not() {
        let t = Instant::now();
        let one = batch(&["1"], 0, 1, t);
        assert_eq!(
            landed_words(
                &one,
                &Landed {
                    sent: 1,
                    failed: Vec::new()
                }
            ),
            "Sent 1 folder to Desk's Downloads."
        );
        let two = batch(&["1", "2"], 2, 0, t);
        assert_eq!(sending_words(&two), "Sending 2 files to Desk\u{2026}");
        let refused = Landed {
            sent: 0,
            failed: vec![
                "refused: Files is turned off for that computer here".into(),
                "refused: Files is turned off for that computer here".into(),
            ],
        };
        assert_eq!(
            landed_words(&two, &refused),
            "Not sent to Desk: Files is off for this computer on Desk (its Devices page)."
        );
        let half = Landed {
            sent: 1,
            failed: vec!["the computer is not linked now".into()],
        };
        assert_eq!(
            landed_words(&two, &half),
            "Sent 1 of 2 to Desk's Downloads; not sent: the link to Desk is down."
        );
    }

    #[test]
    fn the_servers_reasons_are_said_plainly() {
        let say = |why| reason_words("Desk", why);
        assert_eq!(
            say("refused: files over the internet road are off on this computer"),
            "\u{201c}Files over the internet\u{201d} is off on Desk"
        );
        assert_eq!(
            say("files over the internet are off on this computer (Settings)"),
            "\u{201c}Files over the internet\u{201d} is off on this computer"
        );
        assert_eq!(say("the link closed"), "the link to Desk is down");
        assert_eq!(
            say("the other computer did not answer in time"),
            "the link to Desk is down"
        );
        assert_eq!(say("refused: no space left."), "no space left");
        assert_eq!(say("there is no such file"), "there is no such file");
    }
}
