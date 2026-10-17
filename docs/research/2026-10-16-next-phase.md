# Pocketdesk: the next phase (the plan for the 2026-10-16 overnight run)

Written 2026-10-16 from `main` at 035ddf0 (branch `fix/one-stream-on-wifi-return`), plus the R6 notes in commit 128fb9b (branch `test/reliability-pass`). Nothing in the repository was edited for this note.

## Owner's decisions (2026-10-16, evening)

- **Files over the internet road are on by default** (2c–2e): a phone that can open a session there could send files anyway, and the Files permission still applies. A switch in both Settings turns it off.
- **Phase 3 is a bridge, never a dependency.** Pocketdesk stays its own computer server and apps; it also talks to devices that run KDE Connect (the KDE Connect app, GSConnect, Valent). The bridge is **on by default**, with "Work with KDE Connect devices" in both Settings to turn it off (3a–3c below say "behind an off switch"; read that as on by default with the switch). It comes after phase 2.
- **KDE Connect may be installed on the A71** for 3a–3c's device checks, and removed after.
- **R6h is done** (A71 → Windows and A71 → the Azure VM, 2026-10-16; `docs/superpowers/specs/2026-10-14-phase-r-reliability.md` §R6). It added one item, **R6i**, below, and confirmed what 2c–2d are for: a file sent from the computer to a phone on the internet road is offered and never arrives, with the reason given only after 10 minutes.
- **R6a–R6c are done** (branch `feat/config-dir-override`): the second server, the idle harness, and the folder watcher, which brought the idle server to 0.53 % of one core. **A crash at server stop is found and fixed** on the same branch (part of R6g): execution inside an unloaded GraphicsCapture.dll, now pinned (30 stop-during-session rounds clean). R6g's overlapping-session stress test is written and passed: 200 rounds, 400 sessions, no error and no crash, in a release build (2026-10-17).

### R6i: a download waits for the network, not a fixed 65 s

- **Goal:** a download from the computer that breaks retries when the phone has a network again, not on a fixed 5, 15 and 45 s.
- **Why:** on the R6 run the A71's Wi-Fi took about 60 s to rejoin, so all three retries met "Network is unreachable" and the download stopped at 109 MB of 150 (a resend then continued from there, MD5 matching).
- **Touches:** `android/app/.../service/TransferService.kt`, `service/TransferRetry.kt` (JVM-tested): with no network, wait for one (`ConnectivityManager.registerDefaultNetworkCallback`) up to a few minutes, then go on with the backoff.
- **Tests:** JVM tests of the rule; on the A71, a 150 MB offer with the Wi-Fi off for 60–90 s finishes by itself.
- **Size:** S. **Risk:** a phone that never gets its network back must still give up, and say so.

Labels, as in `docs/research/2026-10-16-ecosystem.md`:
- **[R]** read in this repo (a path is given);
- **[S]** a cited source says it (links in §6);
- **[I]** my inference.

---

## 1. Where the project stands

### Done

- **Ecosystem phase 1 is complete and merged** (1a–1g, PRs up to #92) [R: `AGENTS.md` Current Status]:
  - events and hooks;
  - the Activity tab;
  - named tasks;
  - the browser extension;
  - Home Assistant over MQTT;
  - Android broadcasts;
  - the protocol spec, with a generated packet catalogue (`docs/packets.md`) and conformance vectors (`docs/protocol-vectors/control-v1.json`, replayed by `service::vector_tests`).
- **R1–R5 are done.** R6 (128fb9b; `docs/superpowers/specs/2026-10-14-phase-r-reliability.md` §R6, checklist §84) measured:
  - an hour with one session: 5.3 % of one core, private memory 82 → 87 MB;
  - 11 Wi-Fi drops, each recovered in 7–15 s;
  - recovery from a server restart in 10 s.
- **Fixed since R6, so not re-planned here:** the crash report (PR #93), the control link following the stream (PR #94), one stream on a Wi-Fi return (035ddf0), Auto picking VAAPI only where it runs (c89d0cb), and `video_quality` parsing (c5e98b2).

### Main gaps (each checked in the code)

1. **Idle cost doubled: 0.68 % → 1.35 % of one core.** About 19 file-system wake-ups a second come from separate pollers [R]:

   | Poller | Where | Period | What it does each tick, idle |
   |---|---|---|---|
   | File offers outbox (plus the expiry sweep) | `server/src/offers.rs:71,487` | 250 ms | `read_dir` of `outbox/` |
   | Links to phones | `server/src/plugins/links.rs:129` | 250 ms | `read_dir` of `links_out/` |
   | Phone media presses | `server/src/plugins/phone_media.rs:128` | 250 ms | `read_dir` of the presses folder |
   | Requests for a phone's files | `server/src/plugins/phone_files.rs:90` | 300 ms | `read_dir` of `phone_files_out/` |
   | Answers to phone notifications | `server/src/plugins/notification.rs:351` | 500 ms | `phone_notes::take_all` |
   | Live sessions | `server/src/live_sessions.rs:213` | 1 s | `kicks/` and the old `kick` file, `control/`, a stat of the trust store, and **`live_sessions.json` written and renamed every second**: its body carries `updated_unix` (`core/src/local/sessions.rs:103`), so it "changes" on every tick, because readers judge freshness by it (`FRESH_SECS = 5`, `sessions.rs:25,90`) |
   | Text replies outbox | `server/src/plugins/sms.rs:442` | 2 s | `sms::pending` |
   | Home Assistant switch | `server/src/home_assistant.rs:22,515` | 2 s | a stat of `config.toml` (`events::current_config`) |

   Not idle costs, because each returns early with no phone or no pending request:
   - the clipboard push (`clipboard_push.rs:138`; on Linux it spawns `wl-paste` every 2 s while a phone is linked);
   - the Windows HUD (`hypr_events.rs:177`);
   - approvals (`approvals.rs:16`) and task answers (`plugins/tasks.rs:305`).

2. **A 1 Mbit/s link cannot hold a session.** `LOSS_BITRATE_FLOOR_BPS = 1_000_000` (`core/src/streaming/bitrate.rs:66`) is the whole link. The arbiter changes only the bitrate, never frame rate or resolution, during a session (`core/src/streaming/streamer.rs:1412–1480`; the doc comment at `bitrate.rs:60–65` says so). Audio adds 64 kbit/s of Opus (`core/src/streaming/audio.rs:37`). The chaos proxy has no bandwidth cap (`core/src/streaming/chaos.rs:12–18`), so the case cannot be reproduced in tests today.

3. **A rare crash** (exit 139, three times ever) right after a reconnected session's setup. `server/src/crash_report.rs` will catch the next one. The unsafe capture code on Windows sits in `capture_wgc.rs`, `capture_d3d.rs`, `capture_dxgi.rs` and `audio_capture_wasapi.rs` (about 50 `unsafe` sites).

4. **The internet road carries the session and the control channel, but no transfers.**
   - Every transfer dials the phone back at `sender.peer_addr()`, and an iroh connection has none:
     - `share.rs::dial_back` (`server/src/plugins/share.rs:1052`);
     - `clipboard.rs::transfer_target` (`server/src/plugins/clipboard.rs:165`);
     - `docs/protocol.md` §1.2a step 7.
   - `live_sessions.json` marks such phones `files: false` (`server/src/live_sessions.rs:445`).
   - The README says files, the remote browser and image clipboard "still need the same network or Tailscale".

5. **The phone opens a new iroh endpoint, with the same secret key, for every WAN dial** (`IrohDial::dial_alpn`, `core/src/streaming/iroh_connection.rs:321–338`). Away from home, the session and the internet control link are therefore two endpoints with one identity on the same relay. Checklist §64 already lists this as owed ("a relay may route to only one of them").

6. **No computer ↔ computer.**
   - `docs/protocol-spec.md` §7 puts it out of version 1, and `docs/superpowers/specs/2026-10-13-device-keys-design.md:67` says "There is none".
   - The server never acts as a control-channel client.
   - Two servers on one machine would share one config folder and one iroh key: `paths::config_dir()` has no override (`core/src/local/paths.rs:66`), and only the state folder has one (`POCKETDESK_STATE_DIR`).

7. **KDE Connect shares only packet names** [R]:
   - TCP 1716 requires TLS first, and a plaintext `{` is refused (`server/src/service.rs:1012–1025`).
   - TLS is 1.3 only (`core/src/streaming/transport.rs:240,314,347`).
   - The certificate's CN is rcgen's default, not the device id (`transport.rs:384–396`).
   - There is no ping plugin (`docs/packets.md` has no `kdeconnect.ping`; `server/src/packets.rs:195` lists it as a helper-only name).
   - Notification bodies use `app`, not KDE's `appName` (`server/src/plugins/notification.rs:170`).

8. **Owed device checks pile up** (checklist §§43–84). Linux trails Windows: the README "Device-checked" row lists, for Linux, no files, no notifications or texts, no sound on the phone and no desktop app yet.

9. **iroh is 1.2.0** (`Cargo.lock:3064`); 1.3.0 is out [S]. The code sets no DNS fallback mode, so iroh's default (fall back to Google's nameservers when the system configuration cannot be read) still applies [S].

---

## 2. Research findings, each with what it means for Pocketdesk

### 2.1 What users of the category ask for and complain about (2025–2026)

| Finding | Source | Implication |
|---|---|---|
| Intel Unison shut down on 2025-06-30 (Lenovo Aura machines kept it to the end of 2025), with no successor offered | [S1], [S2] | Vendor bridges disappear. A self-hosted, account-free bridge is the durable option; keep the release channel ours (README, F-Droid work) |
| Phone Link's own troubleshooting: notifications stop until notification access is toggled. Android 15's Sensitive Notifications hides some alerts from it entirely. Forum threads: notifications missing after phone migrations, and the app not launching after a Windows update (May 2026) | [S3], [S4], [S5] | The background link is where trust is won or lost. The idle-cost fix (R6b–R6c) and the background notification work matter more than new surface. Pocketdesk already documents the Android 15 limit (d5a9352) |
| KDE Connect on Windows: shares that "succeed" on Android but never arrive and give no notice; bulk send missing; transfers "Stalled" for 1.4 GB; 10–20 Mbit/s against Syncthing's 100+ | [S6], [S7], [S8], [S9] | Pocketdesk's resumable, hash-checked transfers are a real edge, but only at home. Taking them onto the internet road (2c–2d) is the biggest remaining gap, and "sent" must keep meaning "saved" there too |
| KDE Connect users who want it away from home are told to use Tailscale or ZeroTier and type the overlay IP by hand, since discovery does not cross tunnels. A KDE Discuss proposal asks for a libp2p backend (hole punching, relays) because "most normal users don't know about" Tailscale and it "often conflicts with other VPNs" | [S10], [S11] | Pocketdesk's iroh road already answers this. Making every feature ride it (phase 2) is what users ask for, not a nicety |
| Pushbullet-style needs (clipboard, links, files, notifications) are served by KDE Connect or by push services (ntfy, Gotify) that have no clipboard or files | [S12] | Nothing to add. Clipboard and links already work in the background (T10b, T8a); keep them reliable over the internet road |
| scrcpy: audio was the most-requested feature until 2.0. The open camera issue is that v4l2loopback has a fixed resolution | [S13], [S14] | Camera-as-webcam stays later (research item 9 in `2026-10-15-what-makes-it-worth-using.md`) |
| RustDesk shipped Wayland multi-monitor in 1.4.3 (October 2025) and an unattended-Wayland preview in 2026. Reviews mention uneven performance on constrained or unstable networks | [S15], [S16] | A slow link must degrade, not drop. This is R6e–R6f. Unattended access stays out (no lock/power tests on this box) |
| Sunshine's docs: a host link faster than the client's slowest hop overflows buffers into packet loss. Users lower the bitrate cap by hand | [S17] | Matches R6: the encoder must respond to the slow hop by itself. Loss response exists; frame rate does not move |
| WebRTC's sender adapts to bandwidth by lowering resolution or frame rate (`degradationPreference`). LiveKit defaults screen share to "maintain-resolution", i.e. drop frames first | [S18] | For desktop content, lower the frame rate first and keep text sharp (R6f's ladder), then resolution |
| Home Assistant: HASS.Agent lives on as a community fork (MQTT). A new "HA Companion for Windows" registers through `mobile_app`, with no broker | [S19], [S20] | 1e's MQTT bridge is fine. A `mobile_app` path could later drop the broker requirement; not tonight |

### 2.2 KDE Connect's protocol: what compatibility needs (phase 3)

KDE Connect is GPL. Pocketdesk's rule (`AGENTS.md`) allows its ideas only, never its code. The facts below are behaviour, read from KDE's sources and advisories, for a clean-room implementation.

| Fact | Source | Consequence for a compat backend |
|---|---|---|
| Version 8 (released after March 2025) adds a second identity exchange inside TLS. The first, plaintext packet decides paired or not; the second identifies the device. Compatibility with older peers keeps the old exchange | [S21] | Implement v8 only. Current desktop master refuses `protocolVersion < 8` in discovery, in the TCP handshake and in the identity phase [S22] |
| CVE-2025-66270: no implementation checked that the second identity's `deviceId` matched the first. Fixed in desktop 25.12, Android 1.34.4, iOS 0.5.4, GSConnect 68 and Valent alpha.49 | [S23], [S24] | Test it from day one: the second identity's id and version must equal the first's, and the certificate name. Pocketdesk already has the analogous test on its own channel (`2aed8be`) |
| Ports: UDP 1716 for discovery; TCP 1716–1764 for the link; 1739–1764 for payloads | [S22], [S25] | Pocketdesk's control port 1716 is in range, so a broadcast can name it. A `{` first byte routes to compat; this is the spot `service.rs:1022` already peeks |
| **The roles are reversed.** The side that receives a UDP identity opens TCP and writes a plaintext "connection packet" (`deviceId`, `deviceName`, `protocolVersion`, `targetDeviceId`, `targetProtocolVersion`). The TCP client then does **server** TLS and the TCP server does **client** TLS. Each side writes its full identity (`deviceType`, capability lists) after TLS. A line may be 8192 bytes, each step has 1000 ms, and there are at most 42 unpaired links | [S22], [S26] | Pocketdesk must be able to be the TLS *client* on an accepted socket, and the TLS *server* on a socket it dialed. rustls supports both (`tokio-rustls` `TlsConnector` over any stream) [I] |
| `deviceId` must match `^[a-zA-Z0-9_-]{32,38}$` and equal the peer certificate's subject name. KDE derives its own id from its certificate's CN. Names are filtered of `"',;:.!?()[]<>` and cut to 32 characters | [S26], [S22], [S27] | A separate compat identity: a UUID with underscores, and a certificate whose CN is that id (with O=KDE and OU=Kde connect, as KDE writes them [S28]). Never reuse the Pocketdesk certificate, whose CN is rcgen's default |
| **KDE Connect Android caps TLS at 1.2**: `SSLContext.getInstance("TLSv1.2")`, commented "1.3 seems to cause issues in some (older?) devices". It signs its certificate with SHA512withECDSA or SHA512withRSA | [S29] | The compat path must allow TLS 1.2. Pocketdesk is 1.3-only (`transport.rs`); widen only the compat configs. `tokio-rustls` already builds with `tls12` (`core/Cargo.toml:68`). Record it in SECURITY.md |
| Pairing: `kdeconnect.pair {pair: true, timestamp}`. A request without a timestamp is ignored, and more than 1800 s of clock skew is refused. The verification code is the first 8 hex characters (upper case) of SHA-256 over the larger public key's DER, then the smaller's, then the timestamp in decimal | [S30] | The desktop app shows that code on an Allow/Deny prompt (the approvals pattern). No PIN: it is KDE's model, so compat devices get narrower defaults |
| mDNS: `_kdeconnect._udp` with TXT `id`, `name`, `protocol`. KDE Connect Android 1.29 added it, and 1.34.0 turned it off by default | [S31], [S32] | UDP broadcast is what reaches the Android app. On Windows, inbound UDP 1716 has no firewall rule (`install` dropped it; `docs/protocol.md` §1.5). So Pocketdesk broadcasts, and the phone dials in on TCP 1716, which `install`'s rule already allows for `pocketdesk.exe` |
| The Android app broadcasts only on "trusted" networks and needs ACCESS_LOCAL_NETWORK. It keeps a UDP 1716 listener and a TCP server on 1716+ | [S33] | Device checks must use a network the KDE app trusts |
| Payloads: the sender listens on 1739–1764 and does server TLS; the receiver connects and does client TLS, then reads `payloadSize` raw bytes. There is no key and no resume | [S25], [S34] | Phone → computer (Pocketdesk dials the phone) has the same direction as Pocketdesk's own shares, so `share.rs`'s part/place logic can be reused without the key or the resume. Computer → phone needs a listener on 1739–1764 and an inbound firewall rule: not tonight |

### 2.3 iroh (phase 2)

| Fact | Source | Implication |
|---|---|---|
| iroh 1.0 on 2026-06-15; 1.0.2 (relay security fix), 1.1.0 (security fixes), then 1.2.0 and 1.3.0 on 2026-09-28 | [S35] | Pocketdesk's 1.2.0 already has the security fixes. 1.3 is a small bump (2a) |
| 1.2.0: `with_fallback_mode` / `disable_fallback`. Before it, iroh "always fell back to Google's nameservers" when it could not read the system DNS configuration. A relay's authorization denial is fatal (`RelayStatus::auth_denied_reason`) | [S36] | Set the fallback explicitly, so the "third parties" list in `docs/protocol.md` stays true. A refused own relay can say why (it already logs refusals, from the N2 review) |
| 1.3.0: a full relay send queue could stall a connection while it sent its Initial packet (fixed); datagram batching; Router ALPN order follows registration order | [S36] | The fix matters for relayed control and session links. Pocketdesk uses no `Router` (no hits in `core/src`, `server/src` or the bridge), so the ordering change does not apply [R] |
| Multipath (0.96, 2026-01) and QUIC NAT traversal (0.98, 2026-04) are in 1.x; noq is n0's QUIC | [S35], [S37] | No work needed. The path statistics Pocketdesk reads per selected path stay valid |
| "Blinded address lookup" (2026-10-13): records stored under a blinded, rotating key and encrypted, so DHT nodes no longer see the endpoint id. It is a proof of concept in a separate repo, not shipped | [S38] | Later: it would close the README's "DHT nodes see the home IP linked to the WAN id". Not tonight |
| Authenticated relays (2026-07-30) are a managed-service feature, with tokens tied to an API key | [S39] | Pocketdesk's own relay already allow-lists paired keys (`server/src/relay.rs`). Nothing to adopt |

### 2.4 Android TV and Wear OS (phase 5)

| Fact | Source | Implication |
|---|---|---|
| A TV build needs a `LEANBACK_LAUNCHER` activity, `android.software.leanback` and `android.hardware.touchscreen` marked not required, and a banner | [S40] | Cheap in the manifest. The work is D-pad focus through the whole UI, and a gamepad or D-pad pointer for the session |
| Moonlight's TV path emulates the mouse with the controller (12.1 added scroll in that mode) | [S41] | Precedent for a "D-pad mouse" in the session. The `PadSender` code exists (`android/.../stream/PadSender.kt`) |
| No official Wear OS client for KDE Connect: "Android Wear has never been supported" (bug 388519). Watch PC remotes are niche (WowMouse, a gesture presenter) | [S42], [S43] | A Wear companion (media, wake, find, presenter) would be a differentiator. The Data Layer needs the same package and signing key on phone and watch (`docs/research/2026-10-16-ecosystem.md`), so it is release-key work the owner must be part of: not overnight |

---

## 3. The plan, in order

**Ground rules for the run** [R: `AGENTS.md`, the user's memory]:
- One logical change per commit, and each commit gated on its own:
  - Windows gates: `cargo fmt --all -- --check`, workspace clippy `-D warnings` (excluding the bridge and the spike), `cargo test`;
  - both core profiles when core changes;
  - bridge clippy and the cargo-ndk build when the bridge changes;
  - `assembleDebug`, `lintDebug` and `testDebugUnitTest` when Kotlin changes.
- Linux gates are owed to CI on each PR.
- Docs travel with their code. Stage files by name, and skim `git diff --cached` before every commit.
- Merge your own green PRs (the autopilot memory). Send one notification, at the end.
- Use a git worktree per item. Reviewers share the checkout, so do not switch branches under them.
- **Never stop, restart or replace the installed server on the Windows box**, and run no power or lock actions. R6a exists so that every run on that box can use a second, throwaway instance.
- Device tests on the A71 follow the safety recipe:
  - input off through `trusted_devices.json` on the test server;
  - phone prefs through `run-as`;
  - `MSYS_NO_PATHCONV`;
  - pass `-Pandroid.injected.androidTest.leaveApksInstalledAfterRun=true` to any connected test.
- Fixtures use documentation addresses only, and no home IP goes into the repo.
- Every feature's knobs go in **both** Settings, and anything that exposes data is off by default.

**Cut line.** A realistic night is R6a–R6h plus 2a–2e. 2f–2h (computer ↔ computer) and 3a–3c (KDE starter) are ordered next. 3a touches different files from 2f–2h (only `service.rs`'s first-byte branch overlaps), so it can run in a parallel worktree once 2c has merged.

### Overview

| # | Item | Size | Device-checkable tonight |
|---|---|---|---|
| R6a | A throwaway second server on one box (`POCKETDESK_CONFIG_DIR`) | S | Windows box (side by side with the installed server) |
| R6b | Measure what costs idle CPU, per poller | S | Windows box, Azure VM |
| R6c | One inbox watcher instead of eight pollers; mirror written only on change | M | Windows box, VM, A71 (latency of links and presses) |
| R6d | Linux: one `wl-paste --watch` instead of a process every 2 s | S | VM, with the A71 linked over the internet road |
| R6e | Reproduce the 1 Mbit/s link in a test (a rate cap in the chaos proxy) and find which watchdog ends it | S–M | Windows box (loopback, real capture) |
| R6f | A slow link keeps its session: lower the frame rate below the floor, then climb back | M | Windows box (throttled loopback); A71 → VM with `tc` limiting the VM to 1 Mbit/s |
| R6g | Crash hunt: overlapping session start/stop under stress, with crash reports | M | Windows box |
| R6h | R6's owed checks: a 150 MB transfer broken on a healthy link; the R6 pass on Linux | S (wall time ~2 h) | A71 + Windows, A71 + VM |
| 2a | iroh 1.2 → 1.3, and no silent DNS fallback | S | A71 → Windows and VM (WAN sessions) |
| 2b | One iroh endpoint per computer on the phone, shared by the session and the control link | M | Hermetic relay test; A71 → VM with direct UDP blocked (relayed) |
| 2c | Transfers on the internet road: desktop, core and spec | M–L | Hermetic (iroh on loopback) |
| 2d | Transfers on the internet road: phone (bridge and Kotlin), both Settings | M–L | A71 → Windows (control port closed), A71 → VM (real WAN) |
| 2e | Image clipboard on the internet road | S–M | A71 ↔ Windows (control port closed) |
| 2f | Pair two computers | L | Windows ↔ VM |
| 2g | Clipboard between paired computers, from anywhere | M | Windows ↔ VM |
| 2h | Send files to a paired computer | M | Windows ↔ VM |
| 3a | KDE Connect compatibility starter: discovery and the v8 handshake, behind an off switch | M–L | A71 with KDE Connect Android (owner's OK to install) ↔ Windows |
| 3b | Compat pairing (verification code on the desktop) and ping | M | as 3a |
| 3c | Compat clipboard, battery, find my device, and receiving a share | M | as 3a |

### R6a: a throwaway second server on one box

- **Goal:** `POCKETDESK_CONFIG_DIR` (absolute only, like `POCKETDESK_STATE_DIR`) moves the config folder, and with it `config.toml` and `iroh_secret.key`. A second instance with its own ports, state and iroh identity then runs beside the installed one.
- **Why:**
  - Idle and A/B measurements on Windows, which may not stop the installed server.
  - Two-process tests for computer ↔ computer (2f–2h).
  - Today a second instance would share `config.toml` (the same ports) and the iroh key (`server/src/iroh_endpoint.rs:41` reads it from `config_dir()`).
- **Touches:**
  - `core/src/local/paths.rs` (`config_dir()` honours the variable; reuse the parser behind `state_dir_from_env`);
  - `docs/development-setup.md` §4 ("A second server for tests": a config with other ports, `lan_discovery = false`, `dht_publish` as needed; the per-pid stop event in `server/src/platform/windows.rs:226` means `pocketdesk stop` with the variables set stops only that instance);
  - the CLI's `--help` or the man page, if it lists variables.
- **Tests:**
  - unit tests mirroring the state-folder parser: relative refused, empty ignored;
  - an integration test that spawns `env!("CARGO_BIN_EXE_pocketdesk")` with both variables and free ports, waits for its control port to answer, then stops it through `pocketdesk stop` with the same variables. It runs in CI on Linux and on the Windows box.
- **Acceptance:** two servers run side by side on the Windows box, the installed one keeps its pid (checked before and after), and the throwaway one has its own `device_id` and iroh id.
- **Risks:**
  - Anything reading `dirs::config_dir()` directly. A grep found only `paths::config_dir()` callers (`browser_host.rs`, `events.rs`, `iroh_endpoint.rs`, `file_browse.rs`, `config.rs`, the desktop app's `app.rs:872`).
  - The throwaway instance's toasts appear on the owner's desktop, so its PIN toast must not be mistaken for the real one. Name it in the toast? Out of scope; document it.
- **Knobs:** none (a test seam, not a feature).

### R6b: measure what costs idle CPU, per poller

- **Goal:** turn "watchers, not yet attributed" into a table: CPU seconds per poller per minute, and the mirror writes.
- **Why:** R6 found idle at 1.35 % against 0.68 % and could not say why (checklist §84, owed "What doubled the idle cost"). The fix (R6c) needs a before/after on the same harness.
- **Touches:**
  - a new `#[ignore]`d test module in `server/src` (crate-private access to each `run_watcher`). Under `use_state_dir(tmp)` it runs one poller family at a time for 60 s and reads process CPU time (`GetProcessTimes` through the existing `windows` crate on Windows, `getrusage` through the `libc` dev-dependency on Linux);
  - the R6 section of `docs/superpowers/specs/2026-10-14-phase-r-reliability.md` and checklist §84 (results).
- **Tests and checks:**
  - the harness itself;
  - the whole-server number on the R6a throwaway instance (the checklist §45 PowerShell script, 570 s after a 30 s settle, WAN and DHT on, as R4 and R6 ran);
  - `typeperf "\Process(pocketdesk*)\IO Other Operations/sec"` alongside;
  - the same harness on the Azure VM.
- **Acceptance:** the per-poller sum accounts for at least 80 % of the measured increase over 0.68 %, or the table says plainly what is left unexplained.
- **Size:** S (about 40 min of wall time).
- **Risks:** Windows Defender scanning the per-second `live_sessions.json` rewrite costs CPU in Defender, not in Pocketdesk. Note it, and do not claim it as a saving.

### R6c: one inbox watcher instead of eight pollers

- **Goal:**
  - The server wakes for the desktop app's requests when a file appears, not on a timer:
    - one OS change watcher over the inbox folders: `outbox/`, `links_out/`, the phone-media presses, `phone_files_out/`, notification answers, `sms_outbox/`, `kicks/`, `control/`, and approval and task answers;
    - each consumer waits on its own `tokio::sync::Notify` or a 5 s safety sweep.
  - `live_sessions.json` is written only when sessions or links change.
- **Why:** gap 1. Users pick a background bridge partly by its battery and CPU cost (§2.1, Phone Link).
- **Touches:**
  - a new `server/src/inbox.rs`, using the `notify` crate (new dependency; check its licence before adding, and that it builds under `x86_64-pc-windows-gnu`). It falls back to the sweep alone when the watcher cannot start.
  - the pollers: `offers.rs`, `plugins/links.rs`, `plugins/phone_media.rs`, `plugins/phone_files.rs`, `plugins/notification.rs` (`run_answers`), `plugins/sms.rs`, `live_sessions.rs`, `approvals.rs`, `plugins/tasks.rs`.
  - the mirror's freshness: `core/src/local/sessions.rs`. Readers already require the server to be running (`desktop/src/model.rs:71`, `server/src/local_api.rs:115`, by pid and image). Replace the 5 s `updated_unix` rule with the mirror naming the server's pid and start time, so a mirror left behind by a crash is still refused. Update all three readers, `server/src/home_assistant.rs:410` included, in the same change.
  - Fallback if that proves too wide: a heartbeat every 4 s inside the 5 s window, which cuts 75 % of the writes and leaves the readers untouched.
- **Tests:**
  - per consumer: a request file written is taken within 500 ms with the watcher, and within 6 s with the watcher disabled (an injected no-op watcher);
  - a mirror with a dead pid is not fresh;
  - all existing watcher tests stay green.
- **Acceptance:**
  - idle back at or under 0.7 % of one core on the throwaway instance (same script) and lower on the VM;
  - on the A71, "Send link" from the desktop app reaches the phone's notification as fast as before (≤ 0.5 s added at most);
  - `pocketdesk status` and the desktop app still show sessions, and "not running" after a crash.
- **Knobs:** none (no behaviour a person chooses).
- **Risks:**
  - ReadDirectoryChangesW loses events on buffer overflow, hence the sweep.
  - Watched folders must exist before watching: create them at start.
  - A desktop app older than the server would misjudge the mirror. It ships in the same release, but say so in the PR.

### R6d: Linux clipboard watched by one `wl-paste --watch`

- **Goal:** while a phone is linked, Linux runs one long-lived `wl-paste --watch` that signals a change, instead of `wl-paste --type text` every 2 s (`server/src/clipboard_push.rs:24`).
- **Why:** the same idle cost, on Linux, in the case that matters (a phone linked in the background). A process spawned every 2 s is the worst poller left there.
- **Touches:** `server/src/clipboard_push.rs`, `server/src/plugins/clipboard.rs` (the read), the Linux arm only.
- **Tests:** unit tests for the change parser; the Windows arm unchanged.
- **Device (VM, Sway):** `wl-copy` text reaches the A71 linked over the internet road (point the phone's saved control port at a closed port, as in checklist §64); the child ends when the last phone unlinks; and the VM's idle cost while linked is measured before and after.
- **Acceptance:** no `wl-paste` spawn per tick (checked with `pgrep -c` over a minute), and pushes still arrive within 2 s.
- **Size:** S.
- **Risks:** `--watch` needs `wl-clipboard` 2.x. If it is missing, fall back to the poll and log once.

### R6e: reproduce the 1 Mbit/s link, and find which watchdog ends it

- **Goal:** a test that holds a real session on a 1 Mbit/s link and says why it reconnects every 30–60 s.
- **Why:** R6 saw it on the A71 (checklist §84) but could not reproduce it at will. Three timers can end a session:
  - the phone's 10 s no-byte stall (`android/.../stream/StallWatch.kt`);
  - the server's 30 s silent-client rule (`CLIENT_SILENT_LIMIT`, `core/src/streaming/streamer.rs:32`);
  - QUIC's 45 s idle timeout (`transport.rs` `configured_transport`).
- **Touches:**
  - `core/src/streaming/chaos.rs`: an optional `rate_bps` token bucket on the proxy, keeping its counters and existing tests;
  - a new `#[ignore]`d `server/tests/slow_link_session.rs`, built on `session_record.rs`'s pattern (real capture and encoder over loopback QUIC through the proxy). Its client applies the phone's stall rule to `video_bytes()`, and it logs the arbiter's `Encoder bitrate target changed` lines.
- **Tests:** unit tests for the bucket (a deterministic clock); the ignored test, run on the Windows box.
- **Acceptance:**
  - at 1 Mbit/s with 20 ms delay, the test reproduces a session end within 5 minutes, naming the timer that fired;
  - at 20 Mbit/s it runs 5 minutes clean;
  - the result is recorded in the R6 section.
- **Size:** S–M.
- **Risks:** the proxy's token bucket adds its own queueing, so model a router with a bounded queue (drop tail at, say, 64 packets), not unbounded delay.

### R6f: a slow link keeps its session

- **Goal:** when the loss controller sits at its floor and still sees loss, step the frame rate down (60 → 30 → 15) and scale the floor with it (1 Mbit/s × fps/60, never under 250 kbit/s). Climb back the other way, frame rate first, on clean ticks.
- **Why:**
  - Gap 2: today the floor is the whole link.
  - For desktop content, dropping frames keeps text readable, as WebRTC's screen-share default does [S18].
  - Fewer frames into a constant-rate encoder also cut its real bitrate.
  - A 150 MB download also failed on that link (checklist §84).
- **Touches:**
  - `core/src/streaming/bitrate.rs` (a `starved` state and an fps output, as pure functions like `next_loss_ceiling`);
  - `core/src/streaming/streamer.rs` (the arbiter sends `(bitrate, fps)`; the encode task paces frames to the target and rebuilds with `cfg.fps`; the transport's in-flight window uses the live fps);
  - possibly `core/src/streaming/encoder.rs` (the sidecar's `-r`);
  - `core/src/local/config.rs` (`slow_link_frame_rate = true`);
  - desktop `settings_view.rs` ("Slow links: lower the frame rate to keep the picture moving", on by default; it exposes nothing);
  - phone: the HUD caption shows "slow link · 15 fps" (`stream/StatsHud.kt`). The phone's existing full-quality override (`full_quality`, which already clears the relay cap) also disables the step-down, so the phone's knob is the one it has. Strings in en, es and ta.
- **Tests:**
  - unit tests for the ladder: down only after N congested ticks at the floor; up frame rate first; never above configured;
  - R6e's ignored test, now green: 5 minutes at 1 Mbit/s with no session end, at least 10 fps delivered, and back to the configured rate and fps within 60 s after the cap lifts.
- **Device:** A71 → Azure VM with `tc qdisc add dev eth0 root tbf rate 1mbit burst 32kbit latency 400ms` on the VM: 10 minutes with no reconnect; then remove the qdisc and watch it climb back. The phone's input stays off.
- **Size:** M.
- **Risks:**
  - Rebuilding the encoder on every fps step costs an IDR each time; require hysteresis (ticks at a level before moving).
  - In-process VAAPI and the sidecar handle `fps` differently, so test both rungs the box has (NVENC sidecar on Windows; software).
  - Resolution steps are left for later (honest scope).

### R6g: crash hunt with overlapping sessions

- **Goal:** an `#[ignore]`d Windows stress test that starts and stops two overlapping sessions against one `StreamingServer` with real WGC capture and WASAPI loopback, 200 rounds, with `crash_report::install` in the test binary. Then a read-through of capture and loopback teardown for use-after-drop across sessions.
- **Why:** gap 3. The three crashes came right after a reconnected session's setup, and R6 saw two streams open on one Wi-Fi return (now fixed on the phone, but the server must survive any client).
- **Touches:** a new `server/tests/overlapping_sessions.rs`; maybe `core/src/streaming/capture_wgc.rs`, `capture_d3d.rs`, `audio_capture_wasapi.rs` (a fix only if found, in its own PR).
- **Acceptance:** 200 rounds without the process dying; or a `crash.log` backtrace, and a fix PR that makes the same test pass.
- **Size:** M.
- **Risks:**
  - It may not reproduce; say so and keep the test.
  - Runs capture the owner's screen locally only. Nothing leaves the box.

### R6h: R6's owed device checks

- **Goal:** the two checks owed in checklist §84:
  - a 150 MB transfer broken by a Wi-Fi drop on a healthy link, after PR #94 (offer, then `adb shell svc wifi disable` for 30 s, then enable; continued, MD5 matching);
  - the R6 pass on the Azure VM. Over the internet road from the A71: idle with no phone, an hour-long session, phone Wi-Fi drops, and a VM server restart, which is allowed because it is not the owner's box.
- **Touches:** docs only (checklist §84, the R6 section, AGENTS status).
- **Risks:** the VM's NSG must expose only what the run needs, and be deleted afterwards (the azure-test-vm memory: B2ats_v2, delete the RG, budget guard).

### 2a: iroh 1.3, and no silent DNS fallback

- **Goal:** bump `iroh` and `iroh-relay` to 1.3 in core, server and the bridge, and set the DNS fallback mode explicitly. On the phone, `Never`, since the bridge installs Android's servers. On the desktop, `Never` or `IfSystemEmpty`, decided in the PR and documented.
- **Why:** 1.3's fix for a relay send queue that stalled a connection's Initial [S36] helps every relayed link. An explicit fallback keeps `docs/protocol.md`'s list of third parties honest.
- **Touches:** `core/Cargo.toml:91`, `server/Cargo.toml:40,42`, `android/bridge/Cargo.toml:35`, `Cargo.lock`, the endpoint builders in `core/src/streaming/iroh_connection.rs` and `server/src/iroh_endpoint.rs`, and `docs/protocol.md` (the DNS row).
- **Tests:**
  - `cargo test -p pocketdesk-core --features wan`;
  - `server/tests/own_relay.rs`;
  - both clippy profiles;
  - the bridge's cargo-ndk build.
- **Device:** WAN sessions from the A71 to Windows and to the VM; `Diagnostics` "From outside" still reports.
- **Acceptance:** all green, and a session and an internet control link come up.
- **Size:** S.
- **Risks:**
  - Deprecations (`with_nameserver`).
  - A user's own `pocketdesk relay` built on 1.2 must still serve 1.3 clients (wire-stable within 1.x; check against the own-relay test with mixed versions if cheap).

### 2b: one iroh endpoint per computer on the phone

- **Goal:** the phone keeps one iroh `Endpoint` per computer and relay set, carrying the session, the internet control link and, from 2d, transfers. It closes when the last connection on it ends.
- **Why:**
  - Gap 5: two endpoints with one secret key on one relay is the case checklist §64 owes.
  - It is also the base 2c–2d build on.
  - It saves a DHT node and a socket per dial.
- **Touches:**
  - `core/src/streaming/iroh_connection.rs` (an `IrohClient` that owns the endpoint; `dial_alpn` and `dial_control` take it; the DHT lookup stays attached while a dial is pending);
  - `android/bridge/src/api.rs` (`connect_streaming_wan`, `dial_internet_control`), `android/bridge/src/cast.rs`.
- **Tests:**
  - first a red test with the in-process relay pattern from `server/tests/own_relay.rs`: two endpoints with one key, relay only, a stream connection and a control connection, showing one dropped within 30 s;
  - then green with the shared endpoint;
  - existing iroh tests.
- **Device:** A71 → VM with direct UDP from the home IP dropped on the VM (`iptables -A INPUT -p udp -s <home-ip> -j DROP`, never committed). The session and the control link are both up for 10 minutes on a relayed path ("WAN · relayed" badge), and clipboard pushes arrive.
- **Acceptance:** the red test shows the drop (or proves the relay keeps both; then record that §64's line is unfounded and keep the refactor only if 2d needs it); the green test passes; the device run holds 10 minutes.
- **Size:** M.
- **Risks:** relay mode differs per computer (own relays against n0's; `dial_relay_mode`, `iroh_connection.rs:205`), so key the endpoint by relay set, not globally.

### 2c: transfers on the internet road (desktop, core, spec)

- **Goal:**
  - On the internet road, a transfer rides a **bidirectional stream the computer opens on the phone's control connection**, starting with the transfer key line, then the same resumable transfer (`core/src/protocol/transfer.rs`).
  - A phone asks for this by omitting `port` from the request. This covers `kdeconnect.share.request`'s `payloadTransferInfo`, `file_fetch`, `folder_fetch`, and later the image packets.
- **Why:**
  - Gap 4, and research item 1 ("everything over the internet path"), the top request in the KDE Connect and LocalSend threads (`docs/research/2026-10-15-what-makes-it-worth-using.md`).
  - §2.1: users wanting files away from home are sent to Tailscale.
  - It needs no new port, no dial-back across NAT, and no new packet types.
- **Touches:**
  - `core/src/protocol/kdeconnect.rs` (`DeviceSender` gains an optional `open_link_stream()`, `None` on TCP);
  - `server/src/service.rs` (`handle_internet_control` keeps the `iroh::endpoint::Connection` and gives the connection's sender an opener; it sets a lower priority for the transfer streams than for the control stream);
  - `server/src/plugins/share.rs` (a `Route` enum, `Dial { to, tls }` or `Link(opener)`, replacing `dial_back` at every call site: shares, fetches, offers, folder fetches);
  - `server/src/plugins/pair.rs` (`may_transfer` accepts the connection's proven iroh key on this road);
  - `server/src/offers.rs` (`check`: a phone linked over the internet is reachable when its link can open streams);
  - `server/src/live_sessions.rs:445` (`files: true` for such links);
  - `core/src/local/config.rs` (`internet_files`; see Knobs);
  - docs: `docs/protocol-spec.md` §2 and a new "Transfers on the link" subsection, `docs/protocol.md` §1.2a step 7 and §1.4, `README.md` ("everything but files crosses" becomes "everything crosses"), `SECURITY.md` (on this road iroh proves both keys and there is no TLS layer of ours), and `docs/local-api.md` (`send` to an internet-linked phone).
- **Tests:**
  - core: `transfer::send` and `receive` over a real iroh bi-stream between two loopback endpoints (direct addresses, no relays), both directions, with a broken stream continued from the partial;
  - server: the share handler with a fake internet sender whose opener is a `tokio::io::duplex`: share, fetch, offer, folder fetch; a request with a port on the internet road is still refused (no dial-back there); a request without a port on TCP is refused (`docs/packets.md` unchanged);
  - `pocketdesk send --json` to an internet-linked phone;
  - an `#[ignore]`d 500 MB loopback run.
- **Acceptance:** every transfer kind completes over an iroh control link in tests, resumes after a break, and checks its SHA-256. `sent.json` reaches an end state. The control channel stays responsive during a transfer (a ping round trip under 500 ms in the test).
- **Knobs:**
  - desktop: Settings "Files over the internet road" (`internet_files`);
  - phone (2d): Settings "Files over the internet".
  - Default **on**: this exposes nothing a session on that road does not already (relays see sizes and timing, never content; README "What the internet path shows").
  - **Owner decision:** if "off by default when it exposes anything" is meant to cover relay-visible volume, flip both to off. The PR says this.
- **Size:** M–L (desktop and core only).
- **Risks:**
  - n0's public relays may throttle large relayed transfers; say so in the README and keep `transfer_keep_hours` resume.
  - Mixed versions: an old desktop answers a portless request with "no port to connect to", and the phone must say "this computer is too old for files over the internet" (2d).

### 2d: transfers on the internet road (phone)

- **Goal:**
  - The bridge accepts bidirectional streams on the internet control connection and hands each to the transfer waiting for its key.
  - Shares, downloads, offers and folder fetches omit `port` when the link came by the internet road (`CONTROL_OVER_INTERNET`, `android/bridge/src/api.rs:553`).
  - Kotlin stops calling files "home only".
- **Touches:**
  - `android/bridge/src/api.rs` (the accept loop beside the control reader started in `connect_to_peer` at `api.rs:463`; a `TRANSFER_WAITERS` map from key to a oneshot; `send_file_fd`, `accept_transfer`);
  - `android/bridge/src/download.rs` (`fetch_file_fd`);
  - Kotlin: `service/TransferService.kt`, `service/BackgroundFiles.kt`, `ui/RemoteFilesSheet.kt`, `ui/SettingsScreen.kt`, `Prefs.kt`, strings in en, es and ta;
  - desktop `settings_view.rs` for the desktop knob (if not done in 2c).
- **Tests:**
  - bridge: a stream with a known key goes to its waiter; an unknown key or no key line within 5 s is dropped; a waiter that times out is removed;
  - Kotlin JVM: which request shape for which road.
- **Device:**
  - A71 → Windows with the saved control port pointed at a closed port (the checklist §64 method): a 150 MB offer and a 120 MB share, each broken by `svc wifi disable` for 30 s and continued, MD5s matching; "Files on this computer" browsing and a download; a folder fetch.
  - A71 → VM (real WAN): the ecosystem plan's check, 500 MB resumed over iroh with TCP 1716 unreachable.
- **Acceptance:** all of the above, plus the too-old message against a desktop build from before 2c (the installed server on the Windows box, if older, is exactly that case, with no restart needed).
- **Size:** M–L.
- **Risks:** a background transfer on Android when the app is not in front (`TransferService` is a dataSync foreground service; unchanged).

### 2e: image clipboard on the internet road

- **Goal:** `clipboard_image` and `clipboard_image_fetch` take the same `Route` as 2c when the link is internet.
- **Why:** README "Clipboard: text anywhere, images on the same network or Tailscale". It is the last transfer kind left behind.
- **Touches:** `server/src/plugins/clipboard.rs` (`transfer_target` → route; `receive_image`; the fetch path), `android/bridge/src/api.rs` (`send_clipboard_image` and the fetch), `ui/ClipboardImages.kt`, `docs/protocol.md` §1.4, README.
- **Tests:** handler tests with an in-memory route, both directions; the 32 MiB and 40 MP limits unchanged on this road.
- **Device:** A71 ↔ Windows over the internet link: a screenshot copied on Windows lands on the phone's clipboard. Phone → computer stays owed: the A71 has no app that copies an image (checklist §38).
- **Size:** S–M.

### 2f: pair two computers

- **Goal:**
  - In the desktop app, "Pair another computer": paste the other computer's pairing link (`pocketdesk://pair`, which carries its addresses, certificate fingerprint, id and iroh endpoint) and type the PIN that computer shows.
  - This computer's server then dials the other as a control client with its own certificate, pinned to the link's fingerprint, and pairs as a phone does.
  - It also registers its iroh key with a new packet `kdeconnect.pocketdesk.wan_key {endpointId}`, accepted only from a paired device on the home road with its TLS key. That is the same trust as the phone's `[0xFC,0x00]` stream marker (`docs/protocol.md` §2.3).
- **Why:** ecosystem phase 2's "any of my devices", from anywhere for two computers and a phone. Gap 6.
- **Touches:**
  - a new `core/src/local/computers.rs` (owner-only `computers.json`: pins, addresses, the other's iroh endpoint; the app's request files);
  - `core/src/local/pair_link.rs` (parse a pasted link);
  - a new `server/src/peer_computers.rs` (a client of `core::protocol::connection::ConnectionManager`, TLS first, then the iroh road through `IrohDial::dial_control` with this computer's key, mirroring the phone's `connect_to_peer`);
  - `server/src/plugins/pair.rs` (a device of `deviceType: "desktop"` gets narrow default permissions: Clipboard, Files, Links; never Input, Exec, Power, Audio, Mic, Notifications or Texts unless the owner turns them on; and the new `wan_key` packet);
  - desktop `pair_view.rs` and the Devices tab (computers marked as such);
  - `docs/packets.md` (regenerated), `docs/protocol-spec.md` §7 (computer ↔ computer enters scope), `docs/protocol.md`, SECURITY.md.
- **Tests:** a two-process integration test `server/tests/two_computers.rs` on R6a's seams (each process with its own state and config folders and free ports):
  - A pairs with B using the PIN read from B's PIN file;
  - B lists A as a desktop with the narrow defaults;
  - A's `computers.json` pins B;
  - B has A's iroh key;
  - a wrong PIN or a wrong fingerprint pairs nothing.
- **Device:** Windows (as A, the throwaway instance from R6a, so the installed server is untouched) ↔ the VM (as B), with B's NSG open to TCP 1716 from the home IP only for the run.
- **Acceptance:** the test above; on the device, a pairing that survives both restarts (the VM's, and the throwaway's); B's Activity tab shows `phone_paired` for A (rename the event? keep the kind and add `deviceType` in metadata).
- **Knobs:** desktop only (there is no phone side): the "Computers" section; per-device permissions in the Devices tab.
- **Size:** L.
- **Risks:**
  - The server as a client receives B's pushes (HUD, endpoint, wake info, control) and must ignore what is not for a computer.
  - Echo loops are handled in 2g.
  - B must never admit a stream session from A unless the owner allows it.
  - The internet road to the VM before pairing is TCP to a public IP: fine with the pinned fingerprint, but say so.

### 2g: clipboard between paired computers, from anywhere

- **Goal:** with "Share the clipboard with my other computers" on (off by default: content crosses machines), A sends its clipboard changes to B over the link, and B's existing `clipboard_push` sends B's changes to A, since A is a connected client with Clipboard. Each applies what arrives, with an echo guard.
- **Touches:**
  - `server/src/peer_computers.rs`;
  - `server/src/clipboard_push.rs` (reuse `Watch`; skip the text that came from the peer);
  - `server/src/plugins/clipboard.rs` (a clipboard trait, so tests run on an in-memory clipboard);
  - `core/src/local/config.rs` (`computer_clipboard = false`);
  - desktop `settings_view.rs`.
- **Tests:** echo-guard unit tests; the two-process test with the in-memory clipboard: a change on A appears on B and not back on A.
- **Device:** Windows ↔ VM (`wl-copy` / `wl-paste`), both ways in under 2 s: the ecosystem plan's acceptance. Then the same with B's TCP 1716 closed, so it falls back to the iroh road.
- **Size:** M.

### 2h: send files to a paired computer

- **Goal:** `pocketdesk send <computer> <path>` and the desktop app's Files tab (with computers among the targets). A file arrives in B's Downloads as a phone's share does, with resume.
  - Files between computers ride the iroh link only (direct on a LAN, relayed otherwise), using 2c's stream route. This avoids a transfer listener on the computer acting as client.
- **Touches:** `server/src/local_api.rs` (`send`), `core/src/local/outbox.rs` (a computer as a target), `server/src/peer_computers.rs`, desktop Files view, `docs/local-api.md` and its doc test.
- **Tests:** the two-process test, with a 64 MB file broken mid-way and continued, and SHA-256 checked.
- **Device:** a 500 MB file Windows → VM, broken by dropping the VM's UDP for 30 s, then continued, MD5 matching.
- **Size:** M.
- **Risks:** the Explorer "Send to" menu (T10e) addresses phones; leave it for later.

### 3a: KDE Connect compatibility starter: discovery and the v8 handshake

- **Goal:** behind `kdeconnect_compat = false` (desktop Settings "KDE Connect devices", off by default), a compat backend that:
  - keeps a **separate identity**: a UUID with underscores, and a certificate with CN = that id, O=KDE, OU=Kde connect, in `<state>/certs/kdeconnect/`;
  - **broadcasts its UDP identity on demand**: while the owner has "Find KDE Connect devices" open, plus on a network change, with `tcpPort` = the control port;
  - **receives broadcasts** when UDP 1716 can be bound exclusively. When it cannot (the real KDE Connect holds it), it says so and does not start;
  - **routes a `{` first byte on the control port to compat.** There it reads the connection packet (≤ 8192 bytes within 1 s), runs **client** TLS on the accepted socket (TLS 1.2 or 1.3, on this path only), exchanges full identities within 1 s, and requires the second identity's `deviceId` and `protocolVersion` to equal the first's and the id to equal the peer certificate's CN (CVE-2025-66270). It refuses `protocolVersion < 8`, ids not matching `^[a-zA-Z0-9_-]{32,38}$`, and more than 42 unpaired links;
  - **dials a broadcaster's `tcpPort`** (1716–1764 only) and runs **server** TLS on that socket. This half can wait for a later PR if time is short, since on Windows the phone dials in.
- **Why:** ecosystem phase 3. KDE Connect has the iOS client and the GNOME ones Pocketdesk lacks. The compat handshake is the part every later plugin needs.
- **Touches:**
  - a new `server/src/kdeconnect_compat/` (`identity.rs`, `discovery.rs`, `handshake.rs`), parsing the peer CN with `x509-parser` (already in the graph, `Cargo.lock:8636`);
  - `server/src/service.rs` (the first-byte branch at `service.rs:1022`; with compat off, `{` keeps today's refusal);
  - `core/src/streaming/transport.rs` (compat-only TLS configs that allow 1.2);
  - `core/src/local/config.rs`, desktop `settings_view.rs`;
  - docs: `docs/protocol.md` (new rows: UDP 1716 on demand, the compat certificate, TLS 1.2 on this path only), SECURITY.md (a new limit: compat devices, TLS 1.2, pairing by a verification code, narrower permissions), `docs/protocol-spec.md` §7, and new `docs/protocol-vectors/kdeconnect-v8.json` replayed by a test, as the control vectors are.
- **Tests:** a Rust test client written from the facts in §2.2 (not KDE's code):
  - a v8 phone that dials in after our broadcast, completing the handshake;
  - the CVE case (an unpaired id first, then a paired id) closed and nothing registered;
  - a CN mismatch closed;
  - protocol 7 closed;
  - an oversized or slow line closed;
  - with the switch off, `{` gets `POCKETDESK_ERR 1 tls-required` as now;
  - TLS 1.2 refused on the Pocketdesk path (a regression guard).
- **Device (if the owner allows installing KDE Connect from F-Droid on the A71):** "Refresh" in KDE Connect lists "<host> (Pocketdesk)" while Find is open. Windows' firewall drops inbound UDP 1716, so expect discovery only through our broadcast and the phone's TCP dial to 1716, which `install`'s rule allows for `pocketdesk.exe` on Private networks. The KDE app must treat the Wi-Fi as trusted [S33].
- **Not checkable here:** GSConnect, Valent, KDE Connect desktop and iOS. There is no GNOME machine on the phone's network and no iPhone; the VM is across the internet, and KDE drops loopback discovery outside its test mode [S22].
- **Size:** M–L.
- **Risks:**
  - Widening TLS to 1.2 is the security cost: compat configs must never serve the Pocketdesk road.
  - On Linux, `kdeconnectd` binds UDP 1716 with address sharing, so detect it by an exclusive bind.
  - A custom `control_port` outside 1716–1764 makes compat refuse to start, with a reason.

### 3b: compat pairing and ping

- **Goal:**
  - `kdeconnect.pair {pair: true, timestamp}` both ways: a request without a timestamp or with more than 1800 s of skew is refused.
  - The desktop app shows the 8-character verification code with Allow and Deny (the approvals pattern, 30 s).
  - Compat devices live in their own owner-only `kdeconnect_devices.json` with the pinned certificate hash. Unpair works both ways.
  - `kdeconnect.ping` both ways: a desktop notification, and "Ping" in the desktop app.
  - Default permissions: Clipboard, Ping, Find my device, Battery. Files stay off until the owner turns them on.
- **Touches:** `server/src/kdeconnect_compat/pair.rs`, a new ping plugin (also harmless for Pocketdesk phones: announce it), desktop app prompt and Devices tab (compat devices marked, with their narrower switches), `docs/packets.md` (regenerated), SECURITY.md.
- **Tests:**
  - the verification code against a vector computed independently when written (`openssl dgst` or Python over fixed test keys, kept as hex in the test);
  - timestamp skew;
  - the pairing state machine;
  - vectors extended.
- **Device:** A71 KDE Connect pairs with the Windows box, both screens show the same code, and pings arrive both ways.
- **Size:** M.

### 3c: compat clipboard, battery, find my device, and receiving a share

- **Goal:**
  - `kdeconnect.clipboard` and `.clipboard.connect` both ways, through the existing plugin and `clipboard_push`, held to the compat permissions;
  - `kdeconnect.battery` into `phone_status.json` (marked compat);
  - "Ring this phone" (`kdeconnect.findmyphone.request`, KDE Connect's type) from the desktop app;
  - a phone → computer share: `kdeconnect.share.request {filename, payloadSize, payloadTransferInfo{port}}` is answered by dialing the phone's IP at that port (1739–1764 only), running client TLS with the compat certificate against the paired phone's pinned one, reading exactly `payloadSize` bytes, and saving through `share.rs`'s part/place (no resume) with Mark of the Web;
  - every Pocketdesk-only packet refused for compat devices (a test iterating `docs/packets.md`).
- **Touches:** `server/src/kdeconnect_compat/`, `server/src/plugins/{clipboard,phone_battery,siren,share}.rs` (compat entry points), desktop Devices tab.
- **Tests:** a fake KDE phone for each; the refusal sweep; a share with a declared size over 8 GiB refused.
- **Device:** A71 KDE Connect: clipboard both ways, a 50 MB share saved with MD5 matching, the battery in the Devices tab, and the phone ringing.
- **Size:** M.

---

## 4. What tonight's hardware can and cannot prove

| Can check tonight | How |
|---|---|
| Idle cost (R6b, R6c) | Windows: the R6a throwaway instance, the checklist §45 script. Linux: the VM |
| Slow link (R6e, R6f) | Windows: the throttled loopback test. A71 → VM: `tc tbf 1mbit` on the VM |
| Crash stress (R6g) | Windows: loopback, real WGC capture |
| Internet-road transfers (2c–2e) | A71 → Windows with the saved control port pointed at a closed port (iroh goes direct over the LAN); A71 → VM (a real WAN path); relayed paths by dropping home UDP on the VM |
| One endpoint on a relay (2b) | Hermetic in-process relay test; A71 → VM relayed |
| Computer ↔ computer (2f–2h) | Windows (throwaway instance) ↔ VM |
| KDE compat (3a–3c) | KDE Connect Android on the A71 ↔ Windows, if the owner OKs installing it |

| Cannot check here | Why |
|---|---|
| GSConnect, Valent, KDE Connect iOS and desktop interop | No GNOME machine on the phone's LAN, and no iPhone |
| Inbound UDP 1716 discovery on Windows | Needs an elevated firewall rule (`install` + UAC, the owner's call) |
| Phone → computer image clipboard | The A71 has no app that copies an image (checklist §38) |
| Mobile data, a real outside network | The A71 has no SIM; the VM stands in for "outside" |
| Sleep, wake, lock, restarting the installed server | Forbidden on this box |
| Phone battery cost of any of this | The A71 runs on USB power in every run so far |

---

## 5. Not tonight, and why

- **The other computer's screen in the desktop app.** It needs a viewer: a stream client, the `cast_view.rs` decoder and input in egui. L–XL, and a design spec first.
- **Edge-crossing between computers.** Windows-only by the ecosystem plan (not wlroots), and the VM is headless.
- **Phase 3 beyond the starter:**
  - computer → phone shares in compat (a listener on 1739–1764 and an inbound firewall rule needing elevation);
  - notifications (KDE's `appName` against Pocketdesk's `app`, and actions);
  - SMS (no SIM);
  - mousepad and presenter (input during unattended device tests is against the safety recipe);
  - MPRIS, run commands (Exec-class risk), and SFTP browsing (KDE runs an SSH server on the phone);
  - iOS and GSConnect (untestable here).
- **Phase 4 (device mesh).** Signed membership, revocation gossip and a recovery secret need a written design and a security review before code (the plan's "RFC step").
- **Phase 5 (TV, Wear, tablets).** No TV or watch hardware. Wear's Data Layer needs the release signing key on both apps, which is the owner's. A TV emulator pass could come later.
- **Blinded DHT records.** n0's blinded lookup is a proof of concept, not a released crate [S38]. Revisit when shipped: it would remove "DHT nodes see the WAN id with the home IP".
- **Transfers yielding to video on slow links**, and the transfers' 32 KiB/s minimum rate on a 1 Mbit/s link. Measure after R6f; it may need an arbiter term.
- **Forward-dialed transfers on the home road** (the phone dials the computer). It would fix access-point isolation and hotspots, but changes the protocol. RFC first.
- **Home Assistant through `mobile_app`** (no broker, as HA Companion for Windows does [S20]). 1e works; later.
- **Out for other reasons:**
  - a Windows package identity (MSIX; the certificate);
  - unattended access;
  - camera as webcam;
  - a gamepad (none here);
  - calls and SMS (no SIM);
  - macOS and iOS (set aside).
- **A release tag.** Ask the owner first (the release memory).

---

## 6. Sources

- [S1] Tom's Hardware, Intel discontinues Unison: https://tomshardware.com/software/intel-discontinues-unison-app-for-connecting-pcs-and-smartphones
- [S2] 9to5Google, Intel Unison shutting down: https://9to5google.com/2025/04/03/intel-unison-app-shutting-down/
- [S3] Microsoft support, Phone Link notifications troubleshooting: https://support.microsoft.com/en-US/Windows/Apps/PhoneLink/troubleshooting-notifications-in-the-Phone-Link
- [S4] Samsung Community, Phone Link no notifications (Galaxy S25): https://us.community.samsung.com/t5/Galaxy-S25/Phone-Link-no-notifications-on-Windows/m-p/3139791
- [S5] Microsoft Tech Community, Phone Link won't launch (2026): https://techcommunity.microsoft.com/t5/windows-11/phone-link-app-won-t-launch-and-causes-other-complications/m-p/4521091
- [S6] KDE Discuss, file share from Android to Windows not working: https://discuss.kde.org/t/file-share-from-android-to-windows-seems-to-not-work/44034
- [S7] KDE mailing list, Windows build and bulk sending (July 2025): https://mail.kde.org/pipermail/kdeconnect/2025-July/007805.html
- [S8] KDE Discuss, file transfer stalled most of the time: https://discuss.kde.org/t/kde-connect-file-transfer-stalled-most-of-the-time/8930
- [S9] KDE Discuss, why KDE Connect is not as fast as Syncthing: https://discuss.kde.org/t/why-is-kde-connect-not-as-fast-as-syncthing/28905
- [S10] KDE Discuss, crossing network boundaries: https://discuss.kde.org/t/kde-connect-cross-local-network-boundaries-possible-safe/29447
- [S11] KDE Discuss, a libp2p backend proposal: https://discuss.kde.org/t/libp2p-backend-for-kde-connect-p2p-over-internet/46875
- [S12] AlternativeTo, Pushbullet alternatives: https://www.alternativeto.net/software/pushbullet/?p=5
- [S13] It's FOSS, scrcpy 2.0 (audio, the most requested feature): https://itsfoss.com/news/scrcpy-2-0-release.md
- [S14] scrcpy issue 4507 (virtual camera): https://github.com/Genymobile/scrcpy/issues/4507
- [S15] AlternativeTo news, RustDesk unattended Wayland (2026): https://alternativeto.net/news/2026/8/rustdesk-brings-true-unattended-remote-access-to-wayland-including-multi-monitor-setups/
- [S16] RustDesk for Linux guide (multi-monitor Wayland in 1.4.3): https://rustdesk.com/de/blog/rustdesk-fur-linux-der-open-source-remote-desktop
- [S17] Sunshine troubleshooting docs (v2025.628.4510): https://docs.lizardbyte.dev/projects/sunshine/v2025.628.4510/md_docs_2troubleshooting.html
- [S18] W3C webrtc logs on `degradationPreference`, and LiveKit's defaults by source: https://lists.w3.org/Archives/Public/public-webrtc-logs/2025Sep/0003.html, https://docs.livekit.io/reference/client-sdk-flutter/livekit_client/getDefaultDegradationPreference.html
- [S19] HASS.Agent (community fork): https://github.com/hass-agent/HASS.Agent
- [S20] Home Assistant Community, HA Companion for Windows (WinUI 3, `mobile_app`): https://community.home-assistant.io/t/ha-companion-for-windows-native-winui-3-app/1020066
- [S21] KDE security advisory 2025-04-18 (protocol version 8): https://kde.org/info/security/advisory-20250418-2.txt
- [S22] kdeconnect-kde `lanlinkprovider.cpp` (master): https://invent.kde.org/network/kdeconnect-kde/-/raw/master/core/backends/lan/lanlinkprovider.cpp, and `lanlinkprovider.h` (UDP 1716, TCP 1716–1764): https://github.com/KDE/kdeconnect-kde/blob/master/core/backends/lan/lanlinkprovider.h
- [S23] KDE security advisory 2025-11-28 (CVE-2025-66270): https://kde.org/info/security/advisory-20251128-1.txt
- [S24] OpenCVE, CVE-2025-66270: https://app.opencve.io/cve/CVE-2025-66270
- [S25] kdeconnect-kde `compositeuploadjob.h` (payload ports 1739–1764) and `.cpp` (server TLS on upload): https://github.com/KDE/kdeconnect-kde/blob/master/core/backends/lan/compositeuploadjob.h
- [S26] kdeconnect-kde `deviceinfo.h` (identity, connection packet, id regex, name filter): https://invent.kde.org/network/kdeconnect-kde/-/raw/master/core/deviceinfo.h
- [S27] kdeconnect-kde `kdeconnectconfig.cpp` (the device id is the certificate's CN): https://github.com/KDE/kdeconnect-kde/blob/master/core/kdeconnectconfig.cpp
- [S28] kdeconnect-kde `sslhelper.cpp` (CN, O, OU in the self-signed certificate): https://github.com/KDE/kdeconnect-kde/blob/master/core/sslhelper.cpp
- [S29] kdeconnect-android `SslHelper.kt` (TLS up to 1.2): https://github.com/KDE/kdeconnect-android/blob/master/src/main/java/org/kde/kdeconnect/helpers/security/SslHelper.kt
- [S30] kdeconnect-kde `pairinghandler.cpp` (timestamp, 1800 s, verification key): https://github.com/KDE/kdeconnect-kde/blob/master/core/backends/pairinghandler.cpp
- [S31] kdeconnect-kde `mdnshdiscovery.cpp` (`_kdeconnect._udp`, TXT id/name/protocol): https://github.com/KDE/kdeconnect-kde/blob/master/core/backends/lan/mdnshdiscovery.cpp
- [S32] APKMirror, KDE Connect 1.34.0 release notes (mDNS off by default): https://www.apkmirror.com/?p=11395251
- [S33] kdeconnect-android `LanLinkProvider.kt` (trusted networks, ACCESS_LOCAL_NETWORK, UDP and TCP servers): https://github.com/KDE/kdeconnect-android/blob/master/src/main/java/org/kde/kdeconnect/backends/lan/LanLinkProvider.kt
- [S34] Valent, the KDE Connect packet layer (identity fields, `payloadTransferInfo`): https://valent.andyholmes.ca/documentation/protocol.html
- [S35] iroh blog index (1.0 to 1.3, multipath, NAT traversal, relays): https://www.iroh.computer/blog
- [S36] iroh 1.2.0 and 1.3.0 release post: https://www.iroh.computer/blog/iroh-1-3-0
- [S37] iroh on QUIC multipath: https://iroh.computer/blog/iroh-on-QUIC-multipath
- [S38] iroh, blinded address lookup (2026-10-13): https://www.iroh.computer/blog/blinded-address-lookup
- [S39] iroh, protect your relays (authenticated relays): https://www.iroh.computer/blog/authenticated-relays
- [S40] Android developers, create and run a TV app: https://developer.android.com/training/tv/get-started/create
- [S41] F-Droid, Moonlight (controller mouse emulation): https://f-droid.org/packages/com.limelight/
- [S42] KDE bug 388519, Android Wear never supported: https://bugs.kde.org/show_bug.cgi?id=388519
- [S43] Android Authority, Wear OS watch as a mouse (WowMouse): https://androidauthority.com/how-to-turn-wear-os-smartwatch-into-mouse-3402578
