# File transfer B (desktop → phone, remote browser) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The phone browses the computer's home folder and downloads files from it. The desktop's owner sends files to a phone in a session. Every transfer, in both directions, ends with the receiver confirming the save.

**Architecture:** The desktop always dials the phone. A phone→desktop `file_fetch` names a browsed path or a desktop-issued offer id. The server resolves it, dials the transport's peer, presents the phone's key, and writes `u64 size + bytes`. The phone answers with one status byte. Desktop→phone sends start in the desktop app as an `outbox/<id>.json` request; the server turns it into a pushed `file_offer`, which the phone fetches.

**Tech Stack:**
- Rust: tokio, serde, egui 0.36, the `windows` crate.
- Kotlin: Compose Material3, MediaStore.
- JNI through `android/bridge`.

**Spec:** `docs/superpowers/specs/2026-10-13-file-transfer-b-design.md`

## Global Constraints

- **Limits.** Max file 8 GiB (`MAX_FILE_BYTES`). Two transfer slots, shared, refused when full, never queued. The deadline is `30 s + size / 32 KiB`, with a 30 s idle timeout per read and a 10 s connect timeout. The phone's listener accepts for 30 s.
- **Transfer key.** 32 lowercase hex digits (`Uuid::new_v4().simple()`), presented as `key\n` within 5 s.
- **Status byte.** `1` saved, `0` failed. The sender succeeds only on `1`. A missing byte (EOF or 10 s) is a failure.
- **Packet names.**
  - Unchanged: `kdeconnect.filebrowse.request`, `kdeconnect.filebrowse.response`.
  - New, under `kdeconnect.linuxlink.*`: `file_fetch` `{path?|offer?, port, key}`, `file_fetch_refused` `{key, reason}`, `file_offer` `{id, name, size}`.
- **Desktop dials only `DeviceSender::peer_addr`**, and only when `dialable(ip)` holds.
- **Offers.** Single use, expire after 120 s, valid only from the device they were offered to.
- **Outbox requests.** A request older than 60 s is dropped. Ids pass `approval::is_valid_id`.
- **Lists.** `sent.json` keeps the last 50.
- **Commits.** One logical change per commit (conventional, scoped), each with its gates. Stage by name.
- **Rust gates:**
  - `cargo fmt --all -- --check`;
  - `cargo clippy --workspace --all-targets --exclude linux-link-android-bridge --exclude iroh-spike -- -D warnings`;
  - `cargo test --workspace --exclude linux-link-android-bridge --exclude iroh-spike`.
- **Bridge gates:** `cargo clippy -p linux-link-android-bridge --all-targets -- -D warnings` and `cargo test -p linux-link-android-bridge`.
- **Kotlin gates:** `cd android && ./gradlew testDebugUnitTest assembleDebug lintDebug`, with no new lint finding in a changed file.
- User-facing text says "Pocketdesk" and "computer". Strings go in en/es/ta.

## Review Focus

1. **A file deleted or grown between listing and download.** The size header comes from the open file's metadata at send time, and `pump` must stop at exactly that many bytes. A shrunk file fails, never hangs. *Test:* server Task 4, `a_file_that_shrinks_fails_the_send`.
2. **An offer used twice, or by another phone.** The second fetch is refused with a reason, fast. *Test:* Task 5.
3. **A download into a full phone.** The phone writes status `0`, deletes the pending MediaStore row, and the desktop records the failure. *Test:* bridge Task 6, `a_failed_write_answers_zero`, with a writer that errors.
4. **Navigating to a folder that has become unreadable or vanished.** The browse reply carries `error`, and the sheet shows it with Up still available. *Test:* server Task 2, a missing folder gives an error reply that keeps `parent`.
5. **A request file for a phone with no session.** It is recorded as `failed: phone not connected` and not retried forever. *Test:* server Task 5.

---

### Task 1: core — outbox, sent list, permission row

**Files:**
- Create: `core/src/local/outbox.rs`, `core/src/local/sent.rs`
- Modify: `core/src/local/mod.rs`, `core/src/local/paths.rs` (`OUTBOX_DIR = "outbox"`, `SENT_FILE = "sent.json"`), `core/src/protocol/permissions.rs:65`

**Interfaces (Produces):**
```rust
// outbox.rs
pub const REQUEST_TTL_SECS: u64 = 60;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendRequest { pub id: String, pub device_id: String, pub path: String, pub created_unix: u64 }
pub fn request(state_dir: &Path, device_id: &str, path: &str, now_unix: u64) -> io::Result<SendRequest>; // id = uuid v4
pub fn take_all(state_dir: &Path, now_unix: u64) -> Vec<SendRequest>; // reads + deletes every *.json; drops stale/future/invalid/mislabelled
// sent.rs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub enum SentState { Offered, Sent, Failed(String) }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentFile { pub id: String, pub name: String, pub path: String, pub size: u64, pub device_id: String, pub sent_unix: u64, pub state: SentState }
pub const KEEP: usize = 50;
pub fn read(state_dir: &Path) -> Vec<SentFile>;            // oldest first
pub fn record(state_dir: &Path, file: SentFile) -> io::Result<()>;
pub fn update(state_dir: &Path, id: &str, state: SentState) -> io::Result<()>; // no-op if id gone
```
`capability_for("kdeconnect.linuxlink.file_fetch") == Some(Capability::Files)`.

- [ ] **Step 1: Tests**
  - outbox: a request round-trips through `take_all` and its file is gone; a request stamped 61 s ago, one from the future, and a file whose id differs from its name are all dropped *and deleted*; `request` with any device id never writes outside `outbox/`.
  - sent: record, update by id, keep 50, a corrupt file reads as empty.
  - permissions: the new row, plus the existing pair.rs exhaustive test still passing.
- [ ] **Step 2: Implement.** Mirror `received.rs` (whole-file replace via tmp and rename) and `approval.rs` (id validation, `write_atomic`).
- [ ] **Step 3: Gates, then commit** `feat(core): an outbox for files to send to a phone, and the sent list (F7b)`.

### Task 2: server — browse replies say where they are

**Files:** Modify `server/src/plugins/file_browse.rs`.

**Interfaces (Produces):**
```rust
/// The folder or file `requested` names inside `home`, in the form a request may name it (home-prefixed, not `\\?\`).
pub(crate) fn resolve_in(home: &Path, requested: &str) -> Option<(PathBuf /*real*/, PathBuf /*shown*/)>;
pub(crate) fn resolve_file(requested: &str) -> Result<(PathBuf, u64), String>; // regular file inside home, its size
```
- The reply is `{path: shown, parent: Option<shown> (null at home), files: [{name, isDirectory, size, modified, path}]}`.
- An error reply keeps `path` (the shown form when known, else the request) and `parent`.
- The shown form is `home.join(real.strip_prefix(home.canonicalize()))`, so that it passes the lexical check again.

- [ ] **Step 1: Tests**
  - Entries carry a `path` that `resolve_in` accepts again.
  - `parent` is `None` at home and home's shown form one level down.
  - `resolve_file` refuses a folder, a missing file, `..`, a path outside home and a symlink out (unix), and accepts a file with its size.
  - The listing sorts folders first, then by case-insensitive name.
- [ ] **Step 2: Implement.** Factor the reply building into a pure `fn browse_reply(home: &Path, requested: &str) -> serde_json::Value` so the tests need no sender.
- [ ] **Step 3: Gates, then commit** `fix(server): browsing answers with real paths, so the phone can open folders (F7b)`.

### Task 3: both ends — a transfer ends with the receiver's status byte

**Files:** Modify `server/src/plugins/share.rs` (`receive_file`), `android/bridge/src/api.rs` (`send_stream`).

**Interfaces (Produces):**
```rust
// share.rs
const STATUS_SAVED: u8 = 1; const STATUS_FAILED: u8 = 0;
// receive_file writes STATUS_SAVED after rename + mark_of_the_web, STATUS_FAILED on any error after the dial (best effort).
// api.rs
async fn read_status<R: AsyncRead + Unpin>(stream: &mut R) -> Result<(), String>; // 10 s; 1 → Ok, 0 → Err("the computer could not save it"), EOF/timeout → Err
```
- [ ] **Step 1: Tests**
  - Server: the fake phone reads exactly one byte `1` after the data, and `0` after a short transfer.
  - Bridge: a loopback fake desktop that reads the key and the bytes, then answers `1` (Ok), `0` (Err), or closes (Err).
- [ ] **Step 2: Implement.** After `pump`, `send_stream` keeps the socket open (no `shutdown`, since the size is known), calls `read_status`, then drops the socket.
- [ ] **Step 3: Gates, then commit** `fix: a file counts as sent only once the receiver says it saved it (F7)`.

### Task 4: server — the phone can fetch a browsed file

**Files:** Modify `server/src/plugins/share.rs`. Register `file_fetch` in its capabilities.

**Interfaces (Produces):**
```rust
/// Dial `to`, present `key`, write the size and the file, read the phone's status byte.
pub(crate) async fn send_file_to(to: SocketAddr, key: &str, path: &Path) -> Result<u64>;
async fn refuse(sender: &dyn DeviceSender, key: &str, reason: &str); // pushes kdeconnect.linuxlink.file_fetch_refused
```
- The `file_fetch` handler validates in this order, refusing with a reason at each step:
  1. The key is valid. With an invalid key, log only: there is nothing to address the refusal to.
  2. The port is set.
  3. `peer_addr` exists and is dialable.
  4. The target is either `path` (resolved by `file_browse::resolve_file`) or `offer` (Task 5).
  5. The size is at most `MAX_FILE_BYTES`.
  6. A slot is free.
- The send then runs in a spawned task: `send_file_to` under `transfer_deadline(size)`.
- [ ] **Step 1: Tests**
  - Over loopback, a fake phone listener checks the key, reads `u64` BE plus the bytes, and answers `1`; `send_file_to` returns the size.
  - A fake phone answering `0` makes it fail.
  - `a_file_that_shrinks_fails_the_send`: truncate the file after metadata is read. Simulate this by passing a reader shorter than the declared size to the inner `send_reader(to, key, reader, size)`.
- [ ] **Step 2: Implement.** `send_file_to` opens the file, takes `metadata().len()`, and calls `send_reader`.
- [ ] **Step 3: Gates, then commit** `feat(server): a phone can download a file it browsed to (F7b)`.

### Task 5: server — offers from the desktop app

**Files:**
- Create: `server/src/offers.rs`
- Modify: `server/src/service.rs` (spawn the watcher next to `live_sessions::run_watcher`), `server/src/main.rs`/`lib.rs` (mod), `server/src/plugins/pair.rs` (`pub fn phone_id_of(conn_key: &str) -> Option<String>`), `share.rs` (the offer branch).

**Interfaces (Produces):**
```rust
pub struct Offer { pub path: PathBuf, pub device_id: String, pub size: u64, pub expires: Instant }
pub fn offer(path: PathBuf, device_id: String, size: u64) -> String;        // id
pub fn redeem(id: &str, device_id: &str) -> Result<(PathBuf, String /*sent id*/), &'static str>; // single use; "unknown or used offer" / "offer expired" / "offer is for another device"
pub async fn run_watcher();                                                 // 250 ms poll of outbox::take_all
async fn deliver(request: SendRequest, dir: &Path);                         // validate, find sender, push, record
fn target_for(device_id: &str, clients: &[Arc<dyn DeviceSender>]) -> Option<Arc<dyn DeviceSender>>;
```
- **`deliver`:**
  - Checks that the path is a regular file of at most 8 GiB.
  - Checks that the device is trusted and allowed Files, via `TrustStore` `permissions().allows(Files)`.
  - Finds a target: a TCP client whose `phone_id_of(sender.device_id())` or own `device_id()` equals it, from `state::clone_clients()`.
  - Pushes `file_offer {id, name, size}` and records `Offered`.
  - Every refusal records `Failed(reason)`, e.g. `phone not connected` or `files are off for this phone`.
- **Outcome.** The fetch's spawned send updates the sent state to `Sent` or `Failed`.
- **Redeeming** compares the fetching connection's phone id: `pair::phone_id_of(conn_key)` falling back to `device_id()`.
- [ ] **Step 1: Tests**
  - `redeem` is single use, refused from another device and refused after expiry. Use an injectable clock: `redeem_at(id, device, now)`.
  - `target_for` matches a fake sender by phone id and returns `None` otherwise.
  - `deliver` into a temp state dir with no clients records `Failed("phone not connected")`.
- [ ] **Step 2: Implement.** A `Mutex<HashMap<String, Offer>>`, swept of expired offers on each call.
- [ ] **Step 3: Gates, then commit** `feat(server): the desktop app can offer a file to a phone in a session (F7b)`.

### Task 6: bridge — fetch, offers latch, JNI

**Files:** Modify `android/bridge/src/api.rs`, `android/bridge/src/session.rs` (`PENDING_OFFERS`), `android/bridge/src/lib.rs`, `android/app/.../bridge/RustCore.kt`.

**Interfaces (Produces):**
```rust
pub enum FetchTarget { Path(String), Offer(String) }
pub async fn fetch_file_fd(address: String, port: u16, target: FetchTarget, fd: i32) -> Result<u64, String>; // bytes saved
async fn receive_into<W: AsyncWrite + Unpin, S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S, out: &mut W) -> Result<u64, String>; // size header, pump with progress, fsync-free flush, status byte
pub fn take_file_offers() -> String; // JSON array [{id,name,size}]
```
- **JNI:**
  - `nativeFetchFile(address, port, kind: Int /*0 path,1 offer*/, value, fd) -> String` returns `{"ok": bytes}`.
  - `nativeTakeFileOffers() -> String`.
- **Kotlin:** `RustCore.fetchFile(address, port, isOffer, value, fd): Result<Long>` and `RustCore.takeFileOffers(): String`.
- **Control path.** Use the live `CONTROL_WRITER` only when `CONTROL_PEER` is this desktop; subscribe to `INCOMING_PACKETS` before sending, to see the refusal. Otherwise open a dedicated connection, read its lines for the refusal, and latch any `file_offer` seen there too.
- **Receiver writes.** It writes status `1` only after `out.flush()` and the full size. On any write error it writes `0`.
- **Progress.** The progress counters are reused for receiving: `TRANSFER_TOTAL` is set from the size header.
- [ ] **Step 1: Tests**
  - Over loopback, a fake desktop dials, presents the key, and writes size + bytes. The test asserts:
    - the bytes land in the writer;
    - the status `1` is read back;
    - progress runs to the total.
  - Failure cases:
    - a wrong key is dropped and the right one later is accepted;
    - a short stream errors and answers `0`;
    - `a_failed_write_answers_zero`, with a writer that errors;
    - the refusal path returns the reason, via a packet line matching the key.
  - The offer latch: dedup by id, bounded at 16.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Gates (bridge, plus an arm64 `.so` via `cargo ndk`), then commit** `feat(android): the bridge downloads a file from the computer into a descriptor (F7b)`.

### Task 7: Android — receiving in TransferService, saved to Downloads/Pocketdesk

**Files:**
- Modify: `TransferService.kt` (a `Job` sealed type: `Send(uris)` / `Receive(target, name, size)`), strings (en/es/ta), `AndroidManifest.xml` (none needed: MediaStore on 29+, app dir on 26–28).
- Create: `service/DownloadTarget.kt` (MediaStore insert with `IS_PENDING`, open the fd "w", publish or delete), `service/TransferText.kt` (pure: title/text for sending, receiving, saved, failed).
- Test: `test/.../service/TransferTextTest.kt`.

**Interfaces (Produces):**
```kotlin
TransferService.receive(context, address, port, isOffer: Boolean, value: String, name: String, size: Long)
object DownloadTarget { fun create(context, name, mime): Target?; class Target(val uri: Uri, val fd: ParcelFileDescriptor) { fun publish(); fun discard() } }
```
- **Free space.** Checked with `StatFs(Environment.getExternalStorageDirectory())`. Too little is a failure without a fetch.
- **The final notification.** It reads "Saved name" and opens it (`ACTION_VIEW` with the MediaStore uri and a read grant). When sends and receives mix in one batch, it reads "Done: 2 sent, 1 saved".
- [ ] **Step 1:** JVM tests for `TransferText`: the titles, the mixed batch, and the size formatting.
- [ ] **Step 2: Implement.**
- [ ] **Step 3:** Kotlin gates, then commit `feat(android): files from the computer are saved to Downloads with progress (F7b)`.

### Task 8: Android — the Files sheet and incoming offers

**Files:**
- Create: `ui/RemoteFilesSheet.kt`, `ui/RemoteFiles.kt` (pure model: parse the reply, sort, `canGoUp`), `ui/FileOffersEffect.kt`.
- Modify: `ui/QuickSettingsSheet.kt` (`onOpenFiles`), `ui/RemoteScreen.kt` (`showFiles`; mount `FileOffersEffect` in the session), strings.
- Test: `test/.../ui/RemoteFilesTest.kt` (or `files/`).

**Interfaces (Produces):**
```kotlin
data class RemoteEntry(val name: String, val path: String, val isDirectory: Boolean, val size: Long, val modified: Long)
data class RemoteFolder(val path: String, val parent: String?, val entries: List<RemoteEntry>, val error: String?)
object RemoteFiles { fun parse(json: String): RemoteFolder; fun sorted(entries: List<RemoteEntry>): List<RemoteEntry> }
```
- **`listRemoteFiles` returns the whole body.** The bridge's `list_remote_files` changes to return the raw response body JSON (path, parent, files, error) rather than the DTO list. This is done in this task, bridge and Kotlin together.
- **The sheet:**
  - a title showing the folder's last name;
  - an Up row when `parent != null`;
  - folders, then files with their sizes;
  - loading and error states;
  - tapping a file opens a confirm dialog with the size, then `TransferService.receive`.
- **`FileOffersEffect`** polls `takeFileOffers` every 1 s while in the session and starts a receive for each offer.
- [ ] **Step 1:** JVM tests: parsing (with and without `parent`, an error body), sorting, and a missing field defaulting.
- [ ] **Step 2: Implement.**
- [ ] **Step 3:** Gates (bridge + Kotlin), then commit `feat(android): browse the computer's files in a session and download them (F7b)`.

### Task 9: desktop app — send to phone, and the sent list

**Files:**
- Modify: `desktop/src/app.rs` (the Files tab send box), `desktop/src/model.rs` (`Snapshot.sent: Vec<SentFile>` newest first; `pub fn send_targets(&Snapshot) -> Vec<&Device>`, connected devices only), `desktop/Cargo.toml` (`windows` features for the dialog, if needed).
- Create: `desktop/src/pick_file.rs` (Windows: `GetOpenFileNameW`; elsewhere: `None`).

**Interfaces (Produces):**
```rust
pub fn pick_file() -> Option<PathBuf>; // cfg(windows) real, else None (button hidden)
```
- **The send box:**
  - a phone combo (connected only; "No phone in a session" otherwise);
  - a path text field with a Send button;
  - "Choose file…" on Windows;
  - a drop hint, with dropped files sent to the selected phone.

  Each send calls `outbox::request`.
- **The Sent list.** It shows each file's name, its phone (shortened id), "ago", and its state.
- [ ] **Step 1: Tests**
  - `send_targets` lists only connected devices.
  - The snapshot reads `sent.json` newest first.
  - `outbox::request` is called with the chosen device, via a model function `queue_send(state_dir, device, path)`, which refuses a missing path or a folder with a message.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Gates, then commit** `feat(desktop): send a file to a phone from the Files tab (F7b)`.

### Task 10: docs, review, device check

- [ ] README rows (file transfer), `docs/` checklist §30 (F7b device checks), spec "As built", AGENTS *Current Status* bullet. Commit `docs: desktop-to-phone transfer and the remote browser as built (F7b)`.
- [ ] A fresh reviewer over `feat/file-transfer..feat/file-transfer-b`. Fix each Important finding in its own commit.
- [ ] Device run (A71 ↔ Windows, LAN), following the spec's Device row. Record the results in the AGENTS bullet. Push the branch, open the PR against `main` (stacked on #16), and leave the merge to the user.
