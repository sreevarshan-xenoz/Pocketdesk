//! R4 A2 — server-side session telemetry log.
//!
//! Registers a sink with `core`'s session recorder so every streaming
//! pipeline run appends one outcome line (LAN / WAN punched / WAN relayed /
//! rejected / failed, with duration, mean RTT, wire goodput and the transport's
//! own account of loss, congestion window, path MTU and path changes) to a
//! size-capped log under the state dir. Percentile tails ride the same line:
//! `rtt_*`/`enc_*` measured here, `dec_*`/`rnd_*`/`e2e_*` reported back by the
//! device doing the decoding, plus its own reading of the path as `phone_*`.
//! The production counterpart of the
//! research finding that hole punching succeeds ~70% of the time even after
//! prerequisites — the relayed share is a number we must measure, not
//! assume. `pocketdesk sessions` renders the log for humans.

use crate::state;
use anyhow::Result;
use std::path::PathBuf;

/// Rotation threshold for the human-readable log: keep the newest half when it
/// doubles past this.
const MAX_LOG_BYTES: u64 = 1024 * 1024;

/// Rotation threshold for the machine-readable record file. Roomier per line,
/// so it is capped separately — a baseline comparison needs history, not
/// tonight's tail.
const MAX_RECORD_BYTES: u64 = 4 * 1024 * 1024;

pub fn session_log_path() -> Result<PathBuf> {
    Ok(state::state_dir()?.join("streaming_sessions.log"))
}

/// JSON-lines counterpart of [`session_log_path`]: one object per session with
/// every field typed, for tooling and for the benchmark regression check
/// (roadmap 2167/2168). `pocketdesk sessions --json` prints it.
pub fn session_record_path() -> Result<PathBuf> {
    Ok(state::state_dir()?.join(pocketdesk_core::local::paths::SESSIONS_FILE))
}

/// Install the core telemetry sink → [`record`]. Called once at server
/// startup; a second call is a no-op (core keeps the first sink).
pub fn init() {
    pocketdesk_core::streaming::set_session_telemetry_callback(|report| {
        if let Err(e) = record(report) {
            tracing::warn!("Failed to record session telemetry: {e}");
        }
    });
}

/// Append one report to both stores: the tab-separated line for humans and the
/// JSON record for comparison. Each rotates independently.
pub fn record(report: &pocketdesk_core::streaming::SessionReport) -> Result<()> {
    append(session_log_path()?, report.format(), MAX_LOG_BYTES)?;
    let json = serde_json::to_string(report)?;
    append(session_record_path()?, json, MAX_RECORD_BYTES)
}

/// Append `line`, then keep the newest half of the file if it grew past
/// `max_bytes`.
fn append(path: PathBuf, line: String, max_bytes: u64) -> Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        // Rotation reads back from the same handle, which needs the fd opened
        // for reading too: O_WRONLY|O_APPEND made the first rotation on this
        // path die with EBADF (os error 9). It had never run in production
        // because the log never reached the cap.
        .read(true)
        .open(&path)?;
    writeln!(file, "{line}")?;
    let size = file.metadata()?.len();
    if size > max_bytes {
        let mut buf = vec![0u8; (size / 2) as usize];
        file.seek(SeekFrom::End(-(buf.len() as i64)))?;
        file.read_exact(&mut buf)?;
        // Drop a leading partial line from the window we kept.
        let trimmed = match buf.iter().position(|b| *b == b'\n') {
            Some(i) => &buf[i + 1..],
            None => &buf[..],
        };
        // Open fresh (truncating) — the append handle's position is at EOF
        // and keep-all-truncate would race with itself on some filesystems.
        std::fs::write(&path, trimmed)?;
    }
    Ok(())
}

/// Read the last `count` telemetry lines (chronological order).
pub fn read_recent(count: usize) -> Result<Vec<String>> {
    let all = read_recent_from(&session_log_path()?)?;
    let start = all.len().saturating_sub(count.max(1));
    Ok(all[start..].to_vec())
}

/// Read the last `count` lines of a specific log (exposed for tests).
pub fn read_recent_from(path: &PathBuf) -> Result<Vec<String>> {
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut lines: Vec<String> = String::from_utf8_lossy(&data)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect();
    if lines.len() > 10_000 {
        lines.drain(..lines.len() - 10_000);
    }
    Ok(lines)
}

/// The last `count` retained JSON records, oldest first, as parsed objects.
/// Unparseable lines are skipped rather than fatal: one corrupt line in a
/// rotated log must not hide the rest of the history.
pub fn read_records(count: usize) -> Result<Vec<serde_json::Value>> {
    read_records_from(&session_record_path()?, count)
}

/// [`read_records`] against a specific file (exposed for tests).
pub fn read_records_from(path: &PathBuf, count: usize) -> Result<Vec<serde_json::Value>> {
    let lines = read_recent_from(path)?;
    let start = lines.len().saturating_sub(count.max(1));
    Ok(lines[start..]
        .iter()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

/// Newest retained record, if any — the input a benchmark regression check
/// compares against its committed baseline.
pub fn latest_record() -> Result<Option<serde_json::Value>> {
    Ok(read_records(1)?.pop())
}

/// Human summary: outcome tally + the recent tail. Printed by
/// `pocketdesk sessions`. With `json`, emit the retained records instead — one
/// object per line, pipeable into `jq` or the benchmark comparison.
pub fn print_sessions(count: usize, json: bool) -> Result<()> {
    if json {
        let records = read_records(count)?;
        if records.is_empty() {
            // stdout is the data stream: the hint about an empty store goes to
            // stderr so `sessions --json | jq` never sees a non-JSON line.
            eprintln!(
                "No streaming sessions recorded yet (records: {}).",
                session_record_path()?.display()
            );
        }
        for record in records {
            println!("{record}");
        }
        return Ok(());
    }
    let lines = read_recent(count.max(1))?;
    if lines.is_empty() {
        println!("No streaming sessions recorded yet.");
        println!("(log: {})", session_log_path()?.display());
        return Ok(());
    }
    // Tally from the whole log, tail from the requested window.
    let all = read_recent_from(&session_log_path()?).unwrap_or_default();
    let mut tally: std::collections::BTreeMap<&str, u64> = Default::default();
    for line in &all {
        if let Some(field) = line.split('\t').find(|f| f.starts_with("outcome=")) {
            *tally.entry(&field["outcome=".len()..]).or_default() += 1;
        }
    }
    println!(
        "{} sessions recorded ({}):",
        all.len(),
        session_log_path()?.display()
    );
    for (outcome, n) in &tally {
        println!("  {outcome:<12} {n}");
    }
    let relayed = tally.get("wan_relayed").copied().unwrap_or(0)
        + all
            .iter()
            .filter(|l| l.contains("ever_relayed=true"))
            .count() as u64;
    if !all.is_empty() {
        println!(
            "relayed share: {} of {} ({:.0}%)",
            relayed,
            all.len(),
            100.0 * relayed as f64 / all.len() as f64
        );
        // A session whose path moved address is a migration or a relay giving
        // way to a punched direct path. Counted from the transport's own
        // report, so a moved path is never inferred from a log gap.
        let moved = all
            .iter()
            .filter(|line| {
                line.split('\t')
                    .find_map(|field| field.strip_prefix("path_chg="))
                    .is_some_and(|value| value != "0")
            })
            .count() as u64;
        if moved > 0 {
            println!("paths moved mid-session: {moved} of {}", all.len());
        }
    }
    println!("last {}:", count);
    for line in &lines {
        println!("  {line}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::streaming::SessionReport;
    use std::io::Write;

    fn report(outcome: &'static str) -> SessionReport {
        // Build via the core type's fields (they're public).
        let kind = match outcome {
            "lan_direct" => pocketdesk_core::streaming::SessionOutcome::LanDirect,
            "wan_punched" => pocketdesk_core::streaming::SessionOutcome::WanPunched,
            "wan_relayed" => pocketdesk_core::streaming::SessionOutcome::WanRelayed,
            "rejected" => pocketdesk_core::streaming::SessionOutcome::Rejected,
            _ => pocketdesk_core::streaming::SessionOutcome::Failed,
        };
        SessionReport {
            unix_secs: 100,
            outcome: kind,
            ever_relayed: kind == pocketdesk_core::streaming::SessionOutcome::WanRelayed,
            duration_secs: 60,
            rtt_avg_ms: 30,
            goodput_kbps: 5000,
            device_id: Some("pixel-9".into()),
            rtt_tail: None,
            encode_tail: None,
            decode_tail: None,
            render_tail: None,
            e2e_tail: None,
            link: None,
            encoder: None,
        }
    }

    #[test]
    fn recent_lines_are_chronological_and_capped() {
        let dir = std::env::temp_dir().join(format!("ll-sessions-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("streaming_sessions.log");
        for _ in 0..5 {
            let line = format!("{}\n", report("wan_relayed").format());
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(line.as_bytes())
                .unwrap();
        }
        for outcome in ["lan_direct", "wan_punched", "rejected", "failed"] {
            let line = format!("{}\n", report(outcome).format());
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(line.as_bytes())
                .unwrap();
        }
        let lines = read_recent_from(&path).unwrap();
        assert_eq!(lines.len(), 9);
        assert_eq!(lines[0].matches("outcome=").count(), 1);
        let tail = {
            let mut v = lines.iter().rev().take(3).cloned().collect::<Vec<_>>();
            v.reverse();
            v
        };
        assert!(tail[0].contains("wan_punched"));
        assert!(tail[1].contains("rejected"));
        assert!(tail[2].contains("failed"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_report_survives_the_round_trip_as_a_comparable_record() {
        let dir = std::env::temp_dir().join(format!("ll-records-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("streaming_sessions.jsonl");

        let tails = pocketdesk_core::metrics::Samples::new();
        for i in 0..300u64 {
            tails.push_micros(4_000 + i * 1_000);
        }
        let mut report = report("lan_direct");
        report.encode_tail = tails.summary_ms();
        report.rtt_tail = tails.summary_ms();
        append(
            path.clone(),
            serde_json::to_string(&report).unwrap(),
            MAX_RECORD_BYTES,
        )
        .unwrap();

        let records = read_records_from(&path, 10).unwrap();
        assert_eq!(records.len(), 1);
        let r = &records[0];
        // The fields a regression check compares must be typed, not strings:
        // `p99` has to be numerically greater than `p50`.
        assert_eq!(r["outcome"], "lan_direct");
        assert_eq!(r["encode_tail"]["count"], 300);
        assert!(
            r["encode_tail"]["p99_ms"].as_u64().unwrap()
                > r["encode_tail"]["p50_ms"].as_u64().unwrap()
        );
        assert_eq!(r["encode_tail"]["max_ms"].as_u64().unwrap(), 303);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn record_rotation_keeps_the_newest_complete_lines() {
        let dir = std::env::temp_dir().join(format!("ll-rotate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rotated.jsonl");
        // Tiny cap so rotation is exercised without writing a megabyte.
        for i in 0..200u64 {
            append(path.clone(), format!("{{\"i\":{i}}}"), 512).unwrap();
        }
        let records = read_records_from(&path, 10_000).unwrap();
        assert!(!records.is_empty());
        // Every surviving line parses: a partial first line would be dropped,
        // not silently counted as history.
        assert!(records.iter().all(|r| r["i"].is_number()));
        let newest = records.last().unwrap()["i"].as_u64().unwrap();
        assert_eq!(newest, 199, "rotation must keep the tail, not the head");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_link_record_keeps_absent_and_zero_apart_in_both_stores() {
        let dir = std::env::temp_dir().join(format!("ll-link-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("streaming_sessions.jsonl");

        let mut report = report("wan_punched");
        report.link = Some(pocketdesk_core::streaming::LinkReport {
            lost_packets: 12,
            lost_bytes: 4096,
            datagrams_sent: 9_000,
            bytes_sent: 5_000_000,
            path_changes: 1,
            relayed_secs: 4,
            // A transport that had no selected path to sample reports these as
            // `None`: a `0` would read as an uncongested path, a missing key
            // reads as unmeasured.
            congestion_events: None,
            peak_cwnd_bytes: None,
            path_mtu: None,
            black_holes_detected: None,
            // The phone's own reading arrives on every transport, unlike the
            // four above: the client measures it, not the transport API.
            client_rtt_ms: Some(28),
            client_lost_packets: Some(3),
        });
        append(
            path.clone(),
            serde_json::to_string(&report).unwrap(),
            MAX_RECORD_BYTES,
        )
        .unwrap();

        let line = report.format();
        assert!(line.contains("path_chg=1"), "{line}");
        assert!(line.contains("relayed_s=4"), "{line}");
        assert!(line.contains("lost_pk=12"), "{line}");
        assert!(line.contains("phone_rtt=28"), "{line}");
        assert!(line.contains("phone_lost=3"), "{line}");
        assert!(!line.contains("mtu="), "{line}");

        let records = read_records_from(&path, 10).unwrap();
        let link = &records[0]["link"];
        assert_eq!(link["path_changes"], 1);
        assert_eq!(link["relayed_secs"], 4);
        assert_eq!(link["client_rtt_ms"], 28);
        assert!(link["path_mtu"].is_null(), "{link}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_lines_are_skipped_not_fatal() {
        let dir = std::env::temp_dir().join(format!("ll-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("broken.jsonl");
        std::fs::write(&path, "{\"i\":1}\nnot json at all\n{\"i\":2}\n").unwrap();
        let records = read_records_from(&path, 10).unwrap();
        assert_eq!(records.len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn outcomes_are_distinguishable_in_the_log() {
        let lan = report("lan_direct").format();
        let punched = report("wan_punched").format();
        let relayed = report("wan_relayed").format();
        assert!(lan.contains("outcome=lan_direct"));
        assert!(punched.contains("outcome=wan_punched"));
        assert!(relayed.contains("outcome=wan_relayed"));
        assert!(relayed.contains("ever_relayed=true"));
        assert!(!punched.contains("ever_relayed=true"));
    }
}
