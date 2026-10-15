//! What a receiver remembers of unfinished transfers (T6a), so a retry
//! continues instead of starting over.
//!
//! One JSON record per partial file, named by the sending device and the
//! file's source id. The bytes live where the caller keeps them (a file
//! beside the record on the desktop, a pending MediaStore row on the
//! phone): [`Store::sweep`] returns the records it dropped, and the caller
//! deletes their bytes.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io;
use std::path::{Path, PathBuf};

/// One unfinished transfer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The device sending the file.
    pub device: String,
    /// The sender's id for the file: the same on every attempt.
    pub source: String,
    /// The file's name, for the final save.
    pub name: String,
    /// The file's size.
    pub size: u64,
    /// How many of its first bytes are durable.
    pub committed: u64,
    /// When the first attempt began.
    pub created_unix: u64,
    /// Where the bytes are: a file name in the store's folder, or a URI.
    pub location: String,
}

/// How much unfinished work is kept.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Older than this is dropped. Zero keeps nothing.
    pub max_age_secs: u64,
    /// At most this many per sending device; the oldest go first.
    pub per_device: usize,
    /// At most this many committed bytes in all; the oldest go first.
    pub total_bytes: u64,
}

/// Whether `source` is a usable source id: 32 hex digits.
pub fn valid_source(source: &str) -> bool {
    source.len() == 32 && source.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The record store in `dir`.
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The name both the record and (on the desktop) the bytes take: from
    /// the device and the source, so one device can never name another's.
    pub fn id(device: &str, source: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(device.as_bytes());
        hasher.update([0]);
        hasher.update(source.as_bytes());
        hasher.finalize()[..16]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// The record of `device`'s file `source`, if one is kept.
    pub fn get(&self, device: &str, source: &str) -> Option<Record> {
        let bytes = std::fs::read(self.path(&Self::id(device, source))).ok()?;
        let record: Record = serde_json::from_slice(&bytes).ok()?;
        (record.device == device && record.source == source).then_some(record)
    }

    /// Keep `record`, replacing any earlier one for the same file.
    pub fn put(&self, record: &Record) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let id = Self::id(&record.device, &record.source);
        let tmp = self.dir.join(format!("{id}.json.tmp"));
        std::fs::write(&tmp, serde_json::to_vec(record)?)?;
        std::fs::rename(&tmp, self.path(&id))
    }

    /// Forget `device`'s file `source`.
    pub fn remove(&self, device: &str, source: &str) {
        let _ = std::fs::remove_file(self.path(&Self::id(device, source)));
    }

    /// Every record kept, unreadable ones skipped.
    pub fn all(&self) -> Vec<Record> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
            .collect()
    }

    /// Drop what `limits` no longer allow at `now`, oldest first, and return
    /// the dropped records so the caller can delete their bytes. A record
    /// `in_use` (a transfer is writing it now) is never dropped, though it
    /// still counts against the limits.
    pub fn sweep(&self, now: u64, limits: Limits, in_use: impl Fn(&Record) -> bool) -> Vec<Record> {
        let mut kept = self.all();
        kept.sort_by_key(|r| std::cmp::Reverse(r.created_unix));
        let mut dropped = Vec::new();
        let mut per_device: std::collections::HashMap<String, usize> = Default::default();
        let mut bytes = 0u64;
        for record in kept {
            let count = per_device.entry(record.device.clone()).or_default();
            let too_old = now.saturating_sub(record.created_unix) >= limits.max_age_secs;
            let keep = in_use(&record)
                || (!too_old
                    && *count < limits.per_device
                    && bytes.saturating_add(record.committed) <= limits.total_bytes);
            if keep {
                *count += 1;
                bytes += record.committed;
            } else {
                self.remove(&record.device, &record.source);
                dropped.push(record);
            }
        }
        dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> Store {
        let dir = std::env::temp_dir().join(format!("pd-partials-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Store::new(dir)
    }

    fn record(device: &str, source: &str, committed: u64, created: u64) -> Record {
        Record {
            device: device.into(),
            source: source.into(),
            name: "a.bin".into(),
            size: 1000,
            committed,
            created_unix: created,
            location: "x".into(),
        }
    }

    const SOURCE: &str = "0123456789abcdef0123456789abcdef";
    const OTHER: &str = "fedcba9876543210fedcba9876543210";

    #[test]
    fn a_record_is_found_only_for_its_own_device_and_source() {
        let store = store("own");
        store.put(&record("phone-a", SOURCE, 10, 100)).unwrap();
        assert_eq!(store.get("phone-a", SOURCE).unwrap().committed, 10);
        assert!(store.get("phone-b", SOURCE).is_none(), "another device");
        assert!(store.get("phone-a", OTHER).is_none(), "another file");
        store.remove("phone-a", SOURCE);
        assert!(store.get("phone-a", SOURCE).is_none());
        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn ids_do_not_collide_across_the_separator() {
        assert_ne!(Store::id("ab", "c"), Store::id("a", "bc"));
        assert_eq!(Store::id("a", SOURCE).len(), 32);
    }

    #[test]
    fn the_sweep_drops_the_old_then_keeps_the_newest_within_the_limits() {
        let store = store("sweep");
        let limits = Limits {
            max_age_secs: 1000,
            per_device: 2,
            total_bytes: 250,
        };
        let sources: Vec<String> = (0..5).map(|i| format!("{i:032x}")).collect();
        store.put(&record("a", &sources[0], 10, 0)).unwrap(); // too old
        store.put(&record("a", &sources[1], 10, 1500)).unwrap(); // third of a
        store.put(&record("a", &sources[2], 10, 1600)).unwrap();
        store.put(&record("a", &sources[3], 10, 1700)).unwrap();
        store.put(&record("b", &sources[4], 300, 1800)).unwrap(); // over the bytes
        let mut dropped: Vec<String> = store
            .sweep(1900, limits, |_| false)
            .into_iter()
            .map(|r| r.source)
            .collect();
        dropped.sort();
        assert_eq!(
            dropped,
            vec![sources[0].clone(), sources[1].clone(), sources[4].clone()]
        );
        let mut kept: Vec<String> = store.all().into_iter().map(|r| r.source).collect();
        kept.sort();
        assert_eq!(kept, vec![sources[2].clone(), sources[3].clone()]);
        let nothing = Limits {
            max_age_secs: 0,
            ..limits
        };
        assert_eq!(
            store.sweep(1900, nothing, |_| false).len(),
            2,
            "zero keeps nothing"
        );
        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn a_record_in_use_is_never_swept() {
        let store = store("in-use");
        let limits = Limits {
            max_age_secs: 0,
            per_device: 0,
            total_bytes: 0,
        };
        store.put(&record("a", SOURCE, 10, 0)).unwrap();
        store.put(&record("a", OTHER, 10, 0)).unwrap();
        let dropped = store.sweep(100, limits, |r| r.source == SOURCE);
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].source, OTHER);
        assert!(
            store.get("a", SOURCE).is_some(),
            "the transfer using it keeps it"
        );
        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn only_32_hex_digits_are_a_source_id() {
        assert!(valid_source(SOURCE));
        assert!(!valid_source("../../etc/passwd"));
        assert!(!valid_source(&SOURCE[..31]));
        assert!(!valid_source("0123456789abcdef0123456789abcdeg"));
    }
}
