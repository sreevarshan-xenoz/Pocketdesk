//! Events (ecosystem phase 1a): what Pocketdesk does, as it happens, for
//! other programs. The server appends one JSON object per line to
//! `events.jsonl` in the state folder (owner-only, one rotation kept);
//! `pocketdesk events` and anything else running as the owner reads it with
//! a [`Tail`]. No listener: the same rule as the rest of the local API.
//!
//! An event carries metadata, and content only at [`EventsDetail::Full`].

use std::io::{self, BufRead, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::paths::EVENTS_FILE;

/// Every kind of event, as `docs/local-api.md` lists them.
pub const KINDS: &[&str] = &[
    "session_started",
    "session_ended",
    "phone_linked",
    "phone_unlinked",
    "phone_battery",
    "phone_battery_low",
    "notification",
    "text_received",
    "file_received",
    "link_received",
    "session_refused",
    "phone_paired",
    "phone_unpaired",
    "hook_ran",
];

/// The log moves to `events.jsonl.1` at this size.
pub const MAX_LOG: u64 = 1024 * 1024;

/// How much an event says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventsDetail {
    /// Who, when and how much, never what: no notification or text content,
    /// no file names, no full links.
    #[default]
    Metadata,
    /// Also the content: notification title and text, a text's sender and
    /// body, a file's name, a link's address.
    Full,
}

/// One event: `{"at", "event", "device_id", ...fields}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Unix seconds.
    pub at: u64,
    pub event: String,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

impl Event {
    /// An event of `kind`: its `meta` fields always, its `content` fields
    /// only at [`EventsDetail::Full`].
    pub fn new(
        kind: &str,
        device_id: Option<&str>,
        at: u64,
        meta: Value,
        content: Value,
        detail: EventsDetail,
    ) -> Self {
        let mut fields = Map::new();
        let reserved = ["at", "event", "device_id", "api"];
        let mut take = |value: Value| {
            if let Value::Object(map) = value {
                for (key, value) in map {
                    if !reserved.contains(&key.as_str()) {
                        fields.insert(key, value);
                    }
                }
            }
        };
        take(meta);
        if detail == EventsDetail::Full {
            take(content);
        }
        Self {
            at,
            event: kind.to_string(),
            device_id: device_id.map(str::to_string),
            fields,
        }
    }

    /// The event as one line of the log (no newline).
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

fn log_path(state_dir: &Path) -> PathBuf {
    state_dir.join(EVENTS_FILE)
}

fn rotated_path(state_dir: &Path) -> PathBuf {
    state_dir.join(format!("{EVENTS_FILE}.1"))
}

/// Append `event` to the log (the server's side), moving a full log aside
/// first. Not safe for two writers at once: the server is the only one.
pub fn append(state_dir: &Path, event: &Event) -> io::Result<()> {
    let path = log_path(state_dir);
    let line = event.to_line() + "\n";
    match std::fs::metadata(&path) {
        Ok(meta) if meta.len() + line.len() as u64 > MAX_LOG => {
            // Another program holding the log open (on Windows, without
            // sharing deletes) stops the move: the event is still kept,
            // and the move is tried again on the next one.
            if std::fs::rename(&path, rotated_path(state_dir)).is_ok() {
                crate::secure_fs::write_private(&path, b"")?;
            }
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir_all(state_dir)?;
            crate::secure_fs::write_private(&path, b"")?;
        }
        Err(e) => return Err(e),
    }
    let mut file = std::fs::OpenOptions::new().append(true).open(&path)?;
    file.write_all(line.as_bytes())
}

/// The last `max` events in the log, oldest first: the moved log's, then
/// the current one's.
pub fn history(state_dir: &Path, max: usize) -> Vec<Event> {
    let mut events = Vec::new();
    for path in [rotated_path(state_dir), log_path(state_dir)] {
        let (mut offset, mut partial) = (0, String::new());
        events.extend(read_lines(&path, &mut offset, &mut partial));
    }
    let skip = events.len().saturating_sub(max);
    events.split_off(skip)
}

/// Reads the events appended since it last looked, across a rotation.
#[derive(Debug)]
pub struct Tail {
    path: PathBuf,
    rotated: PathBuf,
    offset: u64,
    partial: String,
    /// The first line of the log being read: a log whose first line is
    /// another is a new one, the old having moved to `.1`.
    head: Option<String>,
}

/// The first whole line of `path`, if it has one.
fn first_line(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut line = String::new();
    io::BufReader::new(file).read_line(&mut line).ok()?;
    line.ends_with('\n').then_some(line)
}

impl Tail {
    fn new(state_dir: &Path, offset: u64) -> Self {
        let path = log_path(state_dir);
        Self {
            head: first_line(&path),
            rotated: rotated_path(state_dir),
            path,
            offset,
            partial: String::new(),
        }
    }

    /// From the end of the log now: only events written from here on.
    pub fn at_end(state_dir: &Path) -> Self {
        let offset = std::fs::metadata(log_path(state_dir))
            .map(|m| m.len())
            .unwrap_or(0);
        Self::new(state_dir, offset)
    }

    /// From the start of the log: the events already in it first.
    pub fn from_start(state_dir: &Path) -> Self {
        Self::new(state_dir, 0)
    }

    /// The whole events appended since the last poll; a line not yet
    /// finished waits for the next. Lines that are not events are skipped.
    /// After a rotation the old log's rest is read from `.1` first; two
    /// rotations between polls (2 MiB of events) lose the first's rest.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        let len = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        let head = first_line(&self.path);
        let rotated = len < self.offset || (self.head.is_some() && head != self.head);
        if rotated {
            if self.head.is_some() && first_line(&self.rotated) == self.head {
                events.extend(read_lines(
                    &self.rotated,
                    &mut self.offset,
                    &mut self.partial,
                ));
            }
            self.offset = 0;
            self.partial.clear();
        }
        self.head = head;
        events.extend(read_lines(&self.path, &mut self.offset, &mut self.partial));
        events
    }
}

/// The whole events in `path` from `offset`, moving it past them; a
/// line not yet finished is kept in `partial`.
fn read_lines(path: &Path, offset: &mut u64, partial: &mut String) -> Vec<Event> {
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    if file.seek(SeekFrom::Start(*offset)).is_err() {
        return Vec::new();
    }
    let mut events = Vec::new();
    let mut reader = io::BufReader::new(file);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                *offset += n as u64;
                if !line.ends_with('\n') {
                    partial.push_str(&line);
                    break;
                }
                let whole = std::mem::take(partial) + line.trim_end();
                if let Ok(event) = serde_json::from_str::<Event>(&whole) {
                    events.push(event);
                }
            }
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-events-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file_event(detail: EventsDetail) -> Event {
        Event::new(
            "file_received",
            Some("phone-1"),
            7,
            json!({"size": 42}),
            json!({"name": "holiday.jpg"}),
            detail,
        )
    }

    #[test]
    fn content_is_carried_only_at_full_detail() {
        let meta = file_event(EventsDetail::Metadata);
        assert_eq!(meta.fields.get("size"), Some(&json!(42)));
        assert!(meta.fields.get("name").is_none(), "no file name by default");
        let full = file_event(EventsDetail::Full);
        assert_eq!(full.fields.get("name"), Some(&json!("holiday.jpg")));
    }

    #[test]
    fn a_field_cannot_pose_as_the_events_own() {
        let event = Event::new(
            "link_received",
            None,
            1,
            json!({"event": "session_started", "at": 99, "host": "example.org"}),
            json!({}),
            EventsDetail::Metadata,
        );
        let line: Value = serde_json::from_str(&event.to_line()).unwrap();
        assert_eq!(line["event"], "link_received");
        assert_eq!(line["at"], 1);
        assert_eq!(line["host"], "example.org");
    }

    #[test]
    fn a_tail_reads_what_is_appended_after_it_and_from_the_start() {
        let dir = temp("tail");
        append(&dir, &file_event(EventsDetail::Metadata)).unwrap();
        let mut late = Tail::at_end(&dir);
        let mut early = Tail::from_start(&dir);
        assert!(late.poll().is_empty(), "nothing new yet");
        let next = Event::new(
            "session_ended",
            None,
            8,
            json!({"session_id": 3}),
            json!({}),
            EventsDetail::Metadata,
        );
        append(&dir, &next).unwrap();
        assert_eq!(late.poll(), vec![next.clone()]);
        assert_eq!(early.poll().len(), 2);
        assert!(late.poll().is_empty(), "each event once");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_half_written_line_waits_for_its_end() {
        let dir = temp("partial");
        let mut tail = Tail::at_end(&dir);
        let line = file_event(EventsDetail::Metadata).to_line();
        let (head, rest) = line.split_at(10);
        std::fs::write(log_path(&dir), head).unwrap();
        assert!(tail.poll().is_empty());
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(log_path(&dir))
            .unwrap();
        writeln!(file, "{rest}").unwrap();
        assert_eq!(tail.poll().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn events_written_just_before_a_rotation_are_read_from_the_old_log() {
        let dir = temp("straddle");
        let big = Event::new(
            "notification",
            None,
            1,
            json!({"app": "y".repeat(4096)}),
            json!({}),
            EventsDetail::Metadata,
        );
        let per_log = (MAX_LOG / (big.to_line().len() as u64 + 1)) as usize;
        let mut tail = Tail::at_end(&dir);
        for _ in 0..per_log - 3 {
            append(&dir, &big).unwrap();
        }
        assert_eq!(tail.poll().len(), per_log - 3);
        // Three more fill the log, and the next rotates it, all between
        // two polls.
        for _ in 0..4 {
            append(&dir, &big).unwrap();
        }
        assert!(rotated_path(&dir).exists());
        assert_eq!(
            tail.poll().len(),
            4,
            "the three in the old log and the one in the new"
        );
        // The new log grows past the old offset unread: still a new log.
        for _ in 0..per_log / 2 {
            append(&dir, &big).unwrap();
        }
        assert_eq!(tail.poll().len(), per_log / 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_history_is_both_logs_newest_last_and_capped() {
        let dir = temp("history");
        let at = |n: u64| {
            Event::new(
                "session_ended",
                None,
                n,
                json!({"session_id": n}),
                json!({}),
                EventsDetail::Metadata,
            )
        };
        std::fs::write(
            rotated_path(&dir),
            at(1).to_line() + "\n" + &at(2).to_line() + "\n",
        )
        .unwrap();
        append(&dir, &at(3)).unwrap();
        append(&dir, &at(4)).unwrap();
        let all: Vec<u64> = history(&dir, 10).iter().map(|e| e.at).collect();
        assert_eq!(all, vec![1, 2, 3, 4]);
        let last: Vec<u64> = history(&dir, 3).iter().map(|e| e.at).collect();
        assert_eq!(last, vec![2, 3, 4]);
        assert!(history(&temp("history-empty"), 5).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rotation_that_fails_still_keeps_the_event() {
        let dir = temp("blocked");
        // A directory where the rotated log goes: the rename fails.
        std::fs::create_dir_all(rotated_path(&dir)).unwrap();
        std::fs::create_dir_all(rotated_path(&dir).join("x")).unwrap();
        let line_len = file_event(EventsDetail::Metadata).to_line().len() as u64 + 1;
        std::fs::write(log_path(&dir), "x".repeat((MAX_LOG - 1) as usize) + "\n").unwrap();
        let mut tail = Tail::at_end(&dir);
        append(&dir, &file_event(EventsDetail::Metadata)).unwrap();
        assert_eq!(tail.poll().len(), 1, "kept in the full log");
        assert!(std::fs::metadata(log_path(&dir)).unwrap().len() >= MAX_LOG + line_len - 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_full_log_moves_aside_and_a_tail_follows_it() {
        let dir = temp("rotate");
        let big = Event::new(
            "notification",
            None,
            1,
            json!({"app": "x".repeat(4096)}),
            json!({}),
            EventsDetail::Metadata,
        );
        let mut tail = Tail::at_end(&dir);
        let per_log = (MAX_LOG / (big.to_line().len() as u64 + 1)) as usize;
        for _ in 0..per_log {
            append(&dir, &big).unwrap();
        }
        assert_eq!(tail.poll().len(), per_log);
        append(&dir, &big).unwrap();
        assert!(rotated_path(&dir).exists(), "the full log moved aside");
        assert!(std::fs::metadata(log_path(&dir)).unwrap().len() < MAX_LOG);
        assert_eq!(tail.poll().len(), 1, "the tail follows the new log");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
