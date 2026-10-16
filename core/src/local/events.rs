//! Events (ecosystem phase 1a): what Pocketdesk does, as it happens, for
//! other programs. The server appends one JSON object per line to
//! `events.jsonl` in the state folder (owner-only, one rotation kept);
//! `pocketdesk watch` and anything else running as the owner reads it with
//! a [`Tail`]. No listener: the same rule as the rest of the local API.
//!
//! An event carries metadata, and content only at [`EventsDetail::Full`].

use std::io::{self, BufRead, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::paths::EVENTS_FILE;

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
            std::fs::rename(&path, rotated_path(state_dir))?;
            crate::secure_fs::write_private(&path, b"")?;
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

/// Reads the events appended since it last looked, across a rotation.
#[derive(Debug)]
pub struct Tail {
    path: PathBuf,
    offset: u64,
    partial: String,
}

impl Tail {
    /// From the end of the log now: only events written from here on.
    pub fn at_end(state_dir: &Path) -> Self {
        let path = log_path(state_dir);
        let offset = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        Self {
            path,
            offset,
            partial: String::new(),
        }
    }

    /// From the start of the log: the events already in it first.
    pub fn from_start(state_dir: &Path) -> Self {
        Self {
            path: log_path(state_dir),
            offset: 0,
            partial: String::new(),
        }
    }

    /// The whole events appended since the last poll; a line not yet
    /// finished waits for the next. Lines that are not events are skipped.
    /// A rotation is seen when the new log is shorter than what was read,
    /// so one poll every second or so misses nothing at the log's pace.
    pub fn poll(&mut self) -> Vec<Event> {
        let Ok(mut file) = std::fs::File::open(&self.path) else {
            return Vec::new();
        };
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            // Rotated (or emptied): the new file is read from its start.
            self.offset = 0;
            self.partial.clear();
        }
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }
        let mut events = Vec::new();
        let mut reader = io::BufReader::new(file);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    self.offset += n as u64;
                    if !line.ends_with('\n') {
                        self.partial.push_str(&line);
                        break;
                    }
                    let whole = std::mem::take(&mut self.partial) + line.trim_end();
                    if let Ok(event) = serde_json::from_str::<Event>(&whole) {
                        events.push(event);
                    }
                }
            }
        }
        events
    }
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
