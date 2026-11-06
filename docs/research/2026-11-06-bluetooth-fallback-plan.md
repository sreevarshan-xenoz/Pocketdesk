# Bluetooth fallback, phase 1: implementation plan

**Goal:** When neither the home road nor the internet road reaches the computer, the phone's control link (and with it clipboard, notifications, the remote, deck, ring, quiet, questions, links and small files) comes back over a Bluetooth Classic RFCOMM link to a bonded Windows computer in range.

**Architecture:** A third control road. The Windows server listens on an RFCOMM service; the phone's Kotlin side dials it, pumps the bytes through a Unix socket pair, and hands the other end to the Rust bridge. Over that stream run the same pinned TLS and control handshake as at home, served as `Road::Bluetooth` with the stricter rules the design fixes. File transfers open a second RFCOMM connection each, asked for over the control link.

**Tech Stack:** Rust (tokio, rustls, `windows` 0.62 with Bluetooth and WinSock features, MinGW, no Windows SDK), Kotlin (`android.bluetooth`, `ParcelFileDescriptor.createSocketPair`), JNI.

**Spec:** `docs/research/2026-11-06-bluetooth-fallback-design.md` (read its "Calls made after review").

## Global Constraints

- Bluetooth Classic RFCOMM only, secure (bonded, encrypted) sockets; no BLE, no discovery or scanning.
- On by default: computer `bluetooth_fallback = true` in `config.toml`; phone `Prefs.bluetoothFallback` default `true`.
- The phone dials Bluetooth only after home and internet fail, or at once when it has no usable network; never while another road works.
- Service UUID `b1f2c4e0-6a3d-4c8e-9f51-2d7e0a9c4b13`, service name `Pocketdesk`; the server reads `POCKETDESK_BT_SERVICE_UUID` (tests only) to listen on another.
- Pairing never happens over Bluetooth; a wrong key is refused outright (as the internet road); the computer names itself after admission; wake info and computer-to-computer rules as the internet road; a live Bluetooth link counts as the phone being near for the away lock.
- File transfers over 8 MiB (8 388 608 bytes) wait for Wi-Fi on the Bluetooth road.
- While the phone plays to a Bluetooth headset or is in a call, only control packets go (no transfer starts).
- Bluetooth back-off: 30 s after a failed dial, doubling to at most 8 min; cleared on a network change.
- Repo rules: one logical change per commit with its docs; `docs/status-history.md` entry; SECURITY.md limit; gates `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --exclude pocketdesk-android-bridge --exclude iroh-spike -- -D warnings`, `cargo clippy -p pocketdesk-core --no-default-features --features client -- -D warnings`, `cargo test --workspace --exclude pocketdesk-android-bridge --exclude iroh-spike` on Windows; Linux gates by CI; Android `./gradlew assembleDebug lintDebug testDebugUnitTest`.
- Never run test servers on the owner's PC except from `%LOCALAPPDATA%\pocketdesk-test\pocketdesk.exe`; never turn the PC's Bluetooth radio off or unpair anything.

## Review Focus

1. **Bluetooth on but the computer out of range or off:** the phone backs off and says the computer is unreachable, no spinner forever. (Task 7 test `backs_off_and_doubles`.)
2. **A bonded device that is not a Pocketdesk computer** (earbuds, a car): never dialed. (Task 7 test `only_dials_a_known_computer_address`.)
3. **Wi-Fi returns while on Bluetooth:** the next keep-link tick re-dials home and the Bluetooth link closes. (Task 7 test `prefers_wifi_when_it_returns`.)
4. **Bluetooth permission revoked or radio turned off on the phone mid-link:** the link ends cleanly and Settings says why. (Task 8 test `off_without_permission`.)
5. **The owner's own server and a throwaway on one PC:** the throwaway's UUID override keeps them apart. (Task 4 test `service_uuid_override`.)

---

### Task 1: TLS control handshake over any stream (core)

**Files:**
- Modify: `core/src/protocol/connection.rs:87-148`
- Test: same file's tests

**Interfaces:**
- Produces: `impl ConnectionManager { pub async fn secure_named<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(&self, stream: S, address: &str, identity: &DeviceIdentity) -> Result<(ControlDuplex, PeerClaims)> }`: rustls with the pin for `address`, then `control_handshake_named`. `connect_named` becomes TCP dial + `set_nodelay` + `keep_alive` + `secure_named` (its return type unchanged for current callers; box where needed).

- [ ] **Step 1:** Test `secure_named_over_a_duplex_matches_tcp`: a `tokio::io::duplex` pair, the server end running the test TLS acceptor and handshake the module's existing tests use; `secure_named(client_end, "192.0.2.10", &identity)` returns `Ok` and the same `PeerClaims` a TCP dial gets. A second case with a pin for another address returns `Err` whose text contains `TLS`.
- [ ] **Step 2:** Run `cargo test -p pocketdesk-core secure_named`; expect failure (no such method).
- [ ] **Step 3:** Implement `secure_named` and route `connect_named` through it.
- [ ] **Step 4:** Run the core tests and both core clippy profiles; expect pass.
- [ ] **Step 5:** Commit `refactor(core): the control link's TLS and handshake over any stream`.

### Task 2: The setting, the constants and the packet (core)

**Files:**
- Modify: `core/src/local/config.rs` (field, default, `KNOWN_KEYS`), `config.toml.example`
- Create: `core/src/protocol/bluetooth.rs` (registered in `core/src/protocol/mod.rs`)
- Test: in both files

**Interfaces:**
- Produces: `Config::bluetooth_fallback: bool` (serde default true); `pub const SERVICE_UUID: &str = "b1f2c4e0-6a3d-4c8e-9f51-2d7e0a9c4b13"`, `pub const SERVICE_NAME: &str = "Pocketdesk"`, `pub fn service_uuid() -> String` (the env override `POCKETDESK_BT_SERVICE_UUID` when it parses as a UUID, else `SERVICE_UUID`); `pub const PACKET: &str = "kdeconnect.pocketdesk.bluetooth"` with body `{ "address": "AA:BB:CC:DD:EE:FF", "service": "<uuid>" }` (`pub struct BluetoothInfo { pub address: String, pub service: String }`, serde, `pub fn parse_address(&str) -> Option<[u8; 6]>`); `pub const LINK_HELLO: &str = "POCKETDESK-BT-LINK"`: the first line of a transfer connection, `LINK_HELLO <link id>\n`; `pub const TRANSFER_MAX: u64 = 8_388_608`.

- [ ] **Step 1:** Tests: `bluetooth_fallback_defaults_on` (`Config::default().bluetooth_fallback` and a file without the key both true; it is in `KNOWN_KEYS`); `service_uuid_override` (set the variable to another valid UUID: returned; to `not-a-uuid`: `SERVICE_UUID`); `address_round_trip` (`parse_address("00:00:5E:00:53:01") == Some([0,0,0x5e,0,0x53,1])`, lower case accepted, `"00:00:5E:00:53"` and `"zz:..."` `None`); `link_hello_line` (`format_link_hello(7) == "POCKETDESK-BT-LINK 7\n"`, `parse_link_hello` round-trips, rejects anything else).
- [ ] **Step 2:** Run; expect failure.
- [ ] **Step 3:** Implement; add the key with a comment to `config.toml.example`; `docs/protocol.md` gets the packet, the service and the link hello.
- [ ] **Step 4:** Run the tests and gates; expect pass.
- [ ] **Step 5:** Commit `feat(core): the Bluetooth road's setting, service and packet`.

### Task 3: `Road::Bluetooth` in the server

**Files:**
- Modify: `server/src/service.rs` (`enum Road` at 1521, every `match road`: 1341, 1360, 1675, 1723-1760, 1877-1895), `server/src/away_lock.rs:308,370`, the `DeviceSender` trait and the control connection's sender
- Test: `server/src/service.rs` tests, `server/src/away_lock.rs` tests

**Interfaces:**
- Consumes: Task 1's stream type for nothing yet (this task is the rules).
- Produces: `Road::Bluetooth`; `pub(crate) async fn handle_bluetooth_control<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(stream: S, tls: tokio_rustls::TlsAcceptor, link: Arc<dyn OpenLinkStream>, identity_packet: Option<NetworkPacket>, registry: &Arc<PluginRegistry>, hypr_ipc: Option<&HyprlandIpc>, wan_endpoint: Option<&iroh::Endpoint>, pairing_required: bool) -> Result<()>`: span `transport = "bluetooth"`, refuses non-TLS (no KDE Connect compat here), TLS accept, `serve_control(.., Road::Bluetooth, Some(link), None, peer_key, ..)`. `fn nearby_by_radio(&self) -> bool` on `DeviceSender`, default `false`, `true` for a Bluetooth control link; `away_lock` counts a client present when `nearby(c.link_addr()) || c.nearby_by_radio()`.
- Rules: admission like `Road::Internet` but with the TLS key: `pair::standing(&device_id, peer_key).may_act(true)` must hold, else the refusal line and end (never held for pairing); identity after admission; pairing packets ignored as on the internet road; wake info and the computer-to-computer branches take the `Internet` arm. After admission the server sends `PACKET` (Task 4 supplies the address; here an `Option<BluetoothInfo>` from state, sent when `Some`).

- [ ] **Step 1:** Tests over a duplex stream and the module's test registry: `bluetooth_refuses_a_wrong_key` (a phone paired with key A dials with key B: `HANDSHAKE_REFUSED`, nothing registered); `bluetooth_ignores_pairing` (a `kdeconnect.pair` packet on the road: ignored, standing unchanged); `bluetooth_names_after_admission` (no identity line before the phone's own); `bluetooth_link_counts_as_near` (away-lock presence with `link_addr() == None` and `nearby_by_radio() == true` is present; the internet road's `None, false` still away).
- [ ] **Step 2:** Run `cargo test -p pocketdesk-server bluetooth_`; expect failure.
- [ ] **Step 3:** Implement the variant, the handler and the rules.
- [ ] **Step 4:** Run server tests and gates; expect pass.
- [ ] **Step 5:** Commit `feat(server): a control link over Bluetooth, with the internet road's rules and the away lock's presence`; SECURITY.md gets a new limit stating the four rules and that secure RFCOMM needs an OS bond.

### Task 4: The Windows RFCOMM listener and transfer links (server)

**Files:**
- Modify: `Cargo.toml` (`windows` features `Win32_Devices_Bluetooth`, `Win32_Networking_WinSock` already there, `Win32_System_Rpc` if `GUID` needs it), `server/Cargo.toml`
- Create: `server/src/bluetooth/mod.rs`, `server/src/bluetooth/windows.rs`, `server/src/bluetooth/links.rs`
- Modify: `server/src/service.rs` (start the listener beside the TCP listener when `bluetooth_fallback` and a radio are present; stop on config change)
- Test: `server/src/bluetooth/links.rs` tests; Windows-only unit tests in `windows.rs` that need no radio

**Interfaces:**
- Consumes: Task 2 constants, Task 3 `handle_bluetooth_control`.
- Produces: `pub(crate) fn radio_address() -> Option<[u8; 6]>` (BluetoothFindFirstRadio + BluetoothGetRadioInfo; `None` with no radio); `pub(crate) fn listen(service: uuid::Uuid) -> std::io::Result<Listener>` (AF_BTH/BTHPROTO_RFCOMM socket, `bind` to any port, `listen`, `WSASetService` with the UUID and `SERVICE_NAME`; the record removed on drop); `Listener::accept(&self) -> std::io::Result<tokio::io::DuplexStream>`: a blocking accept on its own thread, each accepted socket pumped both ways by two blocking threads into a 64 KiB `tokio::io::duplex` (no IOCP on Bluetooth sockets). `links.rs`: `pub(crate) struct BluetoothLinks` implementing `OpenLinkStream`: `open()` allocates a link id, sends `kdeconnect.pocketdesk.bluetooth.link` `{ "id": n }` on the control writer, and waits up to 10 s for the phone's transfer connection; `pub(crate) fn offer(&self, id: u64, stream: DuplexStream) -> bool` hands it over. The accept loop reads the first line of each new connection: `LINK_HELLO n` goes to `offer`, a TLS record goes to `handle_bluetooth_control`. Transfers whose size is over `TRANSFER_MAX` are refused on this road with a reason the phone shows ("waits for Wi-Fi").

- [ ] **Step 1:** Tests in `links.rs` over duplex streams: `open_waits_for_the_phones_link` (open, offer the matching id: returns that stream), `open_times_out` (no offer within 10 s with paused time: `Err`), `offer_unknown_id_is_refused` (`false`, the stream dropped); in `windows.rs`: `service_record_names_the_uuid` (the `WSAQUERYSETW` built for a UUID carries it and `SERVICE_NAME`, as a pure builder function).
- [ ] **Step 2:** Run; expect failure.
- [ ] **Step 3:** Implement; send `PACKET` with `radio_address()` and `service_uuid()` after admission on every road (so the phone learns it at home).
- [ ] **Step 4:** Run tests and Windows gates; then on this PC, from the fixed test path with a throwaway state and `POCKETDESK_BT_SERVICE_UUID` set, check `pocketdesk status` says the Bluetooth road is listening. Expect pass.
- [ ] **Step 5:** Commit `feat(server): listen for the phone over Bluetooth on Windows`; `docs/guide.md` gets the feature (what it does, on by default, the OS bond, the switch, what waits for Wi-Fi).

### Task 5: The desktop's switch

**Files:**
- Modify: `desktop/src/settings_view.rs` (a row beside "Files over the internet"), `docs/guide.md` (Settings list)
- Test: the settings view's existing search/row tests

**Interfaces:**
- Consumes: `Config::bluetooth_fallback`.
- Produces: a row "Keep working over Bluetooth when Wi-Fi drops" saved at once like its neighbour; status shows "Bluetooth: listening / no radio / off".

- [ ] **Step 1:** Test `settings_has_bluetooth_fallback_row` (Find a setting finds "Bluetooth"; toggling writes the key).
- [ ] **Step 2:** Run; expect failure.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run desktop tests (and `--features snapshots` clippy); expect pass.
- [ ] **Step 5:** Commit `feat(desktop): the Bluetooth fallback switch`.

### Task 6: The bridge takes a Bluetooth stream (Android bridge)

**Files:**
- Modify: `android/bridge/src/api.rs` (split the dialed-stream handling out of `connect_to_peer` around 637-700 into `async fn run_control(stream: ControlDuplex, address: String, port: u16, road: Road)`; replace `CONTROL_OVER_INTERNET: AtomicBool` with `CONTROL_ROAD: AtomicU8` (`0` home, `1` internet, `2` bluetooth) and update its readers at 1023 and 3467-3482), `android/bridge/src/lib.rs` (JNI)
- Test: bridge unit tests

**Interfaces:**
- Consumes: Task 1 `secure_named`, Task 2 `PACKET`, `BluetoothInfo`, `LINK_HELLO`.
- Produces: `pub async fn connect_over_bluetooth(address: String, port: u16, fd: std::os::fd::OwnedFd) -> Result<ConnectionState, String>` (wraps the fd as a tokio `UnixStream`, `secure_named` with `address`, then `run_control(.., Road::Bluetooth)`); JNI `nativeConnectOverBluetooth(address: String, port: Int, fd: Int): String` (envelope like `nativeConnectToPeer`, takes ownership of the fd); `nativeBluetoothInfo(address: String): String` (the last `PACKET` body heard from that computer, `""` if none); `nativeTakeBluetoothLinkRequest(): Long` (a pending link id from `kdeconnect.pocketdesk.bluetooth.link`, `-1` if none); `nativeOfferBluetoothLink(id: Long, fd: Int): Boolean` (writes `LINK_HELLO id` and serves the transfer on it); `nativeControlRoad(): Int`.

*Corrected 2026-11-06 (Task 4 review):* a transfer link is not opened to the control service: RFCOMM carries one connection per channel per device pair, so the phone dials the transfer service, `core::protocol::bluetooth::link_uuid(service)` (named `Pocketdesk transfers`), one link at a time, and after the hello answers the computer's transfer TLS as its home transfer listener does (protocol.md 1.4a).

- [ ] **Step 1:** Tests: `road_atomic_round_trip`; `bluetooth_info_kept_per_computer` (a `PACKET` heard from `192.0.2.10` is returned for it and not for `192.0.2.11`); `link_request_queue` (two requests, taken in order, then `-1`); `connect_over_bluetooth_runs_control` over a `UnixStream::pair()` with a test server end (the same test TLS helper as Task 1): state becomes `Connected` and `nativeControlRoad` is 2.
- [ ] **Step 2:** Run `cargo test -p pocketdesk-android-bridge` on CI (the bridge is Linux/Android only; `cargo clippy -p pocketdesk-android-bridge` on the host target).
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run; expect pass; build the arm64 `.so` with `android/bridge/build-android.sh`.
- [ ] **Step 5:** Commit `feat(android): the bridge runs the control link over a Bluetooth stream`.

### Task 7: The phone dials Bluetooth (Android)

**Files:**
- Create: `android/app/src/main/java/dev/pocketdesk/android/bluetooth/BluetoothRoad.kt`, `BluetoothRoadRule.kt` (pure), `android/app/src/test/java/dev/pocketdesk/android/bluetooth/BluetoothRoadRuleTest.kt`
- Modify: `bridge/RustCore.kt` (`connectToPeer` wrapper at 385, `watchNetwork` 335-366), `HostStore.kt` (per-host Bluetooth address and service)

**Interfaces:**
- Consumes: Task 6 JNI.
- Produces: `object BluetoothRoadRule { fun shouldDial(enabled: Boolean, permitted: Boolean, radioOn: Boolean, knownAddress: String?, bonded: Set<String>, hasUsableNetwork: Boolean, otherRoadsFailed: Boolean, now: Long, restUntil: Long): Boolean; fun nextRest(fails: Int): Long }` (30 000 ms doubling to 480 000 ms); `object BluetoothRoad { fun dial(ctx: Context, host: Host): Result<String>; fun stop() }`: `BluetoothDevice.createRfcommSocketToServiceRecord(uuid)`, `connect()` off the main thread with a 12 s limit, `ParcelFileDescriptor.createSocketPair()`, two pump threads, the other end's `detachFd()` to `nativeConnectOverBluetooth`; while the road is live it polls `nativeTakeBluetoothLinkRequest` every 500 ms and answers each with a new RFCOMM connection whose fd goes to `nativeOfferBluetoothLink`. `RustCore.connectToPeer` (all callers) tries Bluetooth after a failed native connect, or first when `!hasUsableNetwork`. `HostStore` keeps `nativeBluetoothInfo` after each connect.

*Corrected 2026-11-06 (Task 4 review):* each link request is answered with an RFCOMM connection to the transfer service, `link_uuid(service)`, not to the control service (one connection per channel per device pair); the computer asks for one link at a time.

- [ ] **Step 1:** `BluetoothRoadRuleTest`: `only_dials_a_known_computer_address` (address not in `bonded`: false); `waits_for_other_roads` (`otherRoadsFailed=false`, network usable: false; network unusable: true); `backs_off_and_doubles` (`nextRest(1)=30_000`, `2=60_000`, `5=480_000`, `9=480_000`; `now < restUntil`: false); `off_without_permission` (`permitted=false` or `enabled=false` or `radioOn=false`: false); `prefers_wifi_when_it_returns` (a pure `fun keepBluetooth(road: Int, homeOrInternetUp: Boolean): Boolean` is false once either is up).
- [ ] **Step 2:** Run `./gradlew testDebugUnitTest --tests '*BluetoothRoadRuleTest*'`; expect failure.
- [ ] **Step 3:** Implement; `watchNetwork`'s `onAvailable` also clears the Bluetooth rest and, when a native connect then succeeds, calls `BluetoothRoad.stop()`.
- [ ] **Step 4:** Run unit tests, `lintDebug`, `assembleDebug`; expect pass.
- [ ] **Step 5:** Commit `feat(android): the phone keeps its link over Bluetooth when Wi-Fi is gone`.

### Task 8: The phone's switch, notice, audio and file rules (Android)

**Files:**
- Modify: `Prefs.kt` (`KEY_BLUETOOTH_FALLBACK` default true, `KEY_BLUETOOTH_TOLD`), `ui/SettingsScreen.kt` (switch beside "Files over the internet", asks `BLUETOOTH_CONNECT` when off-granted), strings in `values`, `values-es`, `values-ta`
- Create: `bluetooth/BluetoothNotice.kt`, `bluetooth/BluetoothAudio.kt` (pure rule + a reader of `AudioManager`), tests for both
- Modify: the transfer start path (where the phone sends or accepts a file) for the 8 MiB rule

**Interfaces:**
- Consumes: Task 7 `BluetoothRoad`, Task 6 `nativeControlRoad`.
- Produces: `fun BluetoothAudio.busy(a2dpPlaying: Boolean, inCall: Boolean): Boolean`; `fun transferMayStart(road: Int, bytes: Long, audioBusy: Boolean): TransferVerdict` (`Go`, `WaitForWifi`, `WaitForAudio`); `BluetoothNotice.maybeTell(ctx)`: the first time the road becomes 2, a dialog if the app is in front, else a notification, text "Wi-Fi dropped, so Pocketdesk is using Bluetooth to reach <computer>." with the button "Turn this off" opening the setting; sets `KEY_BLUETOOTH_TOLD`. Wherever the link's road shows today (the card's LAN/WAN, the session's link state), road 2 shows as "Bluetooth".

- [ ] **Step 1:** Tests: `audio_busy_when_playing_or_in_call`; `big_files_wait_for_wifi` (`road=2, bytes=8_388_609`: `WaitForWifi`; `8_388_608`: `Go`; `road=0`, any size: `Go`); `transfers_wait_for_audio` (`road=2, audioBusy=true`: `WaitForAudio`); `told_once` (with a fake prefs store, `maybeTell` tells once, never again); `off_without_permission` (the switch shows the reason text and `BluetoothRoadRule.shouldDial` gets `permitted=false`).
- [ ] **Step 2:** Run; expect failure.
- [ ] **Step 3:** Implement; strings in all three locales.
- [ ] **Step 4:** Run unit tests, lint, assemble; expect pass.
- [ ] **Step 5:** Commit `feat(android): the Bluetooth fallback's switch, one-time notice, and what waits`.

### Task 9: Docs, owed checks and the device run

**Files:**
- Modify: `docs/device-verification-checklist.md` (a new section), `docs/owed-checks.md` (regenerated), `docs/status-history.md`, `README.md` (one line), `scripts/device_checks.py` (checks below)

**Interfaces:**
- Consumes: everything above.

- [ ] **Step 1:** Checks in `scripts/device_checks.py`, run with no `adb reverse` and the throwaway's `POCKETDESK_BT_SERVICE_UUID`: `bt1-takeover` (Wi-Fi off on the phone: the server's log shows `transport="bluetooth"` within 20 s, the target 10 s, the time said); `bt2-clipboard` (a copy each way over the road); `bt3-notification` (the fixture's notification reaches the computer); `bt4-off-switch` (the throwaway's `bluetooth_fallback=false` and restart: no Bluetooth link, the phone says unreachable); `bt5-back-to-wifi` (Wi-Fi on: the road goes back to home within 30 s); `bt6-big-file-waits` (a 9 MB share over the road: refused with "waits for Wi-Fi").
- [ ] **Step 2:** Run them on the OPPO against a throwaway from the fixed path; record the times in the checklist; anything that cannot run is listed as owed with why. Earbuds on the laptop while `bt2`/`bt3` run is a by-ear check for the owner: listed as owed, and if it stutters a server-side throttle is added before phase 1 ships.
- [ ] **Step 3:** Regenerate `docs/owed-checks.md`, write the status-history entry and README line.
- [ ] **Step 4:** Run `python scripts/owed_checks.py --check` and the runner's unit tests; expect pass.
- [ ] **Step 5:** Commit `docs: the Bluetooth fallback's checks and journal`.
