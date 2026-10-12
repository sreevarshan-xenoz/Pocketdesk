# Pocketdesk Desktop App Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Rust + egui desktop companion (tray, status, QR pairing, devices, sessions, settings, logs), plus the phone side of QR pairing.

**Architecture:**
- The server's local-file logic (paths, config, PIN file, live-session mirror, kick request, liveness, identity) moves into a new `core::local` module. The server re-exports it and the new `desktop/` crate uses it, so the app never links the server.
- The server additionally writes `endpoint.json`.
- The phone gains a `pocketdesk://pair` link parser, a deep link, a CameraX + ML Kit scanner, and certificate pinning before the first connection.

**Tech Stack:** Rust 2024; eframe 0.36 (glow), tray-icon 0.26, qrcode 0.14, toml_edit, sha2, base64; Kotlin/Compose; CameraX 1.4; ML Kit barcode-scanning 17.3 (bundled model).

**Spec:** `docs/superpowers/specs/2026-10-12-pocketdesk-desktop-app-design.md`

## Global Constraints

- **Identifiers do not change.** The binary is `linux-link`. The state dir is `<state|local data>/linux-link`, config is `<config>/linux-link/config.toml`, the Android package is `dev.linuxlink.android`, and the packet types are `kdeconnect.linuxlink.*`.
- **User-facing name** is "Pocketdesk". The app binary is `pocketdesk`.
- **Builds and gates:**
  - The Windows build is `x86_64-pc-windows-gnu`.
  - Windows gates are `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --exclude linux-link-android-bridge --exclude iroh-spike -- -D warnings`, and `cargo test --workspace --exclude linux-link-android-bridge --exclude iroh-spike`.
  - The Android gates are `assembleDebug` + `lintDebug`, adding no finding in a changed file.
- `core::local` must not depend on tokio, quinn, iroh or anything Linux-only.
- **Secrets.** The PIN is minted only from the OS RNG and written owner-only (`secure_fs::write_private`). Never commit state files.
- **Safe actions.** The app never runs power or lock actions and never stops the server unless the user clicks Restart. Tests never stop the developer's server.
- **Commits.** One logical change per commit, staged by name.

## Review Focus

1. **A QR scanned after its PIN expired** must say "code expired — show a new one" on the phone, not fail silently. The phone-pairing task maps the server's `pair:false` to that message, and the PIN tests pin the 300 s TTL.
2. **A desktop with virtual adapters** (WSL `172.x`, Hyper-V, Docker, link-local) must still pair. `PairLink` keeps every non-loopback, non-link-local IPv4 and orders private LAN ranges first and Tailscale `100.64/10` last. The phone tries each address with a short timeout. Both orderings are tested.
3. **Settings saved while the server runs** must say a restart is needed. A save must keep comments and unknown keys, so an unchanged save is byte-identical (tested).
4. **The app revoking a device while the server also writes `trusted_devices.json`.** TrustStore writes become atomic (write a temp file, then rename), so neither side reads a half-written file (tested).
5. **A stale `live_sessions.json` after a server crash** must not show phantom sessions. The app trusts the mirror only when `server.pid` names a live process and `updated_unix` is under 5 s old (tested in `sessions.rs`).

---

## File structure

| Path | Responsibility |
|---|---|
| `core/src/streaming/capture_backend.rs` (new) | `CaptureBackend` enum, moved out of the `capture`-gated `capture.rs`. |
| `core/src/local/mod.rs` (new) | `pub mod paths, config, pin, sessions, server, identity, pair_link;` |
| `core/src/local/paths.rs` | `config_dir()`, `state_dir()`, and the file-name constants. |
| `core/src/local/config.rs` | `Config` (moved from the server), `load_from`, `save_to` (toml_edit). |
| `core/src/local/pin.rs` | PIN mint, write and read; TTL. |
| `core/src/local/sessions.rs` | `LiveMirror` parse with freshness; kick request write and parse. |
| `core/src/local/server.rs` | Liveness from `server.pid`; the pid helpers. |
| `core/src/local/identity.rs` | Cert fingerprint, `endpoint.json` read/write, desktop device id. |
| `core/src/local/pair_link.rs` | `PairLink` encode/decode (`pocketdesk://pair?...`), address ordering. |
| `server/src/{config,state,live_sessions}.rs` | Become thin re-exports and adapters over `core::local`. |
| `server/src/iroh_endpoint.rs` | Writes `endpoint.json` once the endpoint is up. |
| `desktop/` (new crate) | `main.rs` (single instance, launch), `app.rs` (tabs), `model.rs` (pure state), `tray.rs`, `pair_view.rs` (QR), `restart.rs`. |
| `android/app/.../pair/PairLink.kt` | Parse the link; JVM unit test. |
| `android/app/.../pair/QrScanScreen.kt` | CameraX preview + ML Kit analyzer. |
| `android/bridge/src/api.rs`, `lib.rs` | `pin_peer_certificate(address, port, fp_hex)`. |

---

### Task 1: `core::local` foundation — paths and config (moved, now savable)

**Files:**
- Create: `core/src/streaming/capture_backend.rs`, `core/src/local/{mod.rs,paths.rs,config.rs}`
- Modify:
  - `core/src/streaming/capture.rs` — delete the enum, `pub use super::capture_backend::CaptureBackend;`
  - `core/src/streaming/mod.rs` — `pub mod capture_backend; pub use capture_backend::CaptureBackend;`
  - `core/src/lib.rs` — `#[cfg(feature = "local")] pub mod local;`
  - `core/Cargo.toml`, `server/Cargo.toml`, `server/src/config.rs`

**Interfaces:**
- Produces:
  - `linux_link_core::streaming::CaptureBackend` (unchanged path, now ungated).
  - `local::paths::{config_dir() -> io::Result<PathBuf>, state_dir() -> io::Result<PathBuf>, CONFIG_FILE, PID_FILE, PIN_FILE, TRUST_FILE, LIVE_FILE, KICK_FILE, ENDPOINT_FILE, LOG_DIR}`.
  - `local::config::{Config, Config::load_from(&Path) -> Result<Config, ConfigError>, Config::save_to(&self, &Path) -> Result<(), ConfigError>, Config::load() / save()}`.
  - Server: `linux_link_server::config::Config` (re-export).

- [ ] **Step 1: Move `CaptureBackend`.**
  - Cut the enum, its `impl` (`ALL`, `LINUX`, `config_key`) and its doc comment from `capture.rs` into `capture_backend.rs`.
  - Add `Serialize` to the derive.
  - Re-export it from `capture.rs` and `streaming/mod.rs`.
  - Build: `cargo build -p linux-link-core --no-default-features --features client` must now compile `CaptureBackend`.

- [ ] **Step 2: Cargo.**
  - In `core/Cargo.toml`: `local = ["dep:toml", "dep:toml_edit"]`, with optional deps `toml = { version = "0.8", optional = true }` and `toml_edit = { version = "0.22", optional = true }`. `dirs` and `sha2` are already non-optional.
  - Server: `linux-link-core = { ..., features = ["wan", "local"] }`.

- [ ] **Step 3: Write the failing tests** (`core/src/local/config.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-local-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("config.toml")
    }

    #[test]
    fn a_missing_file_is_the_defaults() {
        let path = temp("missing");
        let config = Config::load_from(&path).unwrap();
        assert_eq!(config.control_port, 1716);
        assert!(config.pairing_required);
    }

    #[test]
    fn saving_changes_only_the_edited_keys_and_keeps_comments() {
        let path = temp("edit");
        let original = "# my notes\ncontrol_port = 1800 # keep\nfuture_key = \"x\"\n";
        std::fs::write(&path, original).unwrap();
        let mut config = Config::load_from(&path).unwrap();
        config.save_to(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original, "unchanged save is byte-identical");
        config.allow_hevc = true;
        config.save_to(&path).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("# my notes") && saved.contains("future_key = \"x\""));
        assert!(saved.contains("allow_hevc = true"));
        assert_eq!(Config::load_from(&path).unwrap().control_port, 1800);
    }

    #[test]
    fn invalid_toml_is_an_error_not_defaults() {
        let path = temp("bad");
        std::fs::write(&path, "control_port = [").unwrap();
        assert!(matches!(Config::load_from(&path), Err(ConfigError::Parse { .. })));
    }

    #[test]
    fn capture_backend_defaults_to_auto_and_parses_override() {
        let default: Config = toml::from_str("").unwrap();
        assert!(matches!(default.capture_backend, CaptureBackend::Auto));
        let pinned: Config = toml::from_str("capture_backend = \"portal\"").unwrap();
        assert!(matches!(pinned.capture_backend, CaptureBackend::Portal));
        assert!(toml::from_str::<Config>("capture_backend = \"v4l2\"").is_err());
    }
}
```

- [ ] **Step 4: Run** `cargo test -p linux-link-core --features local local::config`. Expected: it fails to compile (`Config` is not defined).

- [ ] **Step 5: Implement.**

`paths.rs`:

```rust
//! Where the server and its companion app keep their files. Both resolve the
//! same way, so whatever one writes the other reads.
use std::io;
use std::path::PathBuf;

pub const CONFIG_FILE: &str = "config.toml";
pub const PID_FILE: &str = "server.pid";
pub const PIN_FILE: &str = "pairing.pin";
pub const TRUST_FILE: &str = "trusted_devices.json";
pub const LIVE_FILE: &str = "live_sessions.json";
pub const KICK_FILE: &str = "kick";
pub const ENDPOINT_FILE: &str = "endpoint.json";
pub const LOG_DIR: &str = "logs";
pub const CERT_FILE: &str = "certs/identity.der";
pub const DEVICE_ID_FILE: &str = "device_id";

fn missing(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("no {what} directory on this system"))
}

/// `<config dir>/linux-link` (`%APPDATA%\linux-link`, `~/.config/linux-link`).
pub fn config_dir() -> io::Result<PathBuf> {
    Ok(dirs::config_dir().ok_or_else(|| missing("config"))?.join("linux-link"))
}

/// `<state dir>/linux-link`, falling back to local data (`%LOCALAPPDATA%` on Windows).
pub fn state_dir() -> io::Result<PathBuf> {
    Ok(dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .ok_or_else(|| missing("state"))?
        .join("linux-link"))
}
```

`config.rs`:
- Move the struct, its defaults and its doc comments from `server/src/config.rs`.
- Derive `Serialize` and `PartialEq`. Use `Option<String>` with `#[serde(skip_serializing_if = "Option::is_none")]`.
- `ConfigError { Io{path,source}, Parse{path,message} }` via `thiserror` (already a core dependency).
- `save_to`:
  1. Parse the existing file, or an empty one, as a `toml_edit::DocumentMut`.
  2. Serialize `self` to a `toml::Table`.
  3. For each key whose value differs from the document's parsed value, set `doc[key] = toml_edit::value(...)`, converting via `toml_edit::Item` from the serialized string, so a key that is equal keeps its formatting and trailing comment.
  4. Write only when the bytes changed, through `crate::secure_fs::write_private`.
- `load()` / `save()` use `paths::config_dir()?.join(CONFIG_FILE)`.

`server/src/config.rs`: `pub use linux_link_core::local::config::Config;`. Delete its moved test.

- [ ] **Step 6: Run** the core tests (`--features local`) and `cargo test -p linux-link-server`. Then run the Windows gates and the `client` profile clippy: `cargo clippy -p linux-link-core --no-default-features --features client -- -D warnings`.

- [ ] **Step 7: Commit** `refactor(core): move config and paths into core::local and make config savable`.

### Task 2: PIN file in `core::local::pin`

**Files:** Create `core/src/local/pin.rs`. Modify:
- `server/src/service.rs` (`pair()` uses `pin::write`; `generate_pin` becomes `pin::mint_value`);
- `server/src/plugins/pair.rs` (`cli_pin` uses `pin::read`).

**Interfaces:**
- Produces:
  - `pin::PIN_TTL: Duration = 300s`.
  - `pin::mint_value() -> String`.
  - `pin::write(state_dir: &Path, pin: &str, now_unix: u64, grant_secs: Option<u64>) -> io::Result<()>`.
  - `pin::read(state_dir: &Path, now_unix: u64) -> Option<FilePin>`, where `FilePin { pin: String, minted_unix: Option<u64>, grant_secs: Option<u64> }`.
  - `pin::remaining(&FilePin, now_unix) -> Option<Duration>`.

- [ ] **Step 1: Failing tests.**

```rust
#[test]
fn a_written_pin_reads_back_until_its_ttl() {
    let dir = temp_dir("pin");
    write(&dir, "123456", 1_000, None).unwrap();
    let read_back = read(&dir, 1_100).unwrap();
    assert_eq!(read_back.pin, "123456");
    assert_eq!(remaining(&read_back, 1_100), Some(Duration::from_secs(200)));
    assert!(read(&dir, 1_000 + 300).is_none(), "expired at exactly the TTL");
}

#[test]
fn the_grant_line_survives_and_legacy_files_without_a_stamp_never_expire() {
    let dir = temp_dir("grant");
    write(&dir, "000042", 50, Some(3600)).unwrap();
    assert_eq!(read(&dir, 60).unwrap().grant_secs, Some(3600));
    std::fs::write(dir.join(PIN_FILE), "654321\n").unwrap();
    assert!(read(&dir, u64::MAX / 2).is_some());
}

#[test]
fn minted_pins_are_six_digits() {
    for _ in 0..64 {
        let pin = mint_value();
        assert!(pin.len() == 6 && pin.bytes().all(|b| b.is_ascii_digit()));
    }
}
```

Move the server's existing `pins_are_six_digits_and_do_not_follow_the_clock` test with `mint_value`. `rand` becomes a core dependency of `local` (already non-optional in core).

- [ ] **Step 2: Run** it and watch it fail. **Step 3:** implement, keeping the exact file format `"<pin>\n<unix>\n[<grant>\n]"`. **Step 4:** run the core and server suites (`cli_pin_format_roundtrip` and `expired_pin_is_rejected` in `pair.rs` must still pass). **Step 5: Commit** `refactor(core): share the pairing PIN file through core::local::pin`.

### Task 3: Live sessions, kick requests, liveness, atomic trust store

**Files:**
- Create `core/src/local/{sessions.rs,server.rs}`.
- Modify `server/src/live_sessions.rs` (use the shared render/parse), `server/src/service.rs` (`kick` uses `sessions::request_kick`), and `core/src/protocol/kdeconnect.rs` (TrustStore `save`: tmp + rename).

**Interfaces:**
- Produces:
  - `sessions::{LiveMirror { updated_unix: u64, sessions: Vec<MirrorSession> }, MirrorSession { device_id: Option<String>, peer: String, transport: String, started_unix: u64 }, parse_mirror(&str) -> Option<LiveMirror>, LiveMirror::is_fresh(now_unix) -> bool (≤ 5 s), request_kick(state_dir, target, now_unix) -> io::Result<()>, parse_kick(raw, now_unix) -> Option<String>, KICK_TTL_SECS = 60}`.
  - `server::{server_pid(state_dir) -> Option<u32>, is_running(state_dir) -> bool}`. `is_running` is true only when the pid file parses and the process exists, with an image name starting `linux-link`:
    - Windows: `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `QueryFullProcessImageNameW`.
    - Linux: `/proc/<pid>/exe` file name.
    - Other OSes: pid file only.

- [ ] **Step 1: Failing tests.**

```rust
#[test]
fn a_mirror_older_than_five_seconds_is_not_live() {
    let raw = r#"{"updated_unix":100,"sessions":[{"device_id":"abc","peer":"10.0.0.2:5","transport":"lan","started_unix":90}]}"#;
    let mirror = parse_mirror(raw).unwrap();
    assert!(mirror.is_fresh(104) && !mirror.is_fresh(106));
    assert_eq!(mirror.sessions[0].device_id.as_deref(), Some("abc"));
}

#[test]
fn a_kick_request_round_trips_and_expires() {
    let dir = temp_dir("kick");
    request_kick(&dir, "all", 1_000).unwrap();
    let raw = std::fs::read_to_string(dir.join(KICK_FILE)).unwrap();
    assert_eq!(parse_kick(&raw, 1_030).as_deref(), Some("all"));
    assert_eq!(parse_kick(&raw, 1_061), None);
}

#[test]
fn a_pid_file_naming_no_live_process_is_not_running() {
    let dir = temp_dir("pid");
    std::fs::write(dir.join(PID_FILE), "4294967").unwrap();
    assert!(!is_running(&dir));
}

#[test]
fn the_trust_store_never_leaves_a_half_written_file() {
    // save() writes `<file>.tmp` then renames; no .tmp remains afterwards.
    let dir = temp_dir("trust");
    let path = dir.join(TRUST_FILE);
    let mut store = TrustStore::load_or_create(&path).unwrap();
    store.trust_device("phone").unwrap();
    assert!(path.exists() && !dir.join("trusted_devices.json.tmp").exists());
}
```

- [ ] **Steps 2–4:** fail, implement (move `render_json`'s types and `parse_kick_file` from the server, which keeps a call into the shared versions), pass. The server tests for `live_sessions` and `kick` must stay green.
- [ ] **Step 5: Commit** `refactor(core): share live sessions, kick requests and liveness; write the trust store atomically`.

### Task 4: Identity, `endpoint.json`, and `PairLink`

**Files:** Create `core/src/local/{identity.rs,pair_link.rs}`. Modify `server/src/iroh_endpoint.rs`, which calls `identity::write_endpoint(state_dir, &body)` after `endpoint_packet` builds the body. Add `base64 = "0.22"` (already a core dependency) to `local`.

**Interfaces:**
- Produces:
  - `identity::cert_fingerprint(state_dir) -> io::Result<[u8; 32]>` (SHA-256 of `certs/identity.der`, the hash `TofuVerifier` pins).
  - `identity::fingerprint_hex([u8;32]) -> String`.
  - `identity::write_endpoint(state_dir, &serde_json::Value) -> io::Result<()>` and `identity::read_endpoint(state_dir) -> Option<serde_json::Value>`.
  - `identity::device_id(state_dir) -> Option<String>`.
  - `pair_link::PairLink { name, device_id, addresses: Vec<String>, control_port: u16, stream_port: u16, pin: String, fingerprint: Option<String>, endpoint: Option<String /* JSON */> }`, with:
    - `PairLink::to_uri() -> String`;
    - `PairLink::from_uri(&str) -> Result<PairLink, PairLinkError>`;
    - `pair_link::order_addresses(Vec<String>) -> Vec<String>`.

- [ ] **Step 1: Failing tests.**

```rust
#[test]
fn a_link_round_trips_through_its_uri() {
    let link = PairLink {
        name: "Desk & Co".into(), device_id: "d-1".into(),
        addresses: vec!["192.168.1.5".into(), "100.80.1.2".into()],
        control_port: 1716, stream_port: 4716, pin: "012345".into(),
        fingerprint: Some("ab".repeat(32)),
        endpoint: Some(r#"{"endpointId":"x","relayUrls":[],"directAddrs":[]}"#.into()),
    };
    let uri = link.to_uri();
    assert!(uri.starts_with("pocketdesk://pair?v=1&"));
    assert_eq!(PairLink::from_uri(&uri).unwrap(), link);
}

#[test]
fn virtual_adapters_and_link_local_are_ordered_or_dropped() {
    let ordered = order_addresses(vec![
        "100.82.1.9".into(), "172.20.240.1".into(), "169.254.3.4".into(),
        "192.168.1.20".into(), "127.0.0.1".into(), "fe80::1".into(),
    ]);
    assert_eq!(ordered, ["192.168.1.20", "172.20.240.1", "100.82.1.9"]);
}

#[test]
fn malformed_links_are_rejected_with_a_reason() {
    assert!(PairLink::from_uri("https://example.com").is_err());
    assert!(PairLink::from_uri("pocketdesk://pair?v=2&pin=123456").is_err());
    assert!(PairLink::from_uri("pocketdesk://pair?v=1&a=1.2.3.4&pin=12").is_err());
}

#[test]
fn the_fingerprint_is_sha256_of_the_der_file() {
    let dir = temp_dir("fp");
    std::fs::create_dir_all(dir.join("certs")).unwrap();
    std::fs::write(dir.join(CERT_FILE), b"abc").unwrap();
    assert_eq!(
        fingerprint_hex(cert_fingerprint(&dir).unwrap()),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
```

- [ ] **Steps 2–4:** fail, implement, pass.
  - Query keys: `v name id a cp sp pin fp ep`.
  - Values are percent-encoded by hand (RFC 3986 unreserved stays). `ep` is base64url, no padding.
  - Order: RFC1918 `192.168/16`, then `10/8`, then `172.16/12`, then Tailscale `100.64/10`. Drop loopback, link-local and IPv6.
- [ ] **Step 5: Commit** `feat(core): pairing links, certificate fingerprint and the endpoint file; the server writes endpoint.json`.

### Task 5: Desktop app — window, tray, status, devices, sessions, logs

**Files:**
- Create `desktop/Cargo.toml` and `desktop/src/{main.rs,app.rs,model.rs,tray.rs}`.
- Modify the root `Cargo.toml` (workspace member `desktop`).

**Interfaces:**
- Consumes:
  - `core::local::{paths, config, sessions, server, pin, identity, pair_link}`;
  - `TrustStore::{load_or_create, trusted_devices, grant_remaining, untrust_device}`.
- Produces:
  - `model::Snapshot { running: bool, devices: Vec<Device>, sessions: Vec<MirrorSession>, log_tail: Vec<String> }`;
  - `model::snapshot(state_dir, now_unix) -> Snapshot`, which is `snapshot_with(state_dir, now_unix, server::is_running(state_dir))`;
  - `model::snapshot_with(state_dir: &Path, now_unix: u64, running: bool) -> Snapshot`;
  - `model::log_tail(state_dir: &Path, lines: usize) -> Vec<String>`;
  - `model::Device { id: String, grant_remaining: Option<u64>, connected: bool }`.

- [ ] **Step 1: Failing tests** (pure, `model.rs`):

```rust
#[test]
fn a_dead_server_shows_no_sessions_even_with_a_mirror_left_behind() {
    let dir = temp_dir("snap");
    std::fs::write(dir.join(LIVE_FILE), r#"{"updated_unix":10,"sessions":[{"device_id":"p","peer":"1.2.3.4:5","transport":"lan","started_unix":1}]}"#).unwrap();
    let snap = snapshot(&dir, 11);
    assert!(!snap.running && snap.sessions.is_empty());
}

#[test]
fn devices_list_permanent_and_granted_and_mark_the_connected_one() {
    let dir = temp_dir("devices");
    let mut store = TrustStore::load_or_create(&dir.join(TRUST_FILE)).unwrap();
    store.trust_device("a").unwrap();
    store.trust_device_with_ttl("b", Duration::from_secs(60)).unwrap();
    std::fs::write(dir.join(LIVE_FILE), r#"{"updated_unix":100,"sessions":[{"device_id":"a","peer":"1.2.3.4:5","transport":"lan","started_unix":90}]}"#).unwrap();
    // `snapshot_with` takes liveness as an argument so the test does not need a real server.
    let snap = snapshot_with(&dir, 101, true);
    let a = snap.devices.iter().find(|d| d.id == "a").unwrap();
    let b = snap.devices.iter().find(|d| d.id == "b").unwrap();
    assert!(a.connected && a.grant_remaining.is_none());
    assert!(!b.connected && b.grant_remaining.is_some_and(|s| s <= 60));
}

#[test]
fn the_log_tail_reads_the_newest_daily_file_last_200_lines() {
    let dir = temp_dir("logs");
    let logs = dir.join(LOG_DIR);
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(logs.join("linux-link.2026-10-11.log"), "old
").unwrap();
    let lines: String = (0..250).map(|i| format!("line {i}
")).collect();
    std::fs::write(logs.join("linux-link.2026-10-12.log"), lines).unwrap();
    let tail = log_tail(&dir, 200);
    assert_eq!(tail.len(), 200);
    assert_eq!(tail.first().map(String::as_str), Some("line 50"));
    assert_eq!(tail.last().map(String::as_str), Some("line 249"));
}
```

- [ ] **Step 2: Implement.**
  - `main.rs`:
    - Single instance: Windows uses a named mutex `Local\PocketdeskDesktop`; Linux uses a lock file in the state dir via `fs2`-style `flock` through `libc`.
    - Starts the tray (Windows: the same thread as winit, before `run_native`; Linux: a GTK thread, `cfg(target_os = "linux")`).
    - Runs eframe with a 1 s `request_repaint_after`.
  - Tabs: Status, Pair, Devices, Sessions, Settings, Logs.
  - Closing hides to the tray (`ViewportCommand::Visible(false)` + `CancelClose`). The tray has Open, Pair a phone…, and Quit.
- [ ] **Step 3:** run the unit tests; launch on Windows and screenshot each tab.
- [ ] **Step 4: Commit** `feat(desktop): Pocketdesk desktop app — status, devices, sessions, logs and tray`.

### Task 6: Pair tab (QR + PIN) and Settings tab

**Files:** Create `desktop/src/{pair_view.rs,settings_view.rs}`.

**Interfaces:**
- Consumes `pin::{mint_value, write, read, remaining}`, `identity::*`, `pair_link::*`, `Config::{load, save}`, and `core::tailscale::lan::get_local_ips()`.
- Produces `pair_view::current_link(state_dir: &Path, config: &Config, now_unix: u64) -> io::Result<PairLink>`, which reuses a live PIN file or else mints and writes a new one.

- [ ] **Step 1: Failing test:**

```rust
#[test]
fn the_link_carries_the_minted_pin_ports_and_fingerprint() {
    let dir = temp_dir("link");
    std::fs::create_dir_all(dir.join("certs")).unwrap();
    std::fs::write(dir.join(CERT_FILE), b"der").unwrap();
    identity::write_endpoint(&dir, &serde_json::json!({"endpointId": "e1"})).unwrap();
    let config = Config { control_port: 1800, streaming_port: 4800, ..Config::default() };
    let link = current_link(&dir, &config, 1_000).unwrap();
    assert_eq!(link.pin, pin::read(&dir, 1_000).unwrap().pin);
    assert_eq!((link.control_port, link.stream_port), (1800, 4800));
    assert_eq!(link.fingerprint.as_deref(), Some(identity::fingerprint_hex(identity::cert_fingerprint(&dir).unwrap()).as_str()));
    assert!(link.endpoint.as_deref().is_some_and(|e| e.contains("e1")));
}
```

- [ ] **Step 2: Implement.**
  - The QR is rendered with `qrcode` into an egui texture: 6 px modules plus a quiet zone.
  - The PIN is shown in large digits with a countdown. The code regenerates at expiry. A "Paired ✓ <device>" banner appears when a new trusted device shows up.
  - Settings is a form over `Config`. Save writes and shows "Restart the server to apply" plus a Restart button (Task 7).
- [ ] **Step 3: Commit** `feat(desktop): pair a phone from a QR code; edit settings`.

### Task 7: Start / restart the server

**Files:** Create `desktop/src/restart.rs`.

- Windows:
  - Start = run the scheduled task `Pocketdesk` (ITaskService `GetTask` → `Run`), else spawn `linux-link.exe start --background` from beside `pocketdesk.exe`, else from PATH.
  - Stop = signal the named stop event `Local\LinuxLinkStop` the server already listens on (`platform::request_stop` logic, re-implemented through `OpenEventW` + `SetEvent`).
- Linux:
  - Start: `systemctl --user start linux-link` when `systemctl --user cat linux-link` succeeds, else spawn `linux-link start --background`.
  - Stop: SIGTERM to the pid from `server.pid`.
- Restart = stop, then wait up to 10 s for `!is_running`, then start.

**Test:** the command selection is a pure function `start_plan(os, has_task, exe_next_to_app) -> StartPlan`, unit-tested. The actions are not run on the developer box (project rule); checklist §24 owes them.

Commit: `feat(desktop): start and restart the server from the app`.

### Task 8: Phone — PairLink parsing, deep link, pinned first connection

**Files:**
- Create `android/app/src/main/java/dev/linuxlink/android/pair/PairLink.kt` and `android/app/src/test/java/dev/linuxlink/android/pair/PairLinkTest.kt` (the first JVM test; add `testImplementation("junit:junit:4.13.2")`).
- Modify:
  - `AndroidManifest.xml` (an intent filter on MainActivity: `VIEW`, `BROWSABLE`, scheme `pocketdesk`, host `pair`);
  - `MainActivity.kt` (route the link to a "Pairing with <name>…" flow);
  - `android/bridge/src/{api.rs,lib.rs}` (`pin_peer_certificate`);
  - `RustCore.kt` (`pinPeerCertificate`).

**Interfaces:**
- `PairLink.parse(uri: String): Result<PairLink>` mirrors Task 4's keys and validation. It is tested against the exact URI the Rust test produced, copied into the test as a fixture.
- `RustCore.pinPeerCertificate(address: String, port: Int, fingerprintHex: String): Result<Unit>` writes the bridge's `known_peers.json` entry for the TLS server name `format!("{:?}", IpAddr)` (the key `TofuVerifier` uses).

The flow, for each address in order:
1. `pinPeerCertificate(addr, sp, fp)`;
2. `connectToPeer(addr, cp)` with a 3 s timeout;
3. `pairWithPin(addr, cp, pin)`;
4. on `pair:true`, save `HostStore.Host(addr, sp, cp)` and `saveWanIdentity(addr, ep)`, then open the session.

On `pair:false` the phone shows `R.string.pair_link_expired` ("This code has expired — show a new one on the desktop"). New strings are needed in en/es/ta.

On-device check: `adb shell am start -a android.intent.action.VIEW -d "<uri from the app>"`.

Commit: `feat(android): pair from a pocketdesk:// link with the desktop certificate pinned`.

### Task 9: Phone — QR scanner

**Files:**
- Create `ui/QrScanScreen.kt`.
- Modify `HomeScreen.kt` (a "Scan pairing code" button), `build.gradle.kts` (`androidx.camera:camera-camera2/lifecycle/view:1.4.2`, `com.google.mlkit:barcode-scanning:17.3.0`), and the manifest (`CAMERA` permission, `uses-feature android.hardware.camera required=false`).

- The screen asks for the camera permission, then binds Preview + ImageAnalysis (`STRATEGY_KEEP_ONLY_LATEST`).
- An ML Kit `BarcodeScanner` restricted to `FORMAT_QR_CODE` hands the first `pocketdesk://` value to Task 8's flow.
- Back cancels.

On-device check: the screen opens and the camera preview runs (scanning a real code is owed, because the USB-tethered phone cannot see the desktop screen).

Commit: `feat(android): scan a Pocketdesk pairing code with the camera`.

### Task 10: Docs, CI, status

- README: a "Desktop app" section.
- `docs/development-setup.md`: Linux build deps for the app (`libgtk-3-dev libayatana-appindicator3-dev libxdo-dev`).
- Checklist §24: the desktop app and QR pairing; the owed Linux tray, restart and camera scan.
- CI: add those apt packages to the Linux jobs (the workspace clippy now builds `desktop`); the release zip gains `pocketdesk.exe`.
- AGENTS: a status bullet.

Commit per file group: `docs: ...` and `ci: ...`.
