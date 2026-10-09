//! `linux-link bench` — the pinned encoder benchmark and its regression gate
//! (roadmap 2195-2200).
//!
//! The measurement itself lives in `core::streaming::bench`; this module is the
//! part that a CI job consumes: run the clip, print the record, compare it
//! against a committed baseline, and leave an exit code that distinguishes
//! "regressed" from "this machine is not the baseline machine".
//!
//! Those two are deliberately different codes. A check that reports a
//! cross-host timing difference as a failure teaches everyone to ignore the
//! check; one that reports it as a skip keeps the failure signal honest.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use linux_link_core::streaming::bench::{self, BenchRecord, BenchTarget};

/// Everything measured fine and the tail is inside its allowance.
pub const EXIT_PASS: i32 = 0;
/// A percentile moved beyond `--tolerance-pct` against the baseline.
pub const EXIT_REGRESSION: i32 = 1;
/// Bad arguments.
pub const EXIT_USAGE: i32 = 2;
/// No baseline matches this workload/host, so nothing was concluded.
pub const EXIT_INCOMPARABLE: i32 = 3;
/// The benchmark itself could not run (no encoder, unwritable output).
pub const EXIT_UNMEASURED: i32 = 4;

#[derive(Debug)]
struct Options {
    target: BenchTarget,
    frames: u32,
    repeat: u32,
    json: bool,
    record: Option<PathBuf>,
    baseline: Option<PathBuf>,
    tolerance_pct: u64,
}

/// Run the benchmark. Returns the process exit code — see `EXIT_*` above.
pub fn run(
    target: &str,
    frames: u32,
    repeat: u32,
    json: bool,
    record: Option<PathBuf>,
    baseline: Option<PathBuf>,
    tolerance_pct: u64,
) -> i32 {
    let Some(target) = BenchTarget::parse(target) else {
        eprintln!("unknown benchmark target; expected software, vaapi, nvenc, amf, qsv or mf");
        return EXIT_USAGE;
    };
    let options = Options {
        target,
        frames,
        repeat: repeat.max(1),
        json,
        record,
        baseline,
        tolerance_pct,
    };
    match measure(&options) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("benchmark failed: {e:#}");
            EXIT_UNMEASURED
        }
    }
}

fn measure(options: &Options) -> Result<i32> {
    let mut runs = Vec::with_capacity(options.repeat as usize);
    for _ in 0..options.repeat {
        runs.push(
            bench::run(options.target, options.frames)
                .with_context(|| format!("opening the {:?} encoder", options.target))?,
        );
    }
    let measured = least_contaminated(runs).context("no benchmark runs to report")?;

    if options.json {
        println!(
            "{}",
            serde_json::to_string(&measured).context("serialising the record")?
        );
    } else {
        print_human(&measured);
    }

    if let Some(path) = &options.record {
        write_record(path, &measured)?;
    }

    let Some(baseline_path) = &options.baseline else {
        return Ok(EXIT_PASS);
    };
    Ok(gate(
        baseline_path,
        &measured,
        options.tolerance_pct,
        options.json,
    ))
}

/// The run to keep out of several.
///
/// Other work on the machine can only ever make an encode slower, so the
/// fastest run is the least contaminated one — the standard choice for timing
/// benchmarks, and the reason `--repeat` exists. A desktop that is compiling
/// something else while a baseline is recorded would otherwise freeze that
/// interference in as the reference every later run is graded against.
fn least_contaminated(mut runs: Vec<BenchRecord>) -> Option<BenchRecord> {
    if runs.len() > 1 {
        runs.sort_by_key(|r| (r.encode_ms.p95_ms, r.encode_ms.p50_ms));
    }
    runs.into_iter().next()
}

/// The verdict half, split out so a CI job's contract is unit-testable without
/// encoding a frame.
///
/// In `--json` mode stdout has already carried the record and nothing else, so
/// the verdict goes to stderr: the exit code is the machine-readable answer.
fn gate(baseline_path: &Path, candidate: &BenchRecord, tolerance_pct: u64, json: bool) -> i32 {
    let baseline = match load_baseline(baseline_path, candidate) {
        Ok(Some(baseline)) => baseline,
        Ok(None) => return EXIT_INCOMPARABLE,
        Err(e) => {
            eprintln!("baseline {}/: {e:#}", baseline_path.display());
            return EXIT_UNMEASURED;
        }
    };
    match bench::compare(&baseline, candidate, tolerance_pct) {
        Ok(regressions) if regressions.is_empty() => {
            verdict(
                json,
                &format!(
                    "PASS: {} within {tolerance_pct}% of the baseline at every percentile.",
                    short_identity(candidate)
                ),
            );
            EXIT_PASS
        }
        Ok(regressions) => {
            verdict(json, &format!("FAIL: {}", short_identity(candidate)));
            for regression in regressions {
                verdict(json, &format!("  {regression}"));
            }
            EXIT_REGRESSION
        }
        Err(reason) => {
            eprintln!("SKIP: {reason}");
            EXIT_INCOMPARABLE
        }
    }
}

fn verdict(json: bool, text: &str) {
    if json {
        eprintln!("{text}");
    } else {
        println!("{text}");
    }
}

fn short_identity(record: &BenchRecord) -> String {
    format!(
        "{} over {} frames of {}x{} @ {} bit/s",
        record.backend, record.frames, record.width, record.height, record.bitrate_bps
    )
}

fn print_human(record: &BenchRecord) {
    let tail = &record.encode_ms;
    println!("{} on {}", short_identity(record), record.host);
    println!(
        "  p50 {} ms  p90 {} ms  p95 {} ms  p99 {} ms  max {} ms  (n={}, mean {} ms)",
        tail.p50_ms, tail.p90_ms, tail.p95_ms, tail.p99_ms, tail.max_ms, tail.count, tail.mean_ms
    );
    println!(
        "  {:.1} s wall, {:.2} MiB out, {} keyframes, {} frames still buffered at the end",
        record.elapsed_ms as f64 / 1000.0,
        record.output_bytes as f64 / (1024.0 * 1024.0),
        record.keyframes,
        record.frames_without_packet
    );
}

fn write_record(path: &Path, record: &BenchRecord) -> Result<()> {
    let path = record_target(path, record);
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(record).context("serialising the record")?;
    std::fs::write(&path, format!("{json}\n"))
        .with_context(|| format!("writing {}", path.display()))?;
    eprintln!("record written to {}", path.display());
    Ok(())
}

/// Where a `--record` path actually lands. A directory (existing, or a path with
/// no extension) gets one file per backend+host, so a box can accumulate its
/// baselines without inventing filenames — the whole point is that the next run
/// on this machine finds this record by itself.
fn record_target(path: &Path, record: &BenchRecord) -> PathBuf {
    let is_dir = path.is_dir() || path.extension().is_none();
    if !is_dir {
        return path.to_path_buf();
    }
    path.join(format!(
        "{}.json",
        slug(&format!("{} {}", record.backend, record.host))
    ))
}

/// Lowercase, alphanumeric runs joined by single dashes. A host identity is a
/// CPU model string full of spaces, parens and commas, none of which belong in a
/// filename.
fn slug(input: &str) -> String {
    let mut out = String::new();
    let mut pending = false;
    for c in input.chars() {
        if c.is_ascii_alphanumeric() {
            if pending {
                out.push('-');
                pending = false;
            }
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() {
            pending = true;
        }
    }
    out
}

/// Load the baseline to compare against.
///
/// A file is taken at face value (then host-checked). A directory is a set of
/// per-machine records, and the right one matches this run's host *and* the
/// encoder that actually ran — a box that has both a software and a VAAPI
/// baseline must not have its VAAPI run graded against libx264. `Ok(None)` means
/// "nothing here matches", which the caller reports as a skip.
fn load_baseline(path: &Path, candidate: &BenchRecord) -> Result<Option<BenchRecord>> {
    if path.is_file() {
        let record = read_record(path)?;
        if record.host != candidate.host {
            eprintln!(
                "SKIP: {}/ is a baseline for [{}], this host is [{}].",
                path.display(),
                record.host,
                candidate.host
            );
            return Ok(None);
        }
        return Ok(Some(record));
    }
    if !path.is_dir() {
        anyhow::bail!("{} does not exist", path.display());
    }
    let entries = std::fs::read_dir(path).with_context(|| format!("reading {}", path.display()))?;
    let mut on_file = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let record = read_record(&path)?;
        if record.host == candidate.host && record.backend == candidate.backend {
            return Ok(Some(record));
        }
        on_file.push(format!("{} on [{}]", record.backend, record.host));
    }
    eprintln!(
        "SKIP: no baseline in {} matches this run ({} on [{}]). On file: {}.",
        path.display(),
        candidate.backend,
        candidate.host,
        if on_file.is_empty() {
            "none (no records)".into()
        } else {
            on_file.join("; ")
        }
    );
    eprintln!(
        "  add one with: linux-link bench --record {}",
        path.display()
    );
    Ok(None)
}

fn read_record(path: &Path) -> Result<BenchRecord> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use linux_link_core::metrics::Summary;
    use linux_link_core::streaming::bench::RECORD_SCHEMA;

    fn record(host: &str, backend: &str) -> BenchRecord {
        BenchRecord {
            schema: RECORD_SCHEMA,
            unix_secs: 0,
            backend: backend.into(),
            width: 1280,
            height: 720,
            fps: 30,
            bitrate_bps: 5_000_000,
            preset: "veryfast".into(),
            codec: "h264".into(),
            frames: 300,
            warmup: 30,
            host: host.into(),
            encode_ms: Summary {
                count: 270,
                mean_ms: 4,
                p50_ms: 4,
                p90_ms: 6,
                p95_ms: 8,
                p99_ms: 8,
                max_ms: 8,
            },
            elapsed_ms: 1_000,
            output_bytes: 500_000,
            keyframes: 5,
            frames_without_packet: 0,
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ll-bench-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, name: &str, record: &BenchRecord) {
        std::fs::write(dir.join(name), serde_json::to_string(record).unwrap()).unwrap();
    }

    fn candidate(host: &str, backend: &str) -> BenchRecord {
        record(host, backend)
    }

    #[test]
    fn a_directory_baseline_is_matched_by_host() {
        let dir = temp_dir("dir");
        write(&dir, "other.json", &record("Ryzen / 6.9", "in-process"));
        write(&dir, "mine.json", &record("this box / 7.2", "in-process"));
        std::fs::write(dir.join("notes.txt"), b"not a record").unwrap();

        let found = load_baseline(&dir, &candidate("this box / 7.2", "in-process")).unwrap();
        assert_eq!(found.map(|r| r.host), Some("this box / 7.2".into()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_directory_baseline_prefers_the_matching_backend() {
        let dir = temp_dir("backend");
        write(&dir, "sw.json", &record("this box / 7.2", "in-process"));
        write(
            &dir,
            "hw.json",
            &record("this box / 7.2", "in-process-vaapi"),
        );
        let hw = load_baseline(&dir, &candidate("this box / 7.2", "in-process-vaapi")).unwrap();
        assert_eq!(hw.map(|r| r.backend), Some("in-process-vaapi".into()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unmatched_host_skips_rather_than_fails() {
        let dir = temp_dir("nomatch");
        write(&dir, "other.json", &record("Ryzen / 6.9", "in-process"));
        assert!(
            load_baseline(&dir, &candidate("this box / 7.2", "in-process"))
                .unwrap()
                .is_none()
        );
        // The skip names what is on file, so the fix is obvious from the log.
        assert!(
            load_baseline(&dir, &candidate("Intel / 7.2", "in-process"))
                .unwrap()
                .is_none()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_explicit_baseline_is_still_host_checked() {
        let dir = temp_dir("explicit");
        let file = dir.join("baseline.json");
        write(&dir, "baseline.json", &record("Ryzen / 6.9", "in-process"));
        assert!(
            load_baseline(&file, &candidate("this box / 7.2", "in-process"))
                .unwrap()
                .is_none()
        );
        assert!(
            load_baseline(&file, &candidate("Ryzen / 6.9", "in-process"))
                .unwrap()
                .is_some()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_baseline_is_an_error_not_a_silent_pass() {
        let dir = temp_dir("missing");
        assert!(load_baseline(&dir.join("nope.json"), &candidate("any", "in-process")).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_corrupt_baseline_says_which_file() {
        let dir = temp_dir("corrupt");
        std::fs::write(dir.join("bad.json"), b"{ not json").unwrap();
        let err = load_baseline(&dir, &candidate("any", "in-process"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("bad.json"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn gate_maps_a_tail_regression_to_the_failure_code() {
        let dir = temp_dir("gate");
        write(&dir, "b.json", &record("here / 1", "in-process"));
        let candidate = candidate("here / 1", "in-process");
        assert_eq!(gate(&dir, &candidate, 25, false), EXIT_PASS);

        let mut slower = candidate.clone();
        slower.encode_ms.p95_ms = 40;
        assert_eq!(gate(&dir, &slower, 25, false), EXIT_REGRESSION);

        // A different encoder on this box has no baseline in the directory: skip,
        // never a pass.
        let mut other_backend = candidate.clone();
        other_backend.backend = "in-process-vaapi".into();
        assert_eq!(gate(&dir, &other_backend, 25, false), EXIT_INCOMPARABLE);
        // Pointed at the software file explicitly, the mismatch is caught by the
        // comparison itself rather than by the search.
        assert_eq!(
            gate(&dir.join("b.json"), &other_backend, 25, false),
            EXIT_INCOMPARABLE
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_fastest_of_several_runs_is_the_one_kept() {
        let mut slow = record("here / 1", "in-process");
        slow.encode_ms.p95_ms = 45;
        slow.encode_ms.p50_ms = 18;
        let fast = record("here / 1", "in-process");
        // Order must not matter, and a single run is taken as it stands.
        assert_eq!(
            least_contaminated(vec![slow.clone(), fast.clone()])
                .unwrap()
                .encode_ms
                .p95_ms,
            8
        );
        assert_eq!(
            least_contaminated(vec![fast.clone(), slow])
                .unwrap()
                .encode_ms
                .p95_ms,
            8
        );
        assert_eq!(least_contaminated(vec![fast]).unwrap().encode_ms.p50_ms, 4);
        assert!(least_contaminated(Vec::new()).is_none());
    }

    #[test]
    fn a_record_round_trips_through_the_file_it_is_written_as() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("r.json");
        write_record(&path, &record("here / 1", "in-process")).unwrap();
        let back = read_record(&path).unwrap();
        assert_eq!(back.host, "here / 1");
        assert_eq!(back.encode_ms.p99_ms, 8);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_record_dropped_in_a_directory_is_found_again_by_host() {
        let dir = temp_dir("autoname");
        let host = "12th Gen Intel(R) Core(TM) i7-12700H / 7.2.5-3-omarchy";
        // --record bench/baselines with no filename still lands somewhere a
        // later run can find without a human remembering what it was called.
        write_record(&dir, &record(host, "in-process")).unwrap();
        let found = load_baseline(&dir, &candidate(host, "in-process"))
            .unwrap()
            .expect("found");
        assert_eq!(found.backend, "in-process");
        assert!(
            slug(&format!("{} {}", found.backend, found.host))
                .starts_with("in-process-12th-gen-intel-r-core-tm-i7-12700h")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_explicit_filename_is_never_rewritten() {
        let dir = temp_dir("explicit-name");
        assert_eq!(
            record_target(&dir.join("nightly.json"), &record("h", "in-process")),
            dir.join("nightly.json")
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
