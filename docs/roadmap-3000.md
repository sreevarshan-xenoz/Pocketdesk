# Linux Link 3000 — verified feature & optimization backlog

Status: rewritten 2026-09-24. The previous file (`roadmap-2000.md`, commit `779608f`) was generated
from a skim of the repository and was wrong often enough that it could not be used to plan work:
~25 % of its items were already shipped, and roughly a third of what it presented as future work
rested on a false premise or duplicated another id. Every one of its 2050 items has now been checked
against source by seven per-range audits plus first-hand verification of the claims this document
makes about tests, CI and telemetry. IDs 001-2050 keep their original numbers so any reference to
them stays meaningful; 2051-3000 are new work items derived from the gaps the audit surfaced and
from the product depth the project still needs.

Execution order, gates and exit criteria: [docs/roadmap-execution-plan.md](roadmap-execution-plan.md).

## How to read this

- **Default is ABSENT.** A range states its exceptions; any id not listed under it has nothing in
  the tree yet. That keeps the document honest without spending a line per no-op.
- **DONE means implemented *and* wired.** A `RustCore.kt` JNI export with no Kotlin caller, or a
  pure function with a unit test and no runtime path, is PARTIAL at best. This distinction is what
  the old document lacked, and it is why so many of its items looked redundant.
- **WRONG is a result, not a insult.** False premises and duplicates are recorded so the next
  generated roadmap does not rediscover them, and so nobody schedules work against a bug that
  cannot exist (e.g. "warn on certificate expiry" when expiry is never checked).
- Evidence is given as `file:line` for the load-bearing claims only. Line numbers drift; the
  symbol names matter more.

## Verification summary

| Range | Area | DONE | PARTIAL | WRONG/dup | Roughly absent |
| --- | --- | --- | --- | --- | --- |
| A 001-100 | Product / foundation | 2 | 5 | 0 | ~93 |
| B 101-200 | QUIC core / transport | 20 | 21 | 9 | ~50 |
| C 201-300 | Security / trust / privacy | 9 | 20 | 16 | ~55 |
| D 301-400 | Capture (Wayland/X11) | 24 | 22 | 2 | ~52 |
| E 401-500 | Video encode / codecs | 26 | 20 | 11 | ~43 |
| F 501-600 | Client decode / render | 27 | 20 | 9 | ~44 |
| G 601-700 | Audio / voice / media | 12 | 8 | 20 | ~60 |
| H 701-800 | Input / HID | 11 | 22 | 6 | ~61 |
| I 801-900 | Files / clipboard / notifications | 6 | 12 | 10 | ~72 |
| J 901-1000 | Window / desktop control | 20 | 18 | 3 | ~59 |
| K 1001-1100 | Virtual display / second screen | 0 | 3 | 5 | ~89 |
| L 1101-1200 | Automation / macros / productivity | 1 | 1 | 13 | ~85 |
| M 1201-1300 | Multi-device / ecosystem | 7 | 12 | 3 | ~66 |
| N 1301-1400 | Discovery / networking / WAN | 24 | 16 | 7 | ~53 |
| O 1401-1500 | Android UX / app architecture | 10 | 26 | 8 | ~56 |
| P 1501-1600 | Linux CLI / desktop UX | 7 | 20 | 0 | ~73 |
| Q 1601-1700 | Observability / diagnostics | 5 | 0 | 1 | ~94 |
| R 1701-1800 | Performance / memory / power | 2 | 1 | 0 | ~97 |
| S 1801-1900 | Reliability / recovery / chaos | 2 | 3 | 5 | ~89 |
| T 1901-2000 | Testing / CI / packaging / release | 4 | 3 | 0 | ~92 |
| X 2001-2050 | Expansion backlog | 1 | 1 | 2 | ~46 |

Headline numbers, all verified first-hand on 2026-09-24:

- 292 Rust test functions in the workspace as run by `cargo test --workspace` (279 pass, 13 are
  `#[ignore]`d), across six integration-test files. Feature-gated tests (`wan`, `encode`) are outside
  that count and only compile under an explicit `--features`.
- **Zero** Kotlin tests — `android/app` has no `src/test` or `src/androidTest` at all.
- **Zero** occurrences of `percentile`, `p95`, `p99` or `histogram` anywhere in `core/src`,
  `server/src` or `android/app`. Every latency/quality number the project reports is an average or
  an EWMA. This is the single most consequential gap in the whole document: without percentiles
  there is no way to see a latency regression that affects 5 % of frames.
- No `benches/` directory; no benchmark harness of any kind.
- CI (`.github/workflows/ci.yml`) is a single `ubuntu-latest` job: fmt → clippy → release build →
  `cargo test --workspace`, with `cargo audit || echo "…"` in `release.yml` so an audit finding
  cannot fail anything. **`main` has been red on every one of the last six pushes** (verified from
  `gh run list`, 2026-06-22 back through the R4 work), and every one fails at the *first* step,
  `cargo fmt --all -- --check`. Measured on a clean checkout of `90d92df`: 96 diff hunks across 22
  files (12 in `core`, 10 in `server`). The in-flight protocol work in the working tree was not
  cleaner — `cargo fmt` there aborted before reporting anything, on an internal rustfmt error
  ("left behind trailing whitespace") at `server/src/v2_multiplexer.rs:102`.
  **Status moved since that measurement (2026-09-24):** 2102, 2103 and the matrix half of 2231 are
  landed — fmt, workspace `clippy -D warnings` and `cargo test --workspace` (285 passed / 0 failed /
  13 ignored) all pass **from a fresh clone**, and `ci.yml` is now four jobs (`rust`, a `profiles`
  matrix over the `client` / `client,wan` / `wan` / `encode` feature sets, `android`, `audit`) with the
  `|| echo` stripped so an audit finding can fail a run. `release.yml` was missing the apt dev packages
  its own build links against, so no tag has ever produced a CI-built artifact.
  What is still open: nothing in that matrix has *executed* — the pushed `main` stays red at
  `cargo fmt --check` until these commits are pushed, which is the user's call.
- The Android matrix (`assembleDebug`, `lintDebug`, any bridge cross-compile) *was* not in CI at all; the
  `android` job added 2026-09-24 builds the arm64 bridge `.so` with cargo-ndk first, because the `.so` is
  gitignored and `assembleDebug` alone would silently package whatever stale binary was copied in. Its
  first real run is still ahead of it.

## Structural corrections to the old roadmap

1. **It audited a stale baseline.** The roadmap's headline capture goal is
   `ext-image-copy-capture-v1` (301) while the tree already shipped a *different* native backend —
   `zwlr_screencopy` output + Hyprland `hyprland_toplevel_export` window capture (R4 B1/B2/B3) —
   which the roadmap never mentions. Same for encode: it plans "runtime encoder verification"
   (414/415) as future work though the throwaway-encode probe, per-`/dev/dri/renderD*` selection
   and startup verification ladder are shipped and live-verified.
2. **It treated library internals as roadmap tasks.** Path validation, connection-id rotation,
   path-MTU discovery, congestion control and pacing (125, 128, 129, 161-164, 168) belong to
   quinn/iroh; there is no application surface to "implement" them on. They are now re-scoped as
   *expose and measure what the transport already knows* (see 2151-2170).
3. **It extrapolated whole products from one primitive.** K (virtual display) is ~92 % fiction —
   there is not one virtual-output call in the tree, and the section was written from prose in
   `v2_blueprint.html`. G (audio) invented a VoIP stack (AEC, VAD, ducking, PLC, meters) from a
   `pw-loopback` capture plus a mic relay. Section I assumed a chunked, resumable transfer protocol
   exists to extend; reality is a single `share.request` and a 64 KB copy loop.
4. **It split shipped mechanisms into many ids and counted them as progress.** Items 414-419 are one
   encoder-verification mechanism; 510-515 are one `setFrameRate` call; 232-235 are four ids for a
   QR feature that does not exist; 875/876/878 duplicate 874/255/259; 1402/1405, 1406/1408/1409,
   1412/1413, 1442/1443 are each the same two lines of Kotlin.
5. **It missed the defects that actually matter, while implying coverage.** At audit time: the session
   HUD's "drops" field was a hardcoded `0` (`android/bridge/src/api.rs`, `frame_drops`), so the UI lied;
   the server's `KEYCODE_MAP` (`server/src/input_injector.rs:27`) covered ~26 keys, so modifiers, media
   keys and function keys past F12 degraded silently; desktop audio dead-ended at the bridge because the
   phone has no player; and the packet-loss input of the adaptive-bitrate controller was an ignored
   parameter (`update_loss(_lost)` in `core/src/streaming/bitrate.rs`), which the old roadmap listed as
   "absent elsewhere" without ever naming. Each of these is a numbered item in section U, and the ones
   already fixed are marked there.
6. **Several sections end with boilerplate "integration tests / soak / release gate" trios**
   (898-900, 998-1000, 1095-1100, …) that describe nothing repo-specific and duplicate section T.
   Testing work is consolidated in 2251-2350 with named targets, and the per-section trios are
   recorded as duplicates.

## Verified status of ids 001-2050

### A. Product / Foundation 001-100

- **DONE** 021 the UX error taxonomy (`ui/UiError.kt`, raw→friendly, live in the snackbar and error card,
  device-verified), 075 `CONTRIBUTING.md` current (edition 2024, JDK/NDK, both clippy profiles).
- **PARTIAL** 004 version constants exist but are scattered (`LL_EXT_VERSION`, `ALPN_V2`, the handshake
  `protocolVersion`) with no registry, 023 typed `config.toml` with serde defaults but no schema version,
  028 a bridge `SessionStatus` enum with no Kotlin-driven state machine behind it, 085 a protocol spec under
  `docs/superpowers/specs/` that describes Flutter-era design rather than the current wire, 098 release notes
  generated from a `CHANGELOG.md` that is stale Flutter content.
- Everything else is ABSENT, including all ten "budget" ids (011-020), the state-machine cluster (027-040),
  every `… test command` / `… lint command` / `config explain|reset|snapshot` id (043-060), the profile and
  wizard ids (061-065), provenance and signing (067-071), and every matrix document (088-095). 043 is worth
  naming: **there is no `doctor` command at all** — the closest thing in the tree is the installer's
  environment probing, and the shipped `linux-link capabilities` verb is not it.

### B. QUIC Core / Transport 101-200

- **DONE** 101 single v2 endpoint with dual-ALPN dispatch (`server/src/service.rs:125,383`, bridge
  `connect_v2`), 102 ALPN then version-range check (`protocol/v2.rs:80`), 106 control-stream framing
  (`v2.rs:42,47`, stream 0), 108 18-byte packet header (`transport.rs:595`), 109 plugin-type routing
  (`kdeconnect.rs:271`), 113 per-op timeouts, 115 typed error enum + QUIC close code, 119 close with
  reason, 120 unknown-kind and oversize rejection, 122 `configured_transport()`, 123 15 s keepalive,
  131 RTT EWMA (`bitrate.rs:180`) + e2e EWMA (`client.rs:50`), 141 newest-frame queue
  (`streamer.rs:659`), 143 keyframe request, 144 its rate limit (`client.rs:529`), 155
  drop-on-full bounded channels, 160 bytes/goodput accounting, 181 relay disclosure chip, 187
  same-device-id eviction, 192 transport metrics to the bridge.
- **PARTIAL** 103 capabilities are carried in the identity packet but never intersected or gated,
  104 stream-kind `if/else` with persistent streams discarding their payload, 112 reply ids only for
  notifications, 116 remote errors only in the wake-relay shape, 124 idle timeout fixed at 45 s with
  no config knob, 133 loss hook ignores its argument, 138 dropped packets counted but never exported,
  146-149 input/control/file/audio each get a stream and quinn's `set_priority` is called on it, but
  the priority does not bound anything under loss (measured 2026-09-24, `video_flood_test` routed
  through the chaos proxy for real: 50/50 urgent packets delivered at 10% datagram loss, avg latency
  2.2-3.8 s and max 6.3 s against the "sub-300 ms" contract, versus 12-89 ms at 0% loss — the flood
  there is raw quinn streams, so the production `streamer.rs` path is still unmeasured), 156 one unbounded
  `read_to_end` (`transport.rs:563`), 168 migration left entirely to quinn defaults, 169/173/174/175
  reconnect loops that restore nothing but the socket, 176 relay-vs-direct chosen by iroh with the
  app only latching a boolean, 186 a 3 s UI cooldown standing in for hysteresis, 200 a single
  version-mismatch unit test.
- **WRONG/dup** 105/139/140/198 assume a datagram plane — `use_datagrams` (`transport.rs:26`) is dead
  config and no datagram is ever sent; 142 dup of 141, 154 dup of 114, 175 dup of 173; 110/111/117/118
  (request/response ids, GOAWAY, session drain) are gRPC vocabulary with no matching wire concept in a
  stringly-typed KDE-Connect JSON control plane.
- Everything else in 101-200 — correlation ids, cancellation messages, keyframe reason codes,
  fairness scheduler and per-class budgets, starvation detection, zero-copy/pooled framing, IPv6 and
  Happy Eyeballs policy, suspend/resume recovery, relay promotion and health scoring, metered policy,
  quotas and rate limits, packet trace, deterministic and fuzzed transport tests — is ABSENT.

### C. Security / Trust / Privacy 201-300

- **DONE** 201 `TrustStore` persistence (`kdeconnect.rs:348`), 202 stable identity cert+key on disk
  (`state/certs/`), 221 `pair --grant` TTL, 224 the same grant aimed at support sessions, 236 `unpair`,
  237 unpair-all, 238 `kick`, 245 Android `RECORD_AUDIO` + the Android-14 microphone FGS type, 294 telemetry
  is local-only by construction.
- **PARTIAL** 203 keys are plaintext files (`identity.key` written with default mode; only the iroh
  secret is `0o600`), 206 the same one permission hardening in the whole tree, 209 implicit
  serde-default legacy load, 210 on corruption `known_peers.json` silently resets to empty — that is a
  TOFU-downgrade path, not recovery, 213 a certificate mismatch hard-fails with friendly copy and
  "Pair again" but there is no re-confirm flow, 220 the PIN is single-use while the trust it creates is
  permanent, 223 view-only is a session toggle rather than an identity property, 227 PIN shown by
  `notify-send` with no pair/unpair notification, 228 wrong PINs logged with no counter or alert, 248
  the wlroots privacy ring shows only while the input grab is active and nowhere else, 254 the grab
  self-releases on a 600 s TTL with no screen privacy, 265/266/269 one home-prefix jail with a lexical
  `..` test, non-canonical so a symlink escapes it, 288 wall-clock TTL rather than idle expiry, 294
  local-only by default with no scrubber behind it.
- **WRONG** 216/217 assume certificate expiry is evaluated — `TofuVerifier` never checks it; 240 dup of
  239; 249 dup of 248; 225 dup of 224; 232-235 are four ids for QR pairing, a feature that does not
  exist anywhere; 267-268, 270-274, 275-276, 291, 295-299 describe machinery (archive handling,
  signature validation, hash-chained logs, crash redaction) with no substrate in this project yet.
- Notably absent and *not* future-optional: 229/230 (a 6-digit PIN with unlimited attempts inside its
  5-minute TTL), 211/212 (fingerprints are computed and thrown away; there is no UI that shows them),
  204 (no Android Keystore use at all — identity key and TOFU pins are plaintext in app storage),
  279/280/281 (see the exec finding below).

> **The exec item must not be built as written.** 277-283 read like "add command safety later", but
> `ExecPlugin` is already registered and runs arbitrary `sh -c` for any packet of type
> `kdeconnect.linuxlink.exec` (`server/src/plugins/exec.rs:52-56`) with no approval, no timeout, no
> output cap and no allowlist. It *is* gated by pairing (`plugins/pair::is_trusted`, checked on the v1
> dispatch path at `service.rs:770`) and no shipped UI calls it, so this is not an unauthenticated
> remote shell — but a support session granted by `pair --grant`, which is explicitly the low-trust
> one-off path, silently gets a full user shell equivalent to keyboard injection. The honest P0 item is
> "gate or remove exec, and give it per-command approval" (2351-2356), not "add an allowlist on top of
> today's behaviour as an enhancement".

### D. Screen Capture / Wayland / X11 301-400

- **DONE** 302 `screencopy` output capture, 303 Hyprland toplevel-export window capture, 308 shm is the
  working buffer path, 310 encoder rebuilds at the real frame size, 325 portal/PipeWire as the second
  Auto attempt, 331 Hyprland adapter (IPC + export + events), 341 X11 root grab, 342 X11 region grab,
  349 idle FPS floor on both backends, 359 duplicate-frame suppression (`capture.rs:781` memcmp), 388
  workspace-change events, 389-393 window open/close/focus/geometry/title events, 400 the
  `capture_attempts()` policy table with its `capture_backend` config key and tests.
- **PARTIAL** 304 screencopy asks for the cursor overlay, the portal path sets nothing, 309 format
  handling differs per backend (screencopy accepts ARGB/XRGB only), 317 damage is a boolean idle gate
  with no rectangles propagated, 319 frames carry a local `Instant`-derived stamp, not a compositor
  present time, 326-328 portal handling is timeouts plus an availability probe, 329 per-global bind
  probes with no aggregate capability report, 351 static/active pacing is binary with no motion level,
  358 dropped frames logged only, 360 equality is a full byte compare, not a hash, 373 the mapped/hidden
  filter exists for the picker only, 377 handshake/stage timeouts but no in-stream stall detection,
  394 `class` and pid exposed, no `app_id`, 395 the privacy shield and `EVIOCGRAB` exist but no window
  is ever filtered out of the capture.
- **WRONG** 301 `ext-image-copy-capture-v1` is not implemented and Hyprland's support for it is still an
  open upstream issue (hyprwm/Hyprland#9916, opened 2025-04) — keep it as a capability-detected
  *backend option* (2551) rather than the strategic foundation the old roadmap made it; 320 dup of 319,
  374 dup of 373.
- ABSENT includes every per-compositor profile beyond Hyprland (332-340: Sway, KWin, Mutter, niri,
  River, Wayfire, Labwc, Weston, COSMIC), all dmabuf/modifier work (306-307 — `dmabuf` events are
  deliberately failed over), transforms and fractional scaling and HiDPI mapping (311-313), colour and
  HDR (314-316, 375), vsync/present-timing (318), GPU ingest and interop (321-324), XDamage/XComposite/
  XShm (344-346 — `capture_x11.rs` is dead code: re-exported, never called), the whole capture-mode and
  tiling/ROI research cluster (352-357, 361-368), cursor decoupling and shape relay (347-348, 369),
  black-frame and compositor-stall detection (376-378), capture watchdog/restart (379-380), and output
  hotplug/mode/scale/rename detection (381-387 — monitor removal is *explicitly filtered out* at
  `hypr_events.rs:28`), plus every capture test/benchmark/gate (396-399).

### E. Video Encoding / Codecs 401-500

- **DONE** 402-404 H.264 profiles mapped, 405 HEVC across VAAPI/NVENC/x265 with the ladder dropping it
  when disallowed, 411 hardware→VAAPI→software ladder, 412 NVENC-first resolution, 413 software
  fallback, 414-419 the runtime-verification cluster: encoder probe, throwaway NVENC encode, per-node
  VAAPI open, startup probe with black frames, dead-child reap, stall supervisor and rebuild, 420
  rebuild at frame dimensions, 421 bitrate-watch rebuild, 425 GOP at 2×fps, 426 forced IDR API +
  channel, 428-429 start-code and Annex-B tests with client-side NAL sniffing, 433 keyframe re-seed on
  client reconfigure plus forced IDR on server rebuild, 436 `zerolatency`, VBV bounds and
  `KEY_LOW_LATENCY`, 445 NVENC capped VBR, 446 two-frame VBV, 448 `bframes=0` on every rung, 449
  `rc-lookahead=0`, 461 the NVENC power-clock guard.
- **PARTIAL** 401 a `VideoCodec` enum limited to H264/H265, 423 codec can only change *downward* through
  the degrade path, 431/434 SPS/PPS and the keyframe cache live client-side; the server keeps no GOP
  cache so a late joiner waits for the next IDR, 435 the first-frame IDR is the encoder default rather
  than enforced, 441-442 the presets are bitrate ceilings and startup quality, 444 VAAPI `maxrate == bitrate`
  approximates CBR, 447 GOP is a constant, not a knob, 462 the guard locks clocks with no correlation
  metric, 478-482 crop is software, scale exists only in the sidecar rung, BGRA→NV12 is a plain
  `sws_scale` through pipes, 485-487 codec/encoder capability caching is partial and re-probed, 490-492
  runtime probes catch driver mismatch, fallbacks are logged not measured, 493 an `allow_hevc` bool is
  the whole policy, 496 one loopback caps roundtrip.
- **WRONG** 406-410 (all AV1) and 450-460 (slice/reference tuning, temporal/spatial layers, SVC,
  simulcast, multi-res, thumbnail/preview/background encode) are research ids with zero presence — the
  old roadmap's own Stage-2 note says AV1 is premature and this document agrees, see 2995; 437-440
  invent content presets that would be expansions of 441; 430 (AVCC) is the opposite of the shipped
  Annex-B wire format.
- ABSENT: 422 framerate reconfiguration, 424 profile change, 427 scene-change IDR, 432 VPS cache, 443
  CRF, 463-471 every encode-time/frame-size histogram and PSNR/SSIM/VMAF hook, 472-477 thermal/battery/
  GPU-memory-aware policy and any zero-copy path, 483-484 10-bit/HDR, 488-489 broken-driver blacklist and
  driver fingerprint, 497-500 soak, leak and crash-recovery tests and the codec release gate.

### F. Client Decode / Render 501-600

- **DONE** 501 capability cache, 502 instantiate-probe (`createDecoderByType`), 504 `KEY_LOW_LATENCY`,
  505 realtime priority, 506 MediaCodec→`SurfaceView`, 508 `setFrameRate` hint from the native refresh
  rate, 522 the 10 s dead-link watchdog, 524 per-keyframe MIME re-sniff with decoder swap, 525 format
  change reconfigure and re-seed, 527 fit-to-container letterbox, 532 viewport→video→desktop touch map,
  539 PiP, 540 session foreground service, 542-543 secure surface + `FLAG_SECURE`, 545-546 blackout
  (secure + dimmed), 547 wake-lock + keep-screen-on, 554 surface create/destroy drives the decoder,
  576 quality-preset cycler in session chrome, 577 the three preset ceilings, 582 codec-change notice,
  585 relayed-link banner, 588 retry.
- **PARTIAL** 507 `graphicsLayer` zoom forces a composed copy, so the surface path is not zero-copy,
  509 reads `display.mode` without acting, 523 a stall tears the link down and needs a manual retry,
  526 rotation survives via `configChanges` with no handling, 544 recents suppression is a `FLAG_SECURE`
  side effect, 555 no saved-state restore, 559-560 the HUD has no decode-time item and its drops field
  is fabricated, 570 the e2e sample is an age-plus-half-RTT estimate, not a per-frame age, 581 silent
  capability degradation with no warning, 583-584 a connecting timer and a LAN/WAN chip, no quality
  banner, 587 retry is immediate with no countdown, 589 a backoff reconnect exists in the bridge but the
  app's retry is a plain stop/start, so it is dead in the live path, 593 exit guards the blackout.
- **WRONG** 510-515 are one `setFrameRate` call split six ways; 520 the HUD's "drops" is `frame_drops: 0`
  hardcoded in `api.rs` — that is a defect to fix (2051), not a feature; 586 dup of 585 (roaming rebind is
  already the same path); 590-592 assume a resume-token subsystem that does not exist.
- ABSENT: 503 secure-decoder capability query, 516 VRR, 517-521 pacing/queue/late-frame metrics, 528-531
  letterbox/crop/stretch/pixel-perfect modes, 533-538 safe-area, notch, foldable, DeX, tablet and
  desktop-window display handling, 541 lockscreen policy, 548-549 thermal and battery callbacks, 550-553
  display hotplug and refresh diagnostics, 556-558 process-death recovery and decoder log sampling,
  561-569 expert/diagnostics screens and any percentile, 571-575 graphs, 578-580 manual FPS/codec/
  resolution overrides, 594-600 session history, details, share, soak, benchmark, release gate.

### G. Audio / Voice / Media 601-700

- **DONE** 603 default-sink query (`pactl get-default-sink`), 607 48 kHz stereo 64 kbit, 608 as configured,
  614 packet-loss target, 622-624 mute/volume/default-device control, 626 output-device picker in the
  audio sheet, 630-633 the complete phone-mic share path (capture → bridge → server → `pw-loopback`
  source with respawn and cleanup), 639 mic toggle, 642 the named virtual source.
- **PARTIAL** 601 desktop audio is captured, encoded, streamed and *then dropped*: `receiveAudio` has no
  caller, so there is no player and nothing to sync against (since 2054 the drop is deliberate and stated —
  the capability report's `played_on_the_receiving_end` column says `no` for this direction, and the desktop
  sends nothing at all when it cannot capture rather than streaming silence); 606 mono exists in config and
  the mic path
  while the desktop stream hardcodes two channels; 611 bitrate is set once with no profile switch; 613 FEC
  is permanently on rather than negotiated; 620 the timestamp header is written and never consumed; 643-645
  one fixed node name and a `MEDIA_ROLE` on capture; 689 the mic node respawns on a PipeWire write failure
  and nothing else in that cluster exists.
- **WRONG** 615-619, 621, 628-629, 634-638, 646-655, 661-686 assume a playout stack (jitter buffer, clock,
  A/V sync, concealment, meters, VAD, ducking, call control); none of its half exists to be tuned, and the
  roadmap's own premise that desktop audio is a shipped feature is false at the last hop; 622/670 and
  624/671 are the same controls twice, and J's 961-964 re-list 622-625.
- ABSENT: 602 monitor-source discovery, 604-605 per-app sources and a source selector, 609-610 44.1/32 kHz,
  612 complexity profiles, 625 device hotplug subscription, 627 route health, 640 a real recording
  indicator, 641 desktop mic selector, 656-660 Opus benchmarks and budgets, 687-688, 691-700.

### H. Input / HID / Remote Interaction 701-800

- **DONE** 701 normalized absolute touch on a dedicated `ABS_X/ABS_Y` uinput device, 702 relative motion,
  703 click, 707 two-finger scroll, 708 horizontal wheel, 713 trackpad mode, 722 direct-touch mode, 726
  crop-aware mapping, 776 server-enforced view-only that drops injected packets, 781 enigo↔uinput ordering
  by `WAYLAND_DISPLAY`, 790 the uinput device lifecycle primitives.
- **PARTIAL** 704-705 right/middle are on the wire and injectable but no phone gesture emits them, 710
  direct touch tracks the finger and releases but **never sends the button press** (`tapAbsolute` unused),
  715 pinch is local zoom, not a desktop gesture, 724-725,727 the mapping math is right with no calibration
  or scale-factor handling, 734 two keycode registries with a ~26-key server map; unmapped codes fall
  through to `Key::Unicode` control characters, 735 unicode text rides enigo while the uinput path is
  ASCII-only and no in-session sender exists, 737 external share pushes clipboard only, 738 modifier
  hold/release is encoded but Shift/Ctrl/Super are absent from the server map so combos degrade, 739 press
  and release with no repeat, 741 MENU mapped client-side only, 742 F1-F12, 743 media/volume keycodes mapped
  client-side and unmapped server-side, 745 the shortcut bar is wired onto that degraded path, 754-756
  gamepad exists as wire format plus toy key emulation with no Android source and press-without-release,
  772 a 16-deep broadcast queue that drops on lag with no ACK, 778 devices built at startup and never
  recreated, 779 error strings advise about the uinput group without enforcing it, 782 the "input profile"
  is one env check, 788 KDE-Connect capability strings, 796-797 touch cancel handled locally, never
  forwarded.
- **WRONG** 723 dup of 713; 768-771, 773-775, 793-795 are listed as incremental hardening while the input
  path has *no* sequencing, ACK, stuck-key release or latency machinery at all — re-scoped as 2651-2666;
  766-767 assume a layout abstraction the injector does not have (QWERTY hardcoded).
- ABSENT: 706, 709 high-resolution wheel, 711-712 long-press drag and desktop-side right-click, 714-719
  the rest of the gesture set, 720-721 multi-window gestures, 728-733 cursor shape relay, 736 an IME
  surface, 740 key repeat, 744 pointer capture, 746-752 shortcut profiles/macros, 757-765 gamepad
  completion (rumble, calibration, layouts, sensitivity), 783-787 portal/accessibility/XWayland
  diagnostics, 789, 791-792, 798-800 input tests and gates.
- **Corrections since this audit (2026-09-24)**, because the clauses above describe what the work below
  started from, not what is true now: 710's missing press is fixed and `tapAbsolute` is the tap path
  (`60ae055`, 2061/2062); 734's ~26-key server map, 735's control-character fallthrough, 738's absent
  modifiers, 741's MENU, 742's F-keys and 743's media/volume rows are closed by the completed table plus
  the shared code set (`dc9fc22`, `a9b903d`, 2057-2060/2271); 754-756's press-without-release is closed by
  the edge-tracked gamepad handler (2063) — though the *Android source* half of that clause still holds,
  so the path remains unreachable from the app (2695). 778's never-recreated devices are now rebuilt once
  on a failed write (2064). 766-767's layout abstraction is now *detected and named* at injector
  construction (2065), but not honoured — layout-aware injection stays 2665.
  736, 740, 744 and the ABSENT list are unchanged.

### I. Files / Clipboard / Notifications 801-900

- **DONE** 866 clipboard history persisted with a sheet, 870 20-entry and 8192-char bounds, 871 dedupe on
  add, 872 local/remote echo prevention, 874 the sync toggle, 886 notification reply end to end.
- **PARTIAL** 805 the file channel kind is demuxed and then ignored, 817 symlink policy exists for browse,
  not transfer, 825 an Android share target that sends inline on a thread, 851 directory errors reported
  while per-entry failures vanish, 873 poll ordering with no conflict rule, 881 server maps urgency the
  client then discards, 882 one notification channel rather than per-app, 885 a reply action, 893 same
  tag+id replaces rather than fuzzy dedupe, 894 pending queue in memory, lost on restart, 898 plugin unit
  tests only.
- **WRONG** 801-804, 807-816 assume a chunked resumable transfer protocol to extend — the shipped path is
  one `share.request` plus a back-connection and a 64 KB copy loop, so the real first item is designing
  that protocol (2851-2863); 826 the foreground service is streaming-only and is not a transfer manager;
  875/876/878 are dups of 874/255/259; 923 dup of 922.
- ABSENT: resume, chunk framing, parallel chunks, integrity hashes, dedupe/cache, compression, sparse
  files, metadata preservation, directories and multi-file transfers, client progress/history/retry/cancel,
  overwrite and executable confirmations, filename encoding policy, hidden-file policy, search/sort/filter,
  the entire file-browser UI (the server plugin answers, `listRemoteFiles` has zero call sites), previews,
  MIME and thumbnails, image/file clipboard types, clipboard search/pin/expiry, per-app notification
  grouping, channels, icons, privacy modes, snooze, rate limits, delivery receipts, and every soak/gate id.

### J. Window / Desktop Control / Workspace 901-1000

- **DONE** 901-905 window list, active window, geometry, titles and class/app-id with a phone-side picker,
  907-908 workspace snapshots pushed on register and on events, 941 monitor list with a picker, 958 privacy
  lock wired, 961-963 volume, mute and sink switching, 975-976 desktop battery and charging state, 980 the
  `sessions` verb, 981 `kick`, 985-986 window open/focus/move and workspace events.
- **PARTIAL** 906 pid serialized server-side and dropped by the client parse, 909 workspaces 1..9 via
  Super+N uinput with named workspaces display-only, 922 a generic `sh -c` exec export with no UI caller,
  942 name/size/fake-primary with an `index == 0` primary heuristic and no position, 954-957 sleep,
  hibernate, reboot and shutdown are reachable from the bridge but have **no UI caller anywhere**, 968
  Tailscale status from the bridge plus the CLI verb, no desktop-side query, 983 outcomes recorded with no
  recording metadata, 984 Hyprland and notification pushes only, 993 `available: false` fallbacks, 994
  clipboard clear confirms while power and exec do not, 997 scattered logs rather than an audit trail,
  998 one host-gated unit test.
- **WRONG** 910-921 is a fabricated continuum: the repo has a hard rule against Hyprland's broken socket1
  write dispatchers (hyprwm/Hyprland#16224) and window actions ride uinput keys, so close/focus/move/maximize
  are a design task (2584-2600), not a checkbox; 987 monitor-removal events are *deliberately filtered out*,
  so listing "detection" as near-done inverts reality.
- ABSENT: process/CPU/GPU/disk/network queries, uptime/kernel/env, monitor modes, refresh, orientation and
  scale fields, night-light/brightness/governor control, lid/idle/lock-state queries, per-session permission
  model, input-device/Bluetooth/Wi-Fi/VPN control, audio/power/health event streams, admin roles, undo
  history, and the desktop-control release gate.

### K. Virtual Display / Second Screen 1001-1100

- **0 DONE.** The audit found no virtual-output primitive of any kind: no `wlr_output_manager` headless
  create, no KWin/Mutter/Xvfb/DRM-lease code. `SessionType::VirtualConsole` is TTY detection, and
  `monitor_index` selects *physical* outputs.
- **PARTIAL** 1037 absolute-pointer mapping and normalized coordinates exist for touch input and are a
  prerequisite, not a second screen; 1050-1052 clipboard/notification/audio plugins are credited as
  second-screen "sidecars" by the old roadmap and are not; 1069 Android PiP of the stream exists.
- **WRONG** the whole premise of the range (a "phone-as-monitor" subsystem to extend) is absent — it is a
  project, now 2601-2650, and the old ids 1003/1006/1024-1052 that restate other sections are dups.
- What is actually feasible here: Hyprland ≥0.44 exposes `hyprctl output create headless`, with known
  upstream problems — resolution/refresh-rate control (hyprwm/Hyprland#5415) and black outputs on some
  versions (#12690) — so a virtual display must be built with capability detection and a documented
  failure path, on a compositor allowlist, not as a general Wayland/X11 promise.

### L. Automation / Macros / Productivity 1101-1200

- **DONE** 1198 the phone-side shortcut bar (grouped key-cap chips wired through `RemoteScreen` and the
  quick-settings sheet). That is the range's only completion.
- **PARTIAL** 1151 desktop notifications are *captured* (`notification_monitor.rs`) but nothing can act on
  them, which is the nearest thing to a trigger the tree has.
- **WRONG** 1116-1128 restate an existing plugin or input primitive as a hypothetical "macro step" (key,
  mouse, text, volume, monitor, window, workspace, clipboard, notification actions all exist as direct
  controls); 1160, 1188-1195, 1196 and 1200 are research, test and doc chores rather than features.
- 97 of 100 ids presuppose a macro/automation engine that does not exist in any form: no DSL, no recorder,
  no scheduler, no trigger system, no templates, no hooks (HTTP/MQTT/shell), no dry-run, no audit history,
  no marketplace. This range is **one speculative epic, not 100 tasks** — it is deferred wholesale to
  2981-3000 and must not be scheduled before the fundamentals in U/V/W/X are green.

### M. Multi-device / Ecosystem 1201-1300

- **DONE** 1203 multiple saved desktops (the phone's multi-host store and the home screen list), 1225
  per-device trust with grants plus `pair`/`unpair`, 1235 the live-session registry keyed by `device_id`
  and transport, 1236 the live-session block in `linux-link status`, 1255 mDNS register and browse, 1256 the
  tailnet discovery poller plus `watch`, 1290 sha256 verification of a downloaded release.
- **PARTIAL** 1202 the registry admits N distinct device ids (it evicts only same-id reconnects) but two
  phones streaming at once has never been run, 1204 the dual-pane layout engages above 600 dp and has never
  been verified on a tablet, 1226 peer online comes from the tailnet, not from a phone-presence registry,
  1228 capabilities are exchanged at handshake and never persisted, 1229 identity carries device type and
  name but no OS version, 1238 same-id eviction is silent last-writer-wins with no consent, 1241 each
  connection spawns an independent pipeline with no shared-viewer awareness, 1247/1248 `pair --grant` is the
  seed of a support mode but is neither guest nor agent mode, 1250 the shield and `notify-send` mark a
  session live without any viewer count, 1281 the installer is single-host, 1296 `--rollback` is single-host,
  1230 desktop→phone battery exists and the reverse does not.
- **WRONG** 1257 dup of 1255 (LAN discovery *is* mDNS here), 1259 dup of 1432 (PIN pairing), 1270 dup of 1416
  (saved hosts); 1205-1211 are five un-written research documents, and 1271-1296 label one user's install
  script as fleet management.
- The load-bearing absences, which the plan treats as prerequisites for anything multi-device: 1242-1244
  **there is no controller arbitration at all** — every paired client can inject keyboard and mouse
  simultaneously and nobody owns the cursor; and 1216-1224 there is no per-device policy of any kind, since
  every knob lives in one global `config.toml`.

### N. Discovery / Networking / Tailscale / WAN 1301-1400

- **DONE** 1301-1302 tailscale status and peer list (plus `linux-link peers`), 1308 direct addresses seeded
  before relay, 1309 default relay mode with a relayed flag, 1312 peer online check, 1324-1325 mDNS service
  registration and the LAN discovery service, 1326 LAN scan, 1329 version TXT record, 1335 endpoint packet
  carries addresses, 1354 the non-default `wan` feature gate, 1356-1358 lan/wan-direct/wan-relayed path
  classification with session outcome telemetry, 1359 relayed state polling, 1361 jittered backoff, 1368
  monotonic timing, 1369 WAN identity cache plus persisted trust, 1374-1375 WoL send and per-host MAC
  storage, 1383 peer discovered/offline events, 1386-1387 goodput and RTT exposure, 1392-1395,1398 the
  chaos proxy (drop, latency, jitter, blackhole) and its reconnect-storm test.
- **PARTIAL** 1304 MagicDNS resolved through the peer list rather than the API, 1306 first IP picked blindly,
  1316 NAT type inferred post hoc from a punched session, 1327 TXT record carries name+version only, 1334/1335
  endpoint advertisement with no discovery-side policy, 1339 the pairing gate restricts plugins while mDNS
  keeps advertising, 1341 a 6-digit PIN standing in for a link code, 1346 unparseable dial addresses skipped,
  1360 a UI cooldown, 1362 a 30 s delay cap with no attempt budget, 1366 future-stamp tolerance, 1370 cache
  cleared on disconnect with no TTL, 1376 relay wake needs a hand-entered relay host, 1377 WoL send
  confirmation without an up-check, 1378 a connect timeout rather than a wake-wait, 1384 online as a boolean,
  1389 raw loss counter with no estimate, 1399 a loopback iroh dial test, 1400 tests run in CI with no
  networking-specific gate.
- **WRONG** 1307 dup of 1306, 1328 dup of 1329, 1334/1335 collapsed, 1352/1353 both only mirror the
  compile-time `wan` feature, 1372/1373 the same prewarm idea; 1319 names `coffee_shop_wifi` — that is a
  chaos-preset label, not captive-portal detection; 1379 `send_wol_with_retry` exists and has no callers,
  so "wake retry" is dead code, not a gap to design around.
- ABSENT: tailnet identity/node-key handling, IP-family policy, path ranking and probing, firewall and MTU
  diagnostics, symmetric-NAT and captive-portal classification, metered policy, address allowlists/CIDR,
  QR/URI pairing and deep links, DNS cache and prewarm, sleep/hibernate wake integration, connectivity
  scoring, reorder injection and bandwidth throttling in the chaos proxy, and a WAN CI suite.

### O. Android UX / App Architecture 1401-1500

- **DONE** 1402-1404 Material You with a preference gate plus light/dark with a matching night window
  theme, 1411 the haptic taxonomy behind a preference, 1432 manual PIN pairing, 1435-1436 failure copy with
  a "Pair again" escape hatch, 1448 the error translation registry, 1483 161/161/161 strings across en/es/ta
  with identical key sets and a per-app locale screen.
- **PARTIAL** 1401 design tokens exist while `Type.kt` is a bare `Typography()`, 1406 semantics limited to an
  optional `contentDescription`, 1407 default typography honors font scale but the HUD hardcodes 11.sp,
  1408-1409 some icons pass `null` descriptions, 1415 hosts persist locally with no offline model, 1416 host
  store, 1422 "last host" is the first entry, 1430 the pairing sheet has pending/ready states, 1433-1434
  waiting status and a success toast without progress, 1441-1443 the HUD, the WAN badge and top chrome exist
  with no diagnostics actions, 1444 relay folded into the WAN banner, 1445 backoff is core-side while the UI
  shows a generic link-dropped toast, 1451-1453 settings is five headers and five stored preferences,
  1457-1459 input mode, mic and clipboard live only inside the session, 1469-1471 inline permission prompts
  and denial strings without rationale screens, 1477 client capability reporting limited to mic availability,
  1480 the reflective haptic constant, 1487-1488 translations complete by count and unvetted, 1490 error copy
  that leaks a raw Rust string in the view-only case, 1491-1492 thin empty/pending states, 1495 fading chrome
  that is not customizable.
- **WRONG** 1405 dup of 1402 (the preference row literally says "Material You colors"), 1409 dup of 1408,
  1413 dup of 1412, 1443 dup of 1442, 1440/1439 are the same missing screen, 1437 is not a UX gap alone: the
  bridge's `list_trusted_peers`/`forget_trusted_peer` exports (and the computed certificate fingerprint) have
  **no caller**, so "device list", "revoke" and "show fingerprint" are one missing screen (2931-2934), and
  1473 refers to exact-alarm scheduling the app never uses.
- ABSENT: touch-target and a11y audits, reduced-motion handling, per-app notification settings, privacy and
  power settings sections, advanced/developer/experimental/diagnostics/benchmark screens, a permission
  centre, battery-optimization guidance, host tags/notes/favorites/search/sort/import/export, QR pairing,
  deep links, in-app session list and active-session card, reconnect countdown, offline UI, retry states,
  support bundle, skeletons and offline banners, HUD customization, foldable/tablet layout polish beyond the
  shipped `FoldableLayout`, plural forms, RTL, and any Compose test (there are none).

### P. Linux CLI / Desktop UX 1501-1600

- **DONE** 1515 `RUST_LOG`/`log_level=trace` through an `EnvFilter`, 1528 `notify-send` when a session
  connects, 1546-1548 `pair` / `unpair <id>` / trusted list reachable from `capabilities`, 1551
  `linux-link sessions`, 1580 Tailscale status in `status` with a boot-time wait and graceful degrade.
- **PARTIAL** 1507 `live_sessions.json` is a machine mirror while `status` prints prose, 1510 telemetry rows
  are tab-separated and machine-readable while the CLI renders text, 1514 **`-v` is a dead flag** — declared
  in `cli.rs:7` and never read by `main.rs`, 1516 `tracing` key-value fields exist with a text-only
  formatter, 1517 stdout reaches the user journal with no structured fields, 1521 colour follows tty
  defaults only, 1524 `pair` prints and notifies but there is no interactive flow, 1527 a PIN-shown
  notification without a success one, 1529-1530 disconnect and failure are log lines, 1552 outcome/duration/
  path rows with no replay metadata, 1564 health is a pid file plus a JSON mirror with no endpoint, 1565-1567
  `Restart=on-failure` exists with no `WatchdogSec`, no `Type=notify`, and no reporting of why a restart
  happened, 1570-1574 the unit orders after PipeWire/wireplumber and the installer probes the environment,
  but the server itself never checks, 1575-1579 real runtime verification exists (uinput tried first with a
  runtime fallback, VAAPI `renderD*` enumeration, the throwaway NVENC encode) with **no command that
  surfaces it**, 1582 installer disk check only, 1586-1589 serde defaults, cert load, grant GC and bind-failure
  context with no validate or preflight verb, 1591 `check_mdns_available()` has a test-only caller, 1592
  relay exposure is only inferable after the fact, 1593-1595 and 1597 belong to `install.sh`, not the binary,
  1598 only the `--grant` parser has unit tests.
- ABSENT: every shell completion (1501-1505, no `clap_complete` dependency), aliases, a `--json` flag on
  anything, quiet mode, syslog, log file selection and rotation, progress UI, terminal QR, a tray icon or
  desktop entry (zero appindicator code in the tree), CLI-side monitor/window/quality/codec/audio pickers
  (all of these are phone-only today), trust export/import, a benchmark command, a diagnostics archive, a
  doctor command, permission/memory/CPU/thermal/firewall checks, self-test, and a release gate.

> Naming the boundary, because the old range blurred it: `linux-link` is exactly ten verbs —
> `start|stop|status|sessions|list|watch|capabilities|connect|pair|unpair|kick` (`server/src/cli.rs:15-66`).
> Install, update, rollback, uninstall, `--check-updates` and `--status` belong to `scripts/install.sh`.
> `README.md` documented `linux-link --status/--check-updates/--list-versions/--rollback/--uninstall` and a
> `--config` flag that the binary does not accept; corrected with this rewrite.

### Q. Observability / Telemetry / Diagnostics 1601-1700

- **DONE** 1635 the e2e EWMA (¾/¼ smoothing) in `core/src/streaming/client.rs:57` feeding the HUD field,
  1640 the compositor-true e2e probe (capture age + RTT/2 on the desktop clock), 1642 RTT in the HUD and in
  session telemetry, 1654 the graded `SessionOutcome` classification with a rotating log and
  `linux-link sessions`, 1655 the lan/wan-direct/wan-relayed tally with the relayed share printed.
- **WRONG** 1666 asks for an "anonymous mode" on telemetry that is a local file by construction — there is
  nothing to toggle until something is ever sent anywhere.
- That is the whole range: five shipped ids, all of them averages. No spans, no traces, no percentiles, no
  histograms, no exporters, no dashboards, no correlation ids across the pipeline, and no measurement of the
  decode→panel half of latency. Everything real in Q is re-expressed as work in 2131-2230.

### R. Performance / Memory / Power 1701-1800

- **DONE** 1741 one shared v1 control connection instead of a throwaway per query (`CONTROL_WRITER`,
  commit `90d92df` — this closes the reconnect-storm finding recorded on 2026-09-22), 1777 adaptive bitrate
  wired through a watch that rebuilds the encoder at the new rate.
- **PARTIAL** 1756 the controller gates *decreases* on three consecutive readings but debounces increases on
  a smoothed average, so the hysteresis is asymmetric by accident rather than by design.
- ~97 ids absent: there is no profile of any kind, no allocation or copy accounting, no pool, no memory or
  CPU budget, no thermal or battery policy, no frame-drop floor guarantee, and — decisively — no benchmark,
  so nothing in this range can currently be regression-tested. See 2166-2210.

### S. Reliability / Recovery / Chaos 1801-1900

- **DONE** 1857 the reconnect-storm test over a real QUIC transport, 1863 the chaos proxy (loss, latency,
  jitter, blackhole) driving real sockets in `chaos_integration.rs`.
- **PARTIAL** 1802 `ExponentialBackoff` is complete and crosses the bridge, but no Kotlin caller uses it —
  the retry button is a plain restart, 1806 `is_retryable()` is classified core-side and shipped as a DTO
  flag nobody reads, 1809 the phone rebinds the stream on a default-network change with a 3 s cooldown and
  there is no server-side equivalent, device-unverified.
- **WRONG/dup** 1803 and 1804 are two more ids for the same backoff struct's `jitter` and `max_delay` fields,
  1805 dup of 1806, 1860 and 1864 dup of 1863.
- The ~50 named failure simulations (compositor restart, PipeWire death, encoder crash, decoder flush,
  Android process death, disk full, clock skew, certificate expiry mid-session, relay outage) have **no
  harness**, and every fuzz id in this range has no target. Honest content of S: four lines.

### T. Testing / CI / Packaging / Release 1901-2000

- **DONE** 1919 clippy `--workspace --all-targets -D warnings` in CI, 1920 `cargo fmt --all -- --check`, 1958
  `create-release` on `v*` tags, 1997 the two GitHub issue templates.
- **PARTIAL** 1904 a forward-compatibility regression test for the `llVersion` stamp and unknown-field
  tolerance, with no cross-version client/server pair matrix, 1922 `cargo audit` present but `|| echo`, so it
  cannot fail a build, 1955 sha256 sums published with nothing signed.
- 1920 and 1919 are listed as DONE because the gates exist and are correct; **`main` nonetheless fails the
  fmt gate on every recent push**, which is exactly what a gate is supposed to make impossible and why
  "CI is green" is the plan's first exit criterion rather than an assumption.
- ABSENT: any test matrix (compositor, distro, Android API level, transport), an Android CI job at all, a
  `--features wan` build, unit-test framework or instrumentation tests for Kotlin, deterministic and seeded
  tests, fuzz targets, soak and long-duration runs, memory-leak and file-descriptor checks, flake detection
  and quarantine, coverage measurement, packaging for any distro family beyond AUR, dependency review,
  signed or reproducible artifacts, SBOM, and the "release gate" ids. What exists is one Ubuntu job, 282 Rust
  tests, an `aur/` PKGBUILD with `sha256sums SKIP`, and `install.sh`.

### X. Additional Expansion Backlog 2001-2050

- **DONE** 2030 Hyprland socket1 read-side IPC driving the workspace HUD and window pull (writes
  deliberately ride uinput, per hyprwm/Hyprland#16224).
- **PARTIAL** 2006 sixteen in-process plugins behind `PluginRegistry` — real, but not a third-party SDK.
- **WRONG/dup** 2007 dup of 2006; 2028-2032 assume compositor IPC that Sway and niri never received in the
  form described, and the block as a whole understates the tree in the direction the rest of the old
  document overstated it: 2006 and 2030 already ship.
- Note the one item this rewrite closes outright: the reconnect-storm / `Broken pipe` finding recorded on
  2026-09-22 was root-caused to the bridge opening a throwaway control connection per query and fixed in
  `90d92df`, so any id promising "fix TCP control churn" (1741, and its restatement in S) is DONE as of
  today.

## New work: ids 2051-3000

These are written from the gaps the audit produced, not from feature nouns. Each is one shippable unit
with an observable done-condition; where an id only makes sense as part of a larger change, the range says
so. All of them are ABSENT today unless marked otherwise.

| Block | Area | Ids | Count |
| --- | --- | --- | --- |
| U | Defect & correctness backlog | 2051-2130 | 80 |
| V | Observability & measurement | 2131-2230 | 100 |
| W | Testing, CI & release engineering | 2231-2350 | 120 |
| X | Security, consent & auditability | 2351-2430 | 80 |
| Y | Transport & protocol completion | 2431-2530 | 100 |
| Z | Capture, compositor & display control | 2531-2650 | 120 |
| AA | Input fidelity & HID | 2651-2740 | 90 |
| AB | Media pipeline: encode, decode, audio | 2741-2850 | 110 |
| AC | Files, clipboard & notifications | 2851-2930 | 80 |
| AD | Product surface: Android & Linux | 2931-2980 | 50 |
| AE | Ecosystem, packaging & research | 2981-3000 | 20 |
| | **New work total** | | **950** |

### U. Defect & correctness backlog 2051-2130

Things that are **wrong or misleading right now**. This is the highest-value block in the document because
each item is a live defect with a known location, not a wish.

**Reported numbers that are not real**
- 2051 **LANDED 2026-09-24** make the session HUD's dropped-frame counter real — `frame_drops` was a literal
  `0` in the bridge's stats struct, so the UI asserted a healthy link it never measured. It now counts the
  holes in the server's per-frame sequence numbers as seen by the client's receive loop
  (`missed_video_frames` in `core/src/streaming/client.rs`): the one place that can know a frame never
  arrived, whatever dropped it. Measured as `highest - lowest + 1 - arrived` so out-of-order stream
  completion self-corrects instead of accumulating phantom drops, with the window restarted on sequence 0
  because a rebuilt encoder counts from 0 again. Three unit tests, including both of those cases.
- 2052 **OBSOLETE — superseded by 2051**: the field is a measurement, so there is nothing to remove.
- 2053 **LANDED 2026-09-24** wire the packet-loss input of the adaptive-bitrate controller — `update_loss`
  took `_lost_packets` and ignored it, so loss never moved the rate. It is wired, but not into that hook:
  the loss response is a stateful controller (it needs ratios over time and a way back up), so it lives as
  `LossCeiling` in `core/src/streaming/bitrate.rs` and is folded into the *live* rate owner — the arbiter
  task in `streamer.rs`, which already composes the relay floor and the HUD preset — as
  `configured.min(relay_cap).min(preset_ceil).min(loss_cap)`. The input is the transport's own cumulative
  counters (`lost_packets` / `datagrams_sent` from `ConnectionStats`, reported by both quinn and iroh),
  sampled per 2 s tick: above 1 % loss the ceiling drops 20 %, below 0.1 % it climbs 10 % until loss stops
  being a term at all, with a 100-datagram floor so a still desktop's near-empty sample cannot cut a
  healthy link, and a 1 Mbit/s floor because below that the right answer is fewer pixels, which this
  controller deliberately does not do mid-session. Eight unit tests on the pure decision function.
  **Found and closed alongside:** the RTT controller those hooks belonged to (`AdaptiveBitrate`,
  `BitrateProfiles`, `AdaptiveBitrateMonitor`, and the `StreamingServer::with_adaptive_bitrate` door that
  nothing ever walked through) had never seen a connection — no RTT-driven adjustment had ever happened, so
  the README's "RTT-based congestion control" described dead code on top of a no-op. Deleted rather than
  revived: a second owner would fight the arbiter that exists. 2821 (loss + RTT together, à la RustDesk)
  stays open as the refinement, and it should be done *in* this arbiter.
- 2054 **LANDED 2026-09-24** stop presenting desktop audio as available: the phone has no player
  (`receiveAudio` has no caller), so either build the playout path (2791-2800) or drop it from the
  advertised capability set. The second half, in three steps — the first one was not a doc fix.
  **Hazard found while verifying this:** the unconsumed audio queue was not merely idle.
  The client's one demux loop handed every audio packet to an 8-slot channel with `send().await`, so a
  consumer that never polls (the Android app today) fills it 160 ms in and then **stalls video behind it** —
  and because the desktop-side audio task fell back to streaming synthesized silence whenever PipeWire
  capture was unavailable, that was not a rare interleaving but the normal shape of a session on such a
  machine. A closed audio receiver also `break`s the loop, tearing the whole session down for an
  audio-only teardown.
  Audio is now offered, never insisted on (`try_deliver_audio`: skip when full, log once when the consumer
  is gone, video unaffected), proven by two unit tests; the silence stream is gone — with no PipeWire
  capture the audio task logs the reason and ends, so a session carries audio only when there is audio to
  speak of; and `capabilities.rs` gained `AudioFormat::played_on_the_receiving_end`, so the generated
  report and `docs/capabilities.md` state that `desktop -> phone` reaches the wire and no speaker, while
  `phone -> desktop` does reach one (`mic_relay.rs` → `pw-loopback`). The stale half of this item is now
  2791-2800: the playout path that would turn that row's `**no**` into a `yes`.
- 2055 **CLOSED by Phase 1 step 3 (`7a1623e`), before this range was worked**: the e2e number stopped being
  only an EWMA. The same per-frame sample that seeds the HUD's smoothed value is pushed raw into the client's
  `SampleBatch` and shipped as `SAMPLE_E2E` (tag 14), which the server folds through the *same*
  `metrics::Samples` reservoir that builds `enc_*`, so a session record carries `e2e_p50/p90/p95/p99/max`
  and a p95 regression is visible in it (`session_telemetry` asserts the tail, `session_record.rs` asserts
  it end to end over loopback QUIC). What is left of this item is the live HUD's single scalar — a p95 you
  can watch while streaming rather than read afterwards — which is 2141-2146, not a missing measurement.
- 2056 **CLOSED (`42edf12`, `e7ac404`)**: the HUD's rates stopped being a lifetime average and started
  naming their basis. The bridge keeps the counter reads the display has asked for and diffs them across
  the newest 3 s (`android/bridge/src/rates.rs`), and the link figure is the median of the session's
  recent one-second polls instead of whichever poll happened to be current; the DTO carries the span a
  rate was divided by and the sample count behind a median, and `StatsHud` prints both under the numbers
  and dims a figure whose basis is still thin. **Confidence is a stated sample count, not an interval,
  and that is the ceiling:** quinn 0.11's public RTT surface is one smoothed `Connection::rtt()`, and the
  `PathStats` iroh 1.3 re-exports per path carries `rtt` with no variance beside it — the only place
  either stack names a variance is the qlog-only `RecoveryMetrics` diagnostic, which is not an accessor
  an application can read. Successive samples are therefore all the evidence a transport figure here can
  have. Device behavior unverified (the caption's wording and width are a phone question).

**Input paths that silently degrade**
- 2057 **CLOSED (`295ff1c`, `dc9fc22`)** complete the server keycode map. Fixed in the order the
  diagnosis required: first the architecture, then the table. The uinput backend routed every key through
  the enigo table on its way to a kernel that only wanted the number (`295ff1c`) — so the key the phone
  pressed became a `Key`, then a *different* evdev code, then `KEY_UNKNOWN` when the table's reverse
  lookup found nothing, which is why Wayland input "degraded" on the backend with no table dependency at
  all. uinput now emits the code off the wire and the table exists only for the X11 rung that genuinely
  needs a keysym name. There it went from 26 rows to 110 (`dc9fc22`), and its codes are now written as
  `evdev::KeyCode` names rather than numbers: a wrong number silently injects another key, a wrong name
  is a compile error.
- 2058 **CLOSED (`dc9fc22`)**: every modifier the phone sends is in the table — both Shifts, both
  Controls, left Alt, left Super, plus Caps Lock and Num Lock. Right Alt and right Super go through
  `Key::Other` with the X keysym, because enigo names no right-hand variant for either on Linux; that is
  the only place in the table where the key is identified by a number rather than a name, and it says so.
- 2059 **CLOSED (`dc9fc22`)**: volume up/down and media next/play-pause/previous map. `KEY_MUTE`
  deliberately does **not** — the client has no control that sends it, and the table's contract test
  covers what the client can emit, not what a future one might.
- 2060 **CLOSED (`dc9fc22`)**, with the item's premise corrected: there is nothing to stop dropping
  F13+ *client-side* because Android has no keycode above F10, so this client cannot name those keys.
  `KEY_MENU` (139) now maps — to enigo's `LMenu`, which is the X `Menu` keysym, the application-keys key;
  it is not `KEY_COMPOSE` (127), which is what the client's own comment claimed for the row. F13-F24 are
  in the server table anyway, so the X11 rung is no weaker than uinput, which will emit any code it
  declares.
- **Found while enumerating this group** (`bf88325`, client side): Android F11/F12 were derived by
  continuing the F1..F10 arithmetic, so the phone's F11 pressed **KEY_NUMLOCK** and its F12 pressed
  **KEY_SCROLLLOCK** (evdev 69 and 70, where the real codes are 87 and 88). The bridge test had been
  asserting the contiguous arithmetic as if it were correct, so the defect had a passing test protecting
  it. No in-app control sends F11 or F12 today, which is why this stayed latent: the fix is guarded by a
  unit test, not by a checklist step.
- **Everything above is unit-verified.** This box injects through uinput, so the completed table — the
  enigo rung — has not been exercised on a real X11/XWayland session; checklist §3 now asks for the
  keyboard list to be run once per backend.
- 2061 **CLOSED (`60ae055`)**, device-unverified: direct-touch now presses. A one-finger drag warps to the
  touched point, sends `BTN_LEFT 1`, moves while held and releases on lift; the release that used to be the
  gesture's only button event no longer lands on a button that was never down. A pinch that began as a drag
  gets its release too — a leaked press is a stuck drag. Not closed by this: the server has no release-all
  for a release packet lost mid-drag (2272), so a session that dies while held leaves the button down.
- 2062 **CLOSED (`60ae055`)** — routed through `tapAbsolute`, not deleted: the no-movement case (a tap that
  never crossed touch slop) is exactly warp + press + release, which is what that function always did. It
  also had a doc comment claiming the server's absolute motion "puts the virtual finger down", which the
  injector contradicts line for line; the comment now says what is true.
- 2063 **CLOSED** (`bcbec8e`), gamepad press/release paired: the packet is a state snapshot, so the handler
  now diffs its button mask against the previous one and presses on a rising edge, releases on a falling
  one.
  Previously every mapped bit re-pressed on each packet and nothing was ever released, so one DPad tap left
  an arrow key held for the rest of the session. The mapping moved from a seven-branch `if` chain to a
  table, and a test asserts the release set equals the press set for all seven bits and that unmapped bits
  reach nothing. Unit-verified only, and honestly so: the path still has no Android source (2695), so no
  control in the app can produce these packets today.
- 2064 **CLOSED** (`3db2f5d`), uinput devices rebuilt on fault: both emits now go through one helper that retries the
  write against a freshly built device once, logs the fault with the device's name and the action that hit
  it, and reports the second failure if the rebuild did not help. Previously the two devices were built at
  startup and never revisited, so one `write(2)` fault (a destroyed uinput device, an ENODEV) failed every
  later packet of the session identically and the only trace was an error string. The abs device also keeps
  its lazy build, so a session that never touched direct-touch still gets one on first use. Proven by a
  test that retargets a live device's descriptor onto a read-only `/dev/null` with `dup2` — the write then
  fails `EBADF` exactly as a dead device's would — and asserts the emit recovers on both devices; it
  self-skips where `/dev/uinput` is absent. Honest limit: the retried packet itself may still be lost,
  because libinput opens a newly created device asynchronously, so what the rebuild guarantees is recovery
  from the *next* event.
- 2065 **PARTIAL — converted into a named defect with a repro (`fc1b368`), which is what this plan's exit
  gate asks for.** The assumption is no longer invisible: the uinput injector now reads the desktop's keyboard layout
  (`XKB_DEFAULT_LAYOUT`, then `localectl status`'s `X11 Layout:` line) when it is built and says what it
  found — `debug!` when the layout is the `us` one its codes were chosen for, a `WARN` naming the layout
  when it is not, a `WARN` about assuming `us` when neither probe answers. It does **not** reject, and the
  reason is a design decision, not an omission: the phone's ordinary key events are position-based too, so
  refusing a non-`us` desktop would take the keyboard away from every non-US user rather than fix them. The
  fix is 2665, and its acceptance test is this repro: start the server with `XKB_DEFAULT_LAYOUT=fr`, tap
  `a` on the phone's keyboard, and the desktop receives `q` while the journal says
  `desktop keyboard layout is not 'us' … layout=fr`. Both branches of the report were observed on this box
  (which reports `us`); the classification itself is unit-tested, including `us(intl)` and a `us,de` group
  list, and the `localectl` parser against that tool's real output.

**State that goes stale or is dropped at a boundary**
- 2066 **CLOSED (`210f240`; the reaction itself is hardware-unverified)** — output add/remove no longer
  fall on the floor. The audit's wording was overstated and is corrected here: the phone never *kept* a
  monitor list that no longer exists, because `kdeconnect.linuxlink.monitors` is answered from a live
  `enumerate_monitors()` on every query (so the picker refetches current state on open) and `service.rs`
  already re-pushed a full `hyprland.state` snapshot on a 4 s tick. What was actually true is that **no
  compositor output event drove anything**: `monitoradded`/`monitorremoved` were emitted, read, discarded,
  and the HUD's screen box plus every window's crop geometry could sit wrong for up to a tick after a
  display vanished. Now those events trigger an immediate snapshot re-push to all clients, logged with the
  event name and data. They deliberately stay out of the forwarded *event* stream — the refresh rides the
  state packet, and the client's `applyEvent` has no handler for an output delta (a real monitor-list push
  is 2575). The two names were confirmed against this Hyprland's own binary (`strings`); `monitorchanged`,
  which the KDE-Connect-era mental model expects, is not emitted on 0.56.2, and an earlier draft of this
  fix nearly handled a `defaultkb` key that `hyprctl devices -j` does not have. Unit-tested for the
  classification and for "an output event produces no HUD delta". Residual, with the reason: no physical
  unplug has ever been observed — this box has a single built-in output and Hyprland's socket1 write
  dispatchers are broken upstream (AGENTS gotcha), so there is no fake hotplug; checklist §6 carries the
  watch-the-log check, and the capture-side restart on hotplug is 2567.
- 2067 carry window `pid` through the client parse (serialized server-side, thrown away).
- 2068 apply the notification urgency the server computes instead of `IMPORTANCE_DEFAULT`.
- 2069 group desktop notifications per app instead of one channel for everything.
- 2070 use a real notification icon rather than the static chat glyph.
- 2071 stop dropping per-entry failures in a directory listing, and make broken symlinks visible rather
  than vanishing.
- 2072 resolve the trust-store `known_peers.json` corruption path loudly — an unreadable file currently
  resets every TOFU pin silently, which reads as an attack to a user and as nothing to the log.
- 2073 intersect the capabilities the v2 identity packet carries; today they are transported and ignored.
- 2074 finish or remove the persistent-stream demux: stream kinds that are matched and then dropped make
  the multiplexer look complete when it is not.

**Dead surfaces that should be wired or deleted** (each is currently a maintenance liability and a
false-positive for any future audit)
- 2075 power plugin: bridge export exists, no UI caller — ship the desktop-power row or remove it.
- 2076 exec: no UI caller; see 2351-2356 before ever exposing it.
- 2077 `listRemoteFiles`: no UI caller.
- 2078 `list_trusted_peers` / `forget_trusted_peer`: no UI caller.
- 2079 certificate fingerprint: computed, never shown.
- 2080 `reconnect_streaming` with its backoff: no app caller; the retry button bypasses it (2081).
- 2081 make the session retry use the bridge's backoff reconnect instead of a fresh stop/start.
- 2082 `send_wol_with_retry`: no callers; delete it or give wake a real retry policy.
- 2083 `core/src/streaming/capture_x11.rs`: re-exported, never invoked, and the source of the roadmap's
  XShm claims — delete the module or make it the X11 backend.
- 2084 `use_datagrams`: dead configuration field; either build the datagram plane (2491-2500) or delete the
  flag so no one plans against it.
- 2085 the `ChannelKind::File` arm that accepts a stream and ignores it while file transfer rides the TCP
  share plugin (2502 decides which plane owns transfer).

**Security defects, not features**
- 2086 write `identity.key` with owner-only permissions (only the iroh secret is `0o600` today).
- 2087 stop logging pairing PINs in plaintext.
- 2088 canonicalize before the home-prefix jail check so a symlink cannot escape it.
- 2089 bound the exec plugin's output and give it a timeout before it is ever reachable.
- 2090 do not advertise the service over mDNS while `pairing_required` is set, or make the advertisement
  explicitly say "locked".
- 2091 cap PIN attempts — a 6-digit PIN with unlimited tries inside its 5-minute TTL is a brute-force
  target on any LAN peer.
- 2092 confirm before overwriting a file on the desktop (`share.rs` joins dir+name and writes).

**UI truth and accessibility defects**
- 2093 don't interpolate raw Rust error text into the view-only message.
- 2094 make the stats HUD honor font scale (11.sp is hardcoded).
- 2095 make HUD fade/size configurable rather than constants.
- 2096 guarantee touch targets in session chrome and the shortcut bar.
- 2097 query decoder secure-output capability instead of calling `setSecure` blindly.
- 2098 surface codec capability degradation as a notice, not silence.
- 2099 describe the session's link quality with one source of truth instead of four partial chips.
- 2100 give the reconnect path a visible countdown and reason.

**Harness and repository defects**
- 2101 **LANDED 2026-09-24** make `core/tests/chaos_integration.rs` allocate its own ports — it hardcoded
  `127.0.0.1:4716/4717` and failed `AddrInUse` against the running user service, so a passing suite and a
  green host were mutually exclusive. `ChaosProxy::new` now binds and exposes `local_addr()`, and the proxy
  counts forwarded/dropped so the suite asserts an exact invariant instead of inferring chaos from a
  100-packet sample (that inference failed 2.65% of the time on its own RNG).
- 2102 **LANDED 2026-09-24** restore a green `cargo fmt --all -- --check` on `main` (red for six pushes).
  Local `main` only — CI stays red until the work is pushed.
- 2103 **LANDED 2026-09-24** the seven clippy findings in `capture.rs` / `capture_x11.rs` / `streamer.rs`
  are fixed, and with them the ~20 behind them that the build stop was hiding; `cargo clippy --workspace
  --all-targets -- -D warnings` now passes, as do the `client`, `client,wan`, `wan` and `encode` profiles.
- 2104 make `cargo audit` fatal in `release.yml` (`|| echo` currently neuters it).
- 2105 run the thirteen `#[ignore]`d tests somewhere they can fail — right now they are documentation.
- 2106 stop the `wan` feature from rotting: compile it in CI (`cargo check -p linux-link-core --features
  wan`, and the bridge's `client,wan`).
- 2107 replace the "Release Ready" badge and `Phase 6 Complete` framing with measured status.
- 2108 purge the Flutter-era documents (`FIX_PLAN.md`, `ARCHITECTURE.html`, `CHANGELOG.md`) or the sections
  of them that contradict the Kotlin client — they are actively misleading to any reader or agent.
- 2109 publish a GitHub Release for `v0.1.0` (the tag exists, `gh release list` is empty).
- 2110 **LANDED 2026-09-24** the stale "`~35` files not rustfmt-clean" gotcha is out of AGENTS.md, which
  now states the opposite obligation: fmt and `-D warnings` are green, so they must stay green.
- 2111 pin the JNI export count to reality with a test instead of a prose number that drifts (docs variously
  say 54/57/58).
- 2112 give session telemetry a failure-reason taxonomy — `SessionOutcome` today cannot answer "why did
  30 % of sessions die at 14:00".
- 2113 aggregate `linux-link sessions` (rates by outcome, path, codec) instead of a newest-N list.
- 2114 make `live_sessions.json` and the `kick` request file atomic — the mirror is polled at 1 s and the
  request file is consumed by whoever sees it first.
- 2115 define `kick <prefix>` ambiguity behavior (≥6 chars today, untested).
- 2116 replace 4 s clipboard polling in both directions with an event, and fix the echo-tracking race that
  polling forces.
- 2117 make notification replies acknowledged rather than fire-and-forget.
- 2118 persist the pending-notification queue; it is in memory and lost on restart.
- 2119 stop ignoring configured width/height, or delete the keys: the encoder follows the real frame size,
  so the config lies.
- 2120 make the relay bitrate floor react to events, not a 2 s poll, so a bandwidth change materialises
  without a visible lag.
- 2121 require no hand-typed relay address for wake-relay; derive it from discovery.
- 2122 verify a WoL target actually came up instead of reporting "sent".
- 2123 give idle timeout a config key (fixed at 45 s today).
- 2124 replace the bounded-but-unbounded `read_to_end(10 MB)` framing read with a real limit + error.
- 2125 make a chaotic v1/v2 dual-stack state explainable: a device that registers on both planes currently
  has two lives and one identity.
- 2126 close the "connected but no video" state with an explicit diagnostic (the dead-link card covers the
  frame gap, not a zero-bitrate session).
- 2127 make the pairing failure distinguish "wrong PIN" from "not trusted" from "no route".
- 2128 ensure a server restart mid-session produces a reconnect, not a silent dead stream (the QUIC
  certificate persists; the transport recovery does not exist).
- 2129 make the Android side survive process death — there is no saved-state restore.
- 2130 turn every one of the above into a regression test as it is fixed, so the defect backlog does not
  refill (see 2341-2350).

### V. Observability & measurement 2131-2230

**Stage timing, per frame**
- 2131 instrument capture start/stop with a monotonic timestamp per frame.
- 2132 instrument encode submit→packet-out per frame.
- 2133 instrument first-byte-sent→first-byte-received per frame (transport).
- 2134 instrument decode-in→render-out per frame on the phone.
- 2135 join those four into one per-frame timeline keyed by frame id.
- 2136 define the frame-id/sequence space that makes 2131-2135 joinable (absent today: video has no
  sequence numbers usable for correlation).
- 2137 carry a compositor present timestamp where the backend can supply one.
- 2138 measure phone panel-latency separately (it is currently out of scope and the HUD implies it is
  included).
- 2139 compute and report the honest capture→panel number once 2138 exists.

**Distributions, not averages**
- 2140 add a rolling reservoir/HDR histogram type in `core` for latency samples.
- 2141 report p50/p90/p99/max for e2e latency per session.
- 2142 the same for encode time.
- 2143 for transport one-way delay.
- 2144 for decode time and render pacing.
- 2145 for input event→injection round trip (currently zero instrumentation).
- 2146 percentiles for frame-size distribution, as a bandwidth-efficiency signal.
- 2147 track jitter as a real estimate (chaos injects jitter; the client never measures it).
- 2148 track loss as a rate with the same rigor as RTT.
- 2149 track reorder and out-of-order arrival counts.
- 2150 publish a per-interval (1 s) summary rather than lifetime averages, so a regression has a time axis.

**Transport introspection** (this is where 125/128/129/161-164 of the old roadmap actually become work)
- 2151 surface quinn connection statistics (bytes in flight, cwnd where exposed) per session.
- 2152 surface stream-level backlog depth per channel kind.
- 2153 surface datagram queue state once 2491 exists.
- 2154 record RTT sample distribution per path, not one smoothed value.
- 2155 log and count path changes (migration events) per session.
- 2156 record connection-ID rotation events where the library reports them.
- 2157 expose whether a session was ever relayed, for how long, and why it left the relay.
- 2158 measure direct-vs-relay throughput delta per session.
- 2159 attribute a session's failure to a layer (capture/encode/transport/decode) in the outcome record.
- 2160 export transport counters over the CLI (`linux-link stats --json`) for scripted measurement.
- 2161 define the stable JSON schema for all of the above before anything consumes it.
- 2162 version that schema.
- 2163 keep the on-disk `live_sessions.json` mirror compatible with it or supersede it deliberately.
- 2164 add a redaction mode for any exportable telemetry (paths, titles, notification text).
- 2165 make redaction the default for anything a support bundle could carry.
- 2166 keep the ring buffer bounded and document its memory cost.
- 2167 emit a session-end diagnostic record with all percentiles in one place.
- 2168 retain the last N session records for post-hoc comparison.
- 2169 make `linux-link sessions` render that record instead of only outcome lines.
- 2170 make every metric reachable from the phone too, since that is where latency is felt.

**Resource observability**
- 2171 sample per-process RSS of the server at session boundaries.
- 2172 sample CPU time split capture/encode/transport.
- 2173 sample GPU/encoder busy time where the driver exposes it.
- 2174 count allocations on the frame path with a feature-gated profiler hook.
- 2175 track pipe/buffer copy counts on the BGRA path, the number that a zero-copy change must beat.
- 2176 measure phone-side decode CPU and battery drain per minute of session.
- 2177 measure thermal state transitions during a session (device-gated, but the API exists).
- 2178 record the encoder's fallback events as telemetry, not just a log line.
- 2179 record capture backend selection and each failed attempt as telemetry.
- 2180 record pairing events (start, success, wrong PIN, timeout) as telemetry.

**Product observability (local-only)**
- 2181 define the minimum local event set: session start/end with cause, path class, codec, resolution.
- 2182 add a per-feature usage counter that survives restart (sheet opens, preset changes, crop use).
- 2183 record which capabilities the phone actually used in a session, to size the dead-surface cleanup.
- 2184 keep all of it on-device with an explicit off switch.
- 2185 document what is stored where (privacy doc, not a policy page).

**Benchmarks — the prerequisite for every claim in the roadmap**
- 2186 create a `benches/` target for the frame path with a synthetic source (no hardware needed).
- 2187 benchmark the v2 framing encode/decode path.
- 2188 benchmark packet parse and dispatch.
- 2189 benchmark the capture→encoder handoff (memcpy cost per frame).
- 2190 benchmark BGRA→NV12 conversion for both software and VAAPI rungs.
- 2191 benchmark the bitrate controller's decision path (pure function, must stay cheap).
- 2192 benchmark the trust-store and capability lookups on the dispatch path.
- 2193 benchmark the audio Opus encode path.
- 2194 benchmark input packet build+parse at 1 kHz event rates.
- 2195 capture benchmark results to a machine-readable artifact in CI.
- 2196 fail CI when a benchmark regresses beyond a declared threshold.
- 2197 add a loopback latency benchmark (server→client on one host over QUIC) as the cheapest regression
  canary.
- 2198 add an encoder quality benchmark against a fixed clip (PSNR/SSIM via ffmpeg) behind a feature gate.
- 2199 keep the reference clip in the repo or fetch it pinned by hash, never "some sample".
- 2200 document the benchmark methodology so numbers are comparable across machines.

**Latency budget & enforcement**
- 2201 write down the latency budget per stage (capture/encode/transport/decode/render) as data, not prose.
- 2202 make 2135's timeline compare against that budget automatically at session end.
- 2203 declare the budget per link class (LAN, tailnet, relayed).
- 2204 add a target for input round trip distinct from video e2e.
- 2205 add a bandwidth budget per preset and assert it in tests.
- 2206 add a CPU budget for the server capture+encode path.
- 2207 add a memory budget for the bridge and the server.
- 2208 add a battery-per-hour budget for the phone.
- 2209 fail the release job when a budget is exceeded by more than the declared tolerance.
- 2210 keep the budgets in one file that docs and tests read (the "one source of truth" the old roadmap's
  Stage 0 asked for).

**Diagnosis UX**
- 2211 render a human-readable "why is this slow" summary from 2167.
- 2212 distinguish "network is the bottleneck" from "encoder is the bottleneck" in that summary.
- 2213 surface it on the phone where the user is standing.
- 2214 make it exportable as the support bundle (2401-2410).
- 2215 include the redacted config, versions and backend choices in it.
- 2216 include the last N session records.
- 2217 never include clipboard content, notification text or file paths in it.
- 2218 show capture-backend and encoder selection in a diagnostics screen (today only in logs).
- 2219 show link path, RTT, loss, and p95 e2e there.
- 2220 show per-device trust and what each device was granted there.

**Regression detection**
- 2221 store benchmark + budget results per commit in a branch the CI can read.
- 2222 compare a run against the trailing baseline, not the last release.
- 2223 open or update an issue automatically when a gate trips.
- 2224 record which measurement is allowed to be noisy and how much slack it gets.
- 2225 keep a per-machine normalization factor, since this project's hardware matrix is a laptop, a phone
  and whatever CI's VM is.
- 2226 make a hardware-dependent result never fail a general-purpose job (VAAPI/NVENC presence).
- 2227 mark results with the compositor and version they were produced on.
- 2228 do the same for the Android device model and OS build.
- 2229 keep the matrix small enough that every entry is actually run.
- 2230 review the whole observability layer once a quarter against what has actually been used to fix
  something, and delete what has not.

### W. Testing, CI & release engineering 2231-2350

**Fix the machine first**
- 2231 land a green `main` (fmt + clippy + tests) before any new gate is added.
- 2232 add `cargo fmt` to the developer loop as a pre-commit or editor requirement so 1920 stops being a
  surprise at push time.
- 2233 make the clippy gate meaningful: fix the seven pre-existing findings, then forbid `--allow` growth.
- 2234 run `cargo test --workspace` with `--features wan` in CI as a second job.
- 2235 run the bridge's `client,wan` core profile in CI.
- 2236 add a job that compiles the Android app (`assembleDebug`) — currently zero CI coverage for Kotlin.
- 2237 add `lintDebug` to that job with a findings budget so the count can only fall.
- 2238 cache the Gradle and NDK layers so an Android job is not a 10-minute tax.
- 2239 check the JNI surface against the Kotlin declarations in CI (export-name/package drift is a
  class of failure that today only shows up as a runtime `UnsatisfiedLinkError`).
- 2240 verify locale key-set equality (161/161/161 today) as a test, not a claim in AGENTS.md.
- 2241 gate the docs: fail on broken relative links inside `docs/` and `README.md`.
- 2242 run the `#[ignore]`d hardware tests on a self-hosted runner with the GPU/Compositor they need, or
  delete them.

**Test matrix, by subsystem**
- 2243 protocol: every `NetworkPacket` type round-trips through `to_wire`/`from_wire` with a fuzzed body.
- 2244 protocol: unknown top-level fields and an unknown `llVersion` are tolerated (extends 1904).
- 2245 protocol: a v1 client against a v2-only server fails with a distinguishable error.
- 2246 protocol: a v2 client with an unsupported version range is refused before any plugin runs.
- 2247 capability negotiation: an intersection test that proves a disabled capability cannot be reached
  even when the packet is hand-crafted.
- 2248 framing: oversized, truncated, and interleaved frame inputs (property test over the reader).
- 2249 demux: unknown stream kind closes with the protocol-violation code, not a panic.
- 2250 datagram path tests once 2491 exists.
- 2251 multiplexer: two devices, same id, last-writer-wins is asserted rather than incidentally true.
- 2252 multiplexer: two distinct ids both stay registered and both receive their own replies.
- 2253 dispatch: a plugin that panics does not take the connection down.
- 2254 dispatch: a slow plugin cannot starve the control stream (bounded per-plugin queue).
- 2255 pairing: PIN grant expiry, single-use, and TTL arithmetic under clock skew.
- 2256 trust store: corrupt, empty, partial-write, and concurrent-write files.
- 2257 TOFU: pin mismatch, cert rotation with confirmation, and the silent-reset defect from 2072.
- 2258 capture: backend selection table tests for every (platform, backend, override) combination.
- 2259 capture: a compositor that vanishes mid-frame is handled, not panicked on.
- 2260 capture: output add/remove/mode-change produce a correct new frame geometry.
- 2261 encode: each ladder rung's argument builder against a golden command line.
- 2262 encode: a sidecar that exits after N frames triggers the documented rung transition.
- 2263 encode: a sidecar that produces no output within the stall window is detected in exactly 2 s.
- 2264 encode: bitrate change mid-session rebuilds at current dimensions with an IDR first.
- 2265 encode: resolution change rebuilds without dropping the session.
- 2266 transport: newest-frame queue drops the right frames under a synthetic slow reader.
- 2267 transport: keyframe request throttle holds under a burst.
- 2268 transport: backoff sequence, jitter bounds, and cap.
- 2269 transport: a connection that dies mid-request produces a typed error, not a timeout in every caller.
- 2270 bitrate: hysteresis unit tests for both directions (fixes the asymmetry in 1756).
- 2271 **CLOSED (`a9b903d`, `db75497`, `dc9fc22`)** input: every keycode the client can emit has a
  server-side mapping. `core::input::keys::PHONE_EVDEV_KEY_RANGES` is the set; the bridge test
  `emitted_codes_are_exactly_the_contracted_set` asserts its table produces *exactly* those 91 codes, and
  the server test `every_key_the_phone_can_send_has_a_mapping` walks the same list against `KEYCODE_MAP`.
- 2272 input: press/release pairing per device — no synthetic path may leave a key down.
- 2273 input: view-only drops injection but not queries.
- 2274 input: absolute coordinate mapping round-trips through letterbox and crop.
- 2275 files: path jail tests including symlink, `..`, and absolute-path inputs.
- 2276 files: transfer resume/integrity tests once 2851-2860 define the protocol.
- 2277 clipboard: size cap, echo prevention, and history bounds.
- 2278 notifications: reply round-trip and the queue-survives-restart behaviour from 2118.
- 2279 power/exec: approval state machine tests (see 2351-2356) — refusal is the default.
- 2280 audio: capture-session lifetime test proving the PipeWire token is not cancelled early.
- 2281 audio: mic relay start/stop/respawn under a killed node.
- 2282 streaming stats: the HUD's fields are asserted against real counters, so 2051 cannot regress.

**Kotlin / Android tests (none exist today)**
- 2283 add a `test` source set and the JUnit/Robolectric wiring.
- 2284 unit-test `HostStore` including the legacy single-host migration.
- 2285 unit-test the clipboard history bounds and dedupe.
- 2286 unit-test `UiError` mapping tables (every raw prefix must map).
- 2287 unit-test `Prefs` defaults and persistence.
- 2288 unit-test the touch-mapping math (letterbox, scale, crop).
- 2289 unit-test codec MIME sniffing from NAL headers against captured streams.
- 2290 unit-test the dead-link window logic with a fake clock.
- 2291 instrumentation-test the pairing sheet end to end.
- 2292 instrumentations-test the session chrome fade/summon behaviour.
- 2293 instrumentation-test the permission flows (mic, notification, FGS types).
- 2294 snapshot or Compose-ui tests for the home, settings and error card.
- 2295 a bridge JNI smoke test on device that calls every export once and asserts no crash.

**Chaos & recovery drills**
- 2296 chaos: compositor restart mid-session.
- 2297 chaos: PipeWire daemon restart during audio and mic.
- 2298 chaos: encoder sidecar SIGKILL at 1 s, 30 s and 10 min.
- 2299 chaos: server restart with a client connected.
- 2300 chaos: Android process death while streaming, then relaunch.
- 2301 chaos: Wi-Fi → cellular → Wi-Fi path change on the phone.
- 2302 chaos: tailnet IP change under a live session.
- 2303 chaos: relay outage with a direct path available.
- 2304 chaos: bandwidth cliff (10 Mbit → 500 kbit) and recovery.
- 2305 chaos: 40 % loss with 200 ms jitter — assert the session degrades, never hangs.
- 2306 chaos: reorder and duplicate packet injection in the chaos proxy.
- 2307 chaos: clock skew between phone and desktop (±5 min) across pairing and telemetry.
- 2308 chaos: disk full during a file receive.
- 2309 chaos: certificate expiry during a long session (currently untested because expiry is unimplemented).
- 2310 a documented, repeatable recovery-drill script that a human runs before each release.

**Soak & endurance**
- 2311 an 8-hour continuous session test with fd and RSS sampling.
- 2312 a 30-minute encode soak asserting no IDR drift and bounded keyframe cache.
- 2313 an input soak: 100 k events, assert no stuck key and no queue growth.
- 2314 a reconnect soak: 1000 cycles asserting the trust store and caches do not grow.
- 2315 a transfer soak: 100 files, mixed sizes, asserting integrity.
- 2316 nightly runs of 2311-2315 with results kept as artifacts.

**Fuzzing**
- 2317 a `cargo-fuzz` target for the wire packet parser.
- 2318 for the v2 frame reader.
- 2319 for the capability/caps byte parser.
- 2320 for the JSON body consumers in each plugin (the real attack surface behind 2351-2356).
- 2321 for the file-browse path handling.
- 2322 for the H.264 NAL sniffing on the client (Malformed streams must not crash the decoder loop).
- 2323 seed every target from the existing tests' corpora.
- 2324 run targets for a fixed budget in CI and gate on zero new crashes.
- 2325 keep a crash corpus under version control with a minimization note per entry.

**Packaging & release**
- 2326 sign release artifacts (minisign or cosign) and make `install.sh` verify, not just hash.
- 2327 fix `aur/PKGBUILD`'s `sha256sums SKIP`.
- 2328 add a Debian/Ubuntu package job building a `.deb` with the user unit.
- 2329 add a Fedora/openSUSE build in a container.
- 2330 build Nix expression or add it to the matrix, since Hyprland users live there.
- 2331 publish a `linux-link-core`-only crate? decide explicitly and record the ADR.
- 2332 produce an SBOM (`cargo deny` + `cyclonedx`) in the release job.
- 2333 run `cargo deny` (licenses + advisories) as a real gate.
- 2334 add `cargo-audit` to CI (not only release) and make it fatal.
- 2335 generate third-party notices for both the Rust workspace and the Android APK.
- 2336 a reproducibility check: two builds of the same commit produce the same binary hash, or the release
  says which parts aren't.
- 2337 publish a GitHub Release with the APK, the server tarball, checksums and signed notes.
- 2338 an upgrade test: install N-1, upgrade to N, verify trust store and config survive.
- 2339 a downgrade/rollback test through `install.sh --rollback`.
- 2340 an uninstall test that asserts no orphan unit, state or config remains.

**Release gate definition**
- 2341 the gate list itself as a single script (`scripts/release-gate.sh`) that CI and humans run.
- 2342 it must include: both clippy profiles, fmt, tests incl. `wan`, Android build+lint, fuzz budget,
  chaos suite, soak (nightly, informational), benchmark thresholds, docs link check, locale parity, audit
  and deny.
- 2343 it must include the on-device checklist subset that cannot be automated, with a named human sign-off.
- 2344 record gate output as the release body, so notes are generated from measurement rather than prose.
- 2345 a documented rollback procedure with one command and a tested restore path.
- 2346 a compatibility statement per release: which client versions may talk to which server.
- 2347 enforce that statement with the cross-version matrix test from 2245/2246.
- 2348 fail the release if the `Current Status` section of AGENTS.md and README disagree with reality.
- 2349 fail the release if any roadmap id marked DONE in this document has no evidence link.
- 2350 keep this document machine-readable enough that 2349 is a script, not a review — which is the honest
  version of the old roadmap's 2040 "automated roadmap status parser".

### X. Security, consent & auditability 2351-2430

**Close the exec surface first**
- 2351 remove `ExecPlugin` from the default registry, or make it opt-in through config plus a per-request
  desktop confirmation.
- 2352 if it stays, give it a timeout, an output cap, and a working directory.
- 2353 an approval UI on the desktop (dialog or notification action) for every command.
- 2354 a per-device command allowlist, evaluated in the plugin, not in the client.
- 2355 audit-log every exec request with the resolved command, exit code and device id.
- 2356 a test proving an unpaired or grant-expired device cannot reach the handler, whatever the transport.

**Identity & secrets**
- 2357 move the phone's identity key into the Android Keystore (or at minimum EncryptedSharedPreferences).
- 2358 move the desktop's identity key into the secret service with a file fallback that warns.
- 2359 owner-only `0600` on every state file (key, trust store, known peers). **DONE (2026-10-14)** — the server makes the state folder `0700` at start on Unix, which covers every file in it; Windows keeps `%LOCALAPPDATA%`'s per-user DACL.
- 2360 a versioned trust-store format with a schema field.
- 2361 a trust-store backup and restore path.
- 2362 refuse to start, loudly, when the trust store is unreadable instead of treating it as empty.
- 2363 a rekey flow for the server identity that does not invalidate every pin silently.
- 2364 certificate expiry, with a real check in `TofuVerifier` (this is what makes 216/217 possible).
- 2365 rotation before expiry, and a graceful path for a client that missed it.
- 2366 display the fingerprint on the desktop side too, so both users can compare.

**Pairing & trust**
- 2367 throttle PIN attempts per peer and lock out on repeated failure.
- 2368 an optional longer PIN for untrusted networks.
- 2369 scoped pairing: view-only, control, files, exec-off as named presets.
- 2370 make `pair --grant` carry a scope, not just a duration.
- 2371 persist which scopes a device was granted, per device id.
- 2372 a pairing audit log with timestamps and peer addresses, on disk and queryable.
- 2373 notify both ends on pair and unpair.
- 2374 alert on a pair attempt from a device id that was previously revoked.
- 2375 one-time pairing that forgets on session end.
- 2376 idle trust expiry for devices that have not connected in N days.
- 2377 a kill switch on the desktop that ends every session and drops trust for one device.
- 2378 a local hotkey that ends the current session immediately (needs a compositor keybind, not the broken
  socket1 write path).
- 2379 emergency stop reachable from the tray or a desktop notification action.

**Consent & visibility**
- 2380 a persistent desktop indicator while any client is capturing, independent of the privacy grab.
- 2381 a banner naming the connected device and what it can do.
- 2382 show viewer/controller count once multi-client arbitration exists.
- 2383 explicit consent on first input injection per device.
- 2384 consent on first file receive into the home directory.
- 2385 consent cache with expiry instead of permanent silent grants.
- 2386 a screen-privacy timeout that blacks out the local display after N idle minutes.
- 2387 make the privacy shield render on GNOME/KDE/X11 or say plainly that it cannot.
- 2388 surface the "recording" state to the compositor's own privacy UI where one exists.

**Data-handling policy**
- 2389 clipboard size cap and an explicit MIME allowlist (even if only text ships today).
- 2390 secret-pattern detection that stops syncing a password manager's content.
- 2391 an app allowlist/denylist for notification relay.
- 2392 content redaction rules for notifications (title only, no body).
- 2393 a "privacy mode" that suspends clipboard, notifications and files while leaving input and video.
- 2394 path confinement done properly: canonicalize, then jail, then symlink policy.
- 2395 a hidden-file policy and an explicit root-directory policy.
- 2396 an executable-bit warning on receive.
- 2397 checksum-on-transfer verification for both directions.
- 2398 a download directory jail option for less-trusted devices.
- 2399 overwrite confirmation on the desktop side.
- 2400 a documented retention policy for every file the server writes.

**Auditability & diagnostics safety**
- 2401 a security event log with a defined event taxonomy.
- 2402 tamper-evidence for it — hash chain or append-only journal, chosen deliberately.
- 2403 a redaction engine shared by logs, telemetry and the support bundle.
- 2404 redact PINs, tokens, clipboard text and notification bodies at the source, not in the exporter.
- 2405 a diagnostics bundle command that produces one archive, redacted by default.
- 2406 explicit consent before any of it leaves the machine, with a visible file list.
- 2407 crash-dump handling policy (do we generate them at all?).
- 2408 secure erase of downloads and cached secrets on request.
- 2409 a "log level: paranoid" that keeps everything, documented as unsafe to share.
- 2410 a security policy page: what is in scope, how to report, and what "paired" implies.

**Threat model, the item everything above should have been derived from**
- 2411 write the threat model before more capability surfaces are added (010 was never done).
- 2412 enumerate the trust boundary precisely: tailnet peer, LAN peer, paired device, granted device,
  revoked device, stolen phone, compromised desktop user.
- 2413 enumerate what each of those can do today, which is how "a grant PIN buys an unconfirmed shell" got
  missed for as long as it did.
- 2414 decide and document the position on a malicious desktop toward the phone (today: one paired desktop
  can push arbitrary notifications and files).
- 2415 a per-feature threat table paired with the roadmap ids in this section.
- 2416 a regression gate that fails when a new plugin registers without a threat-model entry (2417-2430
  reserve the ids for that checklist per plugin).
- 2417-2430 one audit entry per shipped plugin (16 of them), stating: who may call it, what it can touch,
  what it logs, and what the user sees while it runs.

### Y. Transport & protocol completion 2431-2530

The two control planes (QUIC v2 and the KDE-Connect-style TCP v1) are the project's largest structural
liability: every feature must be built twice, and the phone now keeps a TCP registration alive *next to*
the QUIC session it streams over. The goal of this block is one transport carrying every message class with
per-class semantics, then the v1 plane shrinks to compatibility-only.

**The message-class model — the design decision that gates the rest**
- 2431 declare the traffic classes as code: CONTROL, INPUT, AUDIO, VIDEO, FILES, TELEMETRY, EVENTS.
- 2432 give each class an explicit reliability policy instead of inheriting the connection's.
- 2433 CONTROL: reliable, ordered, low-latency, small, never dropped.
- 2434 INPUT: mostly reliable, latency-prioritized, with a coalescing rule for motion and never for
  button transitions.
- 2435 AUDIO: deadline-sensitive — a late frame is worse than no frame.
- 2436 VIDEO: freshness-first — newest frame wins, oldest trimmed (the shipped newest-frame queue is the
  seed of this, 141).
- 2437 FILES: reliable and resumable, bandwidth-fair, allowed to be slow.
- 2438 TELEMETRY: lossy and aggregated, never worth a retransmit.
- 2439 EVENTS: reliable but cancellable, and a stale event should be droppable by the sender.
- 2440 encode the class on the wire (stream type or header byte) so the scheduler never guesses.
- 2441 document the class table in the protocol spec and freeze it as a versioned artifact.
- 2442 a conformance test that every message emitted by any plugin has a declared class.

**Streams, datagrams, scheduling**
- 2443 map CONTROL to one long-lived bidirectional stream with framing (already 106) and nothing else.
- 2444 map INPUT to a dedicated stream or datagram, chosen by measurement, not habit.
- 2445 map VIDEO to per-frame unistreams (today) with the option of datagrams for sub-MTU frames.
- 2446 build the datagram plane for small, deadline-sensitive classes (`use_datagrams` is dead config until
  this exists).
- 2447 a datagram freshness policy: drop anything older than N ms rather than queue it.
- 2448 per-class send budgets so files cannot starve input.
- 2449 a fairness scheduler across classes with a documented algorithm.
- 2450 a starvation detector: log and expose when a class has not sent within its budget.
- 2451 tests that prove a bulk file transfer cannot add input latency beyond a declared bound.
- 2452 the same for a video bitrate spike against the control plane.
- 2453 priority inheritance for the keyframe request path, which is latency-critical by nature.
- 2454 bounded buffers at every hop, with a named overflow behavior per class.
- 2455 backpressure signalled to the producer rather than silently dropped, per class.

**Request/response correctness**
- 2456 request ids on every CONTROL message that expects a reply.
- 2457 response ids referencing the request, so a client can match without ordering assumptions.
- 2458 correlation ids propagated into logs and telemetry for one-hop diagnosis.
- 2459 deadlines carried with the request, enforced on the server, not only on the client.
- 2460 cancellation messages so an abandoned query stops server work (the in-process `CancellationToken`
  has no wire equivalent today).
- 2461 cancellation propagation through a plugin's own spawned work.
- 2462 a stable error-code enum on the wire, mapped from `LinuxLinkError`.
- 2463 structured remote error details (code, message, retryable) instead of a `Display` string.
- 2464 the phone renders those codes through `UiError` rather than substring matching.
- 2465 a retry policy per error class, enforced in one place.

**Connection lifecycle**
- 2466 graceful GOAWAY: stop admitting new requests, drain, then close.
- 2467 a session drain that ends a stream cleanly instead of resetting it.
- 2468 close reasons carried as a code the phone can render ("server restarted", "kicked", "pairing
  expired").
- 2469 protocol-violation closes distinguished from operational closes in telemetry.
- 2470 connection reuse across queries and streaming — the v1 side is fixed (1741); the v2 side must never
  need a second dial.
- 2471 a reconnect state machine in the client with observable states, replacing scattered retry logic
  (this is what 1802's dead backoff was supposed to serve).
- 2472 resume a session after a transport reconnect without a full re-pair and re-handshake.
- 2473 QUIC session resumption / 0-RTT where the identity permits it.
- 2474 server restart recovery: a client that reconnects must land on the same session or get told why not.
- 2475 client restart recovery: relaunch restores the last host and re-attaches without re-pairing.
- 2476 roaming: survive an address change on the same connection (QUIC makes this legal; nothing exercises
  it) — pairs with 2299-2302.
- 2477 path-change telemetry so roaming is observable rather than assumed.
- 2478 idle-timeout policy made configurable, with keepalive suppression when the phone screen is off.
- 2479 battery-aware keepalive on the phone (a session that stays warm all night is a battery bug).
- 2480 metered-network policy: a declared bytes-per-hour budget and a behaviour when it trips.
- 2481 duplicate-session suppression across planes (v1 and v2 both registering the same device is the same
  class of bug 1741 fixed inside v1).
- 2482 a max-client / concurrent-session policy — today any number of paired devices can attach.
- 2483 per-device rate limits on control messages, so a buggy client cannot hammer the plugin dispatch.
- 2484 a quota per device per day for expensive operations (exec, transfers).
- 2485 controller arbitration: one input owner, an explicit token, and a visible claim (the prerequisite for the
  multi-viewer ids in M, and fixes the "any phone can move the cursor" reality found in the M audit).
- 2486 a takeover policy with consent, not last-writer-wins.
- 2487 observer/spectator mode built on 2485 (view-only is the seed).

**Schema, capability and compatibility**
- 2488 a versioned control codec: replace JSON-on-TCP for the hot classes with a compact binary encoding
  behind a capability bit.
- 2489 a message-type registry generated from code so docs and wire cannot drift.
- 2490 a capability negotiation that *gates*: intersect, then refuse un-negotiated messages (fixes 103).
- 2491 datagram role registry once 2446 exists — one byte per role, documented.
- 2492 a compatibility harness that runs a matrix of client/server version pairs (the honest 200).
- 2493 an anti-ossification randomization or reserved-bits rule, only if the binary codec lands.
- 2494 fuzz the binary codec and the demuxer (2317-2322 target list).
- 2495 deterministic transport tests with a seeded scheduler and a virtual clock.

**Migration & removal — the boring part that makes this real**
- 2496 a written migration plan for v1 → v2 with an explicit "both planes live" window.
- 2497 which v1 plugins move first, ordered by risk (queries before streaming, streaming before files).
- 2498 keep the TCP plane for pairing/legacy discovery only, and say so in the spec.
- 2499 an opt-out for the QUIC plane for a debugging window, removed before release.
- 2500 a telemetry field that reports which plane each session used, so removal is a measurement.
- 2501 delete criteria: v1 goes when zero sessions report it for a release cycle.
- 2502 the file transfer plane decision: TCP back-connection vs QUIC stream (2851-2863 depends on it).
- 2503 the audio plane decision: today it rides QUIC while control rides TCP.
- 2504 the notification/event plane decision: push semantics with the class from 2439.
- 2505 a single dial path: the bridge should not own both a TCP keepalive and a QUIC session for one device.
- 2506 a session object that owns transport, identity, capabilities and state, replacing module-level statics.
- 2507 the statics this implies (`CONTROL_WRITER`, `INCOMING_PACKETS`, `SESSION_RELAYED`, …) are fine for one
  session per process and must be called out as such before anyone needs two.
- 2508 a per-session log span so a support request can be filtered to one session.
- 2509 define and test the "no reply" case at the transport level — currently every caller invents its own
  timeout string.
- 2510 define the "wrong reply" case (type mismatch, late response to a cancelled request).
- 2511 bound the inbound queue and drop policy for a lagging subscriber, which the broadcast in 1741 needs
  before it can be trusted under load.
- 2512 a handshake that reports which capture backends, encoders and codecs the server actually has, so the
  phone can pre-flight instead of discovering on first use.
- 2513 the same for the phone's decoder capabilities, sent to the server so encode choices match the real
  receiver.
- 2514 renegotiate mid-session on a codec or resolution change without tearing the connection down.
- 2515 a compatibility note per release tying the wire version to the app version (see 2346).
- 2516 a protocol version registry file that both planes and the docs read (this is 004, finally).
- 2517 ALPN and `llVersion` cross-checked, with one authoritative source for "are we compatible".
- 2518 a hard rule, tested: never break the wire without a version bump and a compatibility statement.
- 2519 document every current packet type and body shape — the closest thing to it today describes a
  Flutter-era design.
- 2520 a wire-changer checklist (test, doc, version, compat statement, rollback) enforced in review.
- 2521 connection-ID rotation behavior observed and reported, not assumed.
- 2522 path MTU discovery outcome recorded per session (PMTUD is quinn's, the *observation* is ours).
- 2523 IPv4/IPv6 preference policy for LAN dialing (today `ips.first()` wins).
- 2524 Happy Eyeballs-style racing for multi-address peers, if 2523 shows it matters.
- 2525 NAT rebinding handling asserted by a test that changes the source address mid-session.
- 2526 relay selection and health scoring, so a bad relay is measurable rather than tolerated.
- 2527 relay→direct promotion when a punched path appears after the session started.
- 2528 direct→relay fallback when a direct path dies, without user action.
- 2529 relay consent and disclosure: the desktop says whether it allowed relaying, and the phone shows it.
- 2530 a transport-level "quality score" per class, feeding 2211's diagnosis rather than a HUD guess.

### Z. Capture, compositor & display control 2531-2650

**Compositor coverage that is real**
- 2531 a per-compositor capability table generated at runtime, not prose in a matrix document.
- 2532 Sway/wlroots: verify the `zwlr_screencopy` path (generic code exists; it has never been run there).
- 2533 KWin: Wayland `org.kde.kwin.screenshots` route or PipeWire portal, plus a documented test.
- 2534 GNOME/Mutter: portal-only path with restore-data caching.
- 2535 niri: screencopy verification.
- 2536 a wlroots-wide pass (River/Labwc/Wayfire) behind one CI-adjacent script, since they share the
  protocol even when they differ in policy.
- 2537 COSMIC: record it as unsupported until it stabilizes its own session backend.
- 2538 a "compositor unsupported" error that says what was tried and what the fallback is.
- 2539 keep the "write dispatchers are broken upstream" rule as a tested assertion, not folklore: no code
  path may call `dispatch`/`keyword`/`setoption` on socket1.
- 2540 portal restore-data caching so a session does not re-prompt every launch.
- 2541 portal failure diagnostics that distinguish refused, cancelled, timed out, and unavailable.
- 2542 an explicit statement of what X11 support means today (root + region grab, no window capture).
- 2543 XDamage-driven capture instead of `memcmp` polling on X11.
- 2544 XComposite/XShm path, or delete the claim and the dead `capture_x11.rs` (2083).
- 2545 X11 window capture with a window→geometry mapping usable by the picker.
- 2546 cursor shape relay so the phone shows the desktop's actual cursor.
- 2547 cursor-only updates for a still desktop with a moving pointer.
- 2548 decouple the cursor from the frame (composited on the phone) — the biggest static-screen bandwidth
  win available.

**Buffer & colour path**
- 2549 dmabuf constraint discovery and a real dmabuf ingest path (screencopy currently fails over on
  purpose).
- 2550 modifier negotiation with a safe fallback to shm.
- 2551 `ext-image-copy-capture-v1` as a capability-detected third Wayland backend — evaluate, don't
  inherit: Hyprland support is an open upstream issue (#9916), so it must be selected only when present and
  never trusted as the foundation.
- 2552 output-transform handling (rotated and transformed panels, currently only a Y-invert special case).
- 2553 fractional-scaling and HiDPI coordinate math end to end (capture, mapping, touch).
- 2554 colour-management: at minimum tag frames as limited/full range and BT.709 and stop the ambiguity.
- 2555 HDR metadata path: parked; see the do-not-build list.
- 2556 damage-rectangle propagation so the encoder can skip clean regions.
- 2557 present-timestamp capture where the backend can provide it.
- 2558 a capture watchdog that restarts the backend rather than ending the session.
- 2559 frame-deadline miss detection with a per-session counter.
- 2560 static-screen FPS floor extended into an adaptive ceiling driven by change rate.
- 2561 motion-sensitivity that measures a change level instead of a binary static/active switch.
- 2562 duplicate suppression by hash rather than full compare, once frames are large enough to care.
- 2563 black-frame and protected-window detection so a DRM-protected app is reported, not streamed as
  black.
- 2564 per-region priority (the window under the cursor) behind an explicit experiment, not a promise.
- 2565 scroll-aware ROI, only if 2564 shows the mechanism works.
- 2566 output hotplug: add, remove, rename, mode change, scale change, orientation change — add and remove
  now drive a compositor re-snapshot (2066), but a rename, mode, scale or orientation change still surfaces
  only on the next 4 s tick, and this Hyprland emits no event for it at all (`strings` on the binary shows
  exactly `monitoradded` and `monitorremoved`), so that half needs a poll or a different signal, not a
  broader match arm. None of it has been seen with a real cable.
- 2567 capture source restart on hotplug without dropping the session.
- 2568 a capture test-pattern source (replacing zeros from the cfg(test) helper) usable on device.
- 2569 golden-frame tests for the conversion path.
- 2570 a capture latency benchmark that needs no hardware.

**Monitor & window model**
- 2571 expose monitor position, not just size, so multi-monitor mapping is correct.
- 2572 real primary detection instead of `index == 0`.
- 2573 modes and refresh rate in the monitor list.
- 2574 orientation and scale in the monitor list.
- 2575 monitor change events pushed to the phone (paired with 2066).
- 2576 per-monitor streaming re-selection mid-session without a new pipeline.
- 2577 window `pid` propagated to the client (2067).
- 2578 `app_id` alongside `class`, since the desktops disagree about which they provide.
- 2579 window state (minimized/fullscreen/floating/pinned) in the model.
- 2580 a window-filter policy for capture: exclude a named window, not just crop to one.
- 2581 window title change events, which the current model only samples.
- 2582 per-window crop geometry validated against the encoder's dimensions (the crop pipeline is the most
  device-unverified thing in the app).
- 2583 window capture on X11 or an explicit "Hyprland only" capability bit.

**Desktop control actions (re-scoped from the fabricated 910-921)**
- 2584 decide the mechanism per compositor: uinput keybindings (works today, layout-dependent), layer-shell
  helper, or a documented socket1 write when upstream fixes it.
- 2585 close window.
- 2586 focus window.
- 2587 move/resize window by gesture.
- 2588 maximize/restore/unfullscreen toggle.
- 2589 move window to workspace N.
- 2590 switch to named workspace (numeric-only today).
- 2591 create/rename/remove workspace, if the compositor allows it.
- 2592 window snap/move-to-monitor.
- 2593 a control-action abstraction so an action is one call regardless of mechanism.
- 2594 an idempotent result reported back: did the desktop actually do it, or did we type a key into nothing.
- 2595 per-action permission, tied to 2369's scopes.
- 2596 audit every control action (see 2401).
- 2597 an undo affordance where the compositor can supply one.
- 2598 make the phone's window/workspace UI reflect reality within one event, not on next poll.
- 2599 tests for the action layer's command construction with a fake compositor.
- 2600 a documented matrix of which actions work on which compositor — measured, not asserted.

**Virtual display / phone-as-second-screen (its own epic; the old K range's honest residue)**
- 2601 a display abstraction that separates "physical output", "captured output" and "virtual output".
- 2602 a Hyprland headless-output prototype via `hyprctl output create headless` and socket1 *read*
  verification of the result.
- 2603 handle the known upstream limitations: resolution/refresh control (#5415) and black-output reports
  (#12690) — with a probe that proves the output is live before it is offered.
- 2604 a wlr `output_manager_v1` headless path for generic wlroots compositors.
- 2605 an X11 path (Xvfb or the dummy driver) or an explicit statement that X11 is unsupported here.
- 2606 a portal-virtual-output path for desktops that expose one.
- 2607 lifecycle: create, size, position, destroy, and idempotent re-create after a compositor restart.
- 2608 persistence: a virtual output that survives a service restart without orphaning.
- 2609 arrange it relative to physical outputs (position, primary, above/below).
- 2610 scale and orientation for the virtual output independent of the panel.
- 2611 a resolution profile matched to the phone's viewport and a sane bitrate ceiling.
- 2612 a refresh-rate profile for the virtual output, capped by the session FPS.
- 2613 mirror mode (clone an existing output) as a separate, simpler feature.
- 2614 extend mode with the phone as the second screen — the actual goal, gated on 2601-2613.
- 2615 input targeting across outputs so a touch on the phone lands on the virtual output only.
- 2616 cursor confinement and crossing policy.
- 2617 focus policy: does the virtual output take focus, and how does that feel.
- 2618 a black-screen mode: the phone sees the desktop, the desk does not.
- 2619 privacy handling for a virtual output (the shield applies to capture, not to this).
- 2620 power policy: a virtual output must not keep a laptop panel awake or defeat suspend.
- 2621 hotplug recovery: the session survives the virtual output being destroyed underneath it.
- 2622 a "compositor refused" fallback that says so and reverts cleanly (no half-created outputs).
- 2623 measure the latency and quality difference of a virtual output vs a crop of the physical one.
- 2624 a resource budget: a virtual output must not exceed a declared CPU/GPU share.
- 2625 document per-compositor support honestly in the matrix (090).
- 2626 an opt-in flag; nothing about this feature may be automatic.
- 2627-2650 reserved for the follow-on work that only becomes definable once 2601-2626 exist: EDID/profile
  import-export, per-device display profiles, multi-phone arrangements, layout presets, drag-across-outputs,
  and their tests, telemetry and docs.

### AA. Input fidelity & HID 2651-2740

**Correctness before capability — the input path has no delivery semantics at all today**
- 2651 sequence numbers on every input event per connection.
- 2652 an ACK or explicit fire-and-forget decision per class (see 2434), so "did my click land" is
  answerable.
- 2653 a stuck-key release-all on disconnect, session end and view-only engage.
- 2654 release-all on transport error, which is where stuck modifiers will actually happen.
- 2655 a watchdog that releases any key held longer than a declared window.
- 2656 coalescing rules for motion that never coalesce a press with a move.
- 2657 input latency measured per event (see 2145) with a p95, not an impression.
- 2658 an input queue depth metric on the server side.
- 2659 a drop counter that reaches the HUD (and is not the fabricated field from 2051).
- 2660 assert press/release balance in every test that injects (2272).
- 2661 a repeat policy (auto-repeat for held keys) and a way to stop it.
- 2662 make key state queryable so the phone can show what it thinks is held.
- 2663 **PARTIAL (`a9b903d`)**: the *set* of codes is now shared — `core::input::keys` is the one source
  both ends' tests are written against — but the two *tables* are still hand-written (Android key code →
  evdev in the bridge, evdev → enigo `Key` in the server). Generating either from the other, or from a
  single data file, is what remains; 2271 is the guard that makes a drift fail loudly meanwhile.
- 2664 **CLOSED (`db75497`, `dc9fc22`)** — see 2271; both directions are enumerated in tests.
- 2665 layout-aware injection (a non-QWERTY desktop must receive the keysym the user pressed). The `us`
  assumption is now *detected and reported* at injector construction (2065), which is what turns this from a
  hidden behaviour into a measurable one; its repro is recorded there. What remains is the injection itself —
  resolving the phone's key against the desktop's actual keymap instead of emitting a physical position — and
  telling the phone which layout was found, so the app can say so before a user types into the wrong keyboard.
- 2666 unicode text injection on the uinput path, not only through enigo. **Mechanism found while working
  2057-2060 (`dc9fc22`)**: `InputInjector::text()` on uinput goes through `char_to_keycode`, which yields a
  bare key press with no Shift — so pasted capitals arrive lowercase, and `*`/`+` land on the keypad codes
  (`KEY_KPASTERISK`, `KEY_KPPLUS`) rather than the shifted digits. The enigo rung's new table cannot fix
  this; it needs a shift-aware text path.

**Pointer & touch**
- 2667 right-click and middle-click reachable from a phone gesture.
- 2668 a long-press that produces a desktop right-click or a drag, user-selectable.
- 2669 high-resolution scroll (the wire has integer clicks only).
- 2670 a real drag in direct-touch: press, move, release (fixes 2061).
- 2671 pointer capture mode for games and 3D apps.
- 2672 relative-mode mouse with a sensitivity and acceleration control.
- 2673 a trackpad mode with tap-to-click, two-finger scroll, and edge scroll.
- 2674 inertial scroll forwarded as a wheel burst rather than discrete clicks.
- 2675 pinch-to-zoom and pinch-to-layout mapped to desktop modifiers (ctrl+wheel).
- 2676 three- and four-finger gestures mapped to workspace switching.
- 2677 a palm-rejection heuristic for large phones in direct-touch.
- 2678 touch calibration for letterbox and aspect mismatch beyond the current arithmetic.
- 2679 per-monitor offset math for multi-monitor streaming.
- 2680 stylus/pen support with pressure where the device reports it.
- 2681 mouse-button forwarding for a Bluetooth mouse paired to the phone.
- 2682 external keyboard mapping (full HID usage where Android supplies one) including function layer.
- 2683 key-repeat and modifier-latch behaviour for external keyboards.
- 2684 an on-screen keyboard mode that injects through the text path rather than keystrokes.

**Keyboard & text**
- 2685 an IME surface in the session so non-Latin input is possible at all.
- 2686 a paste-from-phone action that injects rather than only syncing clipboard.
- 2687 a type-text fast path that avoids per-character keystrokes for long strings.
- 2688 dead-key and compose handling on the text path.
- 2689 a keycode-vs-text decision rule, documented, so behaviour is predictable.
- 2690 shortcut profiles per app or per layout, with a phone-side editor.
- 2691 a macro step, only once 2351-2356 and the input correctness items above hold — the smallest honest
  slice of the old L range.
- 2692 a gaming profile (mouse+keyboard mapping) as an explicit, opt-in surface.
- 2693 media-key delivery verified against PipeWire/portal-free desktop shortcuts.
- 2694 a system-requests layer (logout, lock, inhibit-sleep) with consent.

**Gamepad (wire + toy emulation, now with paired press/release; no Android source)**
- 2695 a real gamepad source on Android (`InputDevice` handling with axes and buttons).
- 2696 **CLOSED** with 2063: the server handler is edge-triggered on the button mask, so every press has
  its release. Note this was never *reachable* — with no Android source the packets only exist in tests.
- 2697 axis delivery that is not quantized to a stick-to-key emulation.
- 2698 triggers as analog axes, not buttons.
- 2699 a hat/DPad that releases.
- 2700 force feedback, if and only if the desktop can drive it without a new dependency.
- 2701 a calibration and dead-zone UI.
- 2702 a layout mapping screen (which phone button means which virtual button).
- 2703 a per-app gamepad profile.
- 2704 sensitivity and response-curve controls.
- 2705 tests for the wire format, including zero-length and malformed payloads.

**Injection infrastructure**
- 2706 an injector health probe at session start, with a clear message on failure (uinput permissions are
  the classic case and today it is an advisory string).
- 2707 a documented, checked udev/group requirement in `install.sh` and the AUR package (see 1575).
- 2708 recreate the injector when the device disappears instead of running session-wide on a dead handle.
- 2709 a Wayland-native path where available (`zwp_virtual_keyboard` / text-input) as a fallback to uinput.
- 2710 an X11 path that does not depend on the Wayland assumptions (XTEST works; enigo's route should be
  asserted by test).
- 2711 per-compositor input notes in the compatibility matrix.
- 2712 a11y/portal diagnostics when a desktop refuses synthetic input.
- 2713 an explicit "input is blocked by view-only" state that reaches the phone.
- 2714 an explicit "input is blocked because the screen is locked" state.
- 2715 lock-screen behaviour defined and tested (currently undefined).
- 2716 secure-entry detection: never pretend input works where it cannot.
- 2717 multi-touch to single-pointer translation policy.
- 2718 cursor warp vs relative move policy per mode.
- 2719 input coalescing bounds under a lossy path, expressed in ms.
- 2720 a per-event budget so an input storm cannot monopolise the control plane (pairs with 2451).
- 2721-2740 reserved for the per-device input matrix: the phone models and tablets actually supported, their
  touch sampling rates, their external-keyboard and gamepad quirks, with one test or one documented
  limitation per entry rather than a wish list.

### AB. Media pipeline: encode, decode, audio 2741-2850

**Encoder**
- 2741 a codec registry that lists negotiated codecs per session and per device (replaces the enum-only
  401).
- 2742 make the first frame an enforced IDR with cached SPS/PPS, not an encoder default.
- 2743 a server-side GOP/keyframe cache so a late joiner or a reconnecting decoder starts instantly.
- 2744 periodic decoder-config resend independent of keyframes.
- 2745 scene-change-triggered IDR, evaluated against a bitrate cost.
- 2746 framerate reconfiguration mid-session (today fps is fixed at pipeline start).
- 2747 resolution reconfiguration as a policy, not only as an encoder rebuild side effect.
- 2748 profile-level reconfiguration without a codec change.
- 2749 CRF/capped-VBR mode selection exposed as a preset rather than a compile-time choice.
- 2750 a real CBR mode per rung where the encoder supports it.
- 2751 GOP length as config.
- 2752 lookahead/reference tuning knobs behind measured defaults only.
- 2753 content presets (desktop, text, video, battery) that set resolution/fps/bitrate/GOP coherently —
  the honest replacement for the fabricated 437-440, and gated on 2746/2747 so a preset can actually move
  resolution and framerate.
- 2754 an encoder watchdog per rung that reports time-to-first-frame on every rebuild.
- 2755 encode-time and frame-size histograms feeding 2142/2146.
- 2756 an encoder quality benchmark mode (PSNR/SSIM against a pinned clip) that CI can run on VAAPI where
  present and skip-with-reason where not.
- 2757 thermal-aware encoder policy: reduce, do not stall.
- 2758 battery-aware policy on the desktop when on a laptop.
- 2759 a broken-driver blacklist keyed on driver fingerprint, learned from failures rather than prose.
- 2760 runtime FFmpeg/driver compatibility reporting (this box's NVENC/API-13 mismatch is the exact case).
- 2761 a codec-fallback telemetry event, so "we have been on software for weeks" is visible.
- 2762 user codec override, gated behind the capability negotiation from 2490.
- 2763 negotiation diagnostics: why the session chose what it chose, on screen.
- 2764 an in-process NVENC path when a driver new enough to support it exists (hardware-gated; parked).
- 2765 dma-buf zero-copy from capture to encoder: the only remaining big latency/bandwidth win on the
  server, and the reason 2549 exists.
- 2766 GPU-side colour conversion instead of `sws_scale` per frame.
- 2767 GPU crop/scale path for the window-crop feature (currently software `crop_region`).
- 2768 a pinned-host-buffer experiment to bound the pipe copy cost (measure with 2175).
- 2769 a long-run encoder soak with leak checks (2312).
- 2770 an encoder crash-recovery test that proves the ladder and the supervisor agree.

**Decoder & render**
- 2771 decoder capability probe cached per device model and OS build, not only per process.
- 2772 a secure-output capability query before `setSecure` (2097).
- 2773 low-latency flags verified: confirm the vendor keys the target SoC actually honours, measured by
  decode-to-render delta rather than by hope.
- 2774 a decode-queue metric and a render-queue metric.
- 2775 late-frame and dropped-frame metrics that reach the HUD for real.
- 2776 automatic decoder restart on stall (today a stall tears down and waits for a human).
- 2777 codec switch applied on a keyframe boundary with no visible corruption.
- 2778 resolution and orientation switch without a black flash.
- 2779 display refresh-rate mode selection and 60/90/120 Hz profiles where the panel offers them.
- 2780 frame pacing against the phone's own vsync (the `setFrameRate` hint is not a pacing loop).
- 2781 VRR behaviour, or an explicit statement that it is unsupported.
- 2782 letterbox, crop, stretch and pixel-perfect modes as user choice.
- 2783 safe-area, notch and fold-crease insets applied to the video pane.
- 2784 DeX/tablet/desktop-mode layout policy.
- 2785 a lock-screen and notification policy for a running session.
- 2786 screenshot prevention asserted, not inherited from `FLAG_SECURE` as a side effect.
- 2787 background-session behaviour defined (what plays, what pauses, what stays warm).
- 2788 a brightness policy during blackout that cannot leave a user with a dark-but-unlocked panel.
- 2789 thermal and battery callbacks feeding the quality preset.
- 2790 a decode-time percentile view in the diagnostics screen (2219).

**Audio — nothing here matters until the phone can play sound**
- 2791 build the client-side Opus player (decode + `AudioTrack` in low-latency mode) — the precondition for
  the entire old G range.
- 2792 a jitter buffer sized by the measured network, not a constant.
- 2793 a playout clock and drift handling.
- 2794 audio/video synchronisation using 2131-2136's timeline.
- 2795 a discontinuity and concealment policy for lost audio packets.
- 2796 default-output switching on the phone (speaker ↔ headphone ↔ Bluetooth).
- 2797 per-app source capture on the desktop (`pw-profiler`-style node selection).
- 2798 a source selector in the phone UI, backed by 2797.
- 2799 sample-rate and channel-count modes with a negotiation, and a 32 kHz fallback for weak links.
- 2800 Opus bitrate and complexity profiles that switch with the link preset.
- 2801 optional FEC and a packet-loss target that matches the measured loss (see 2053).
- 2802 mute/volume propagation semantics defined for both directions.
- 2803 audio-route health reporting (device gone, sink suspended, node stolen).
- 2804 hotplug subscription for PipeWire node changes instead of a one-shot default-sink query.
- 2805 a per-session PipeWire node name so multiple sessions do not collide.
- 2806 echo cancellation, noise suppression and gain control on the mic path — meaningless until 2791's
  playout path exists, since there is no far-end signal to cancel.
- 2807 push-to-talk and a mic mute shortcut with a visible state.
- 2808 a mic-level indicator on the desktop side, so "the phone is transmitting" is obvious.
- 2809 media playback controls forwarded (play/pause/next/prev/seek) using the media keys 2693 delivers.
- 2810 track metadata relay.
- 2811 album-art relay, with the same size limits and refusal copy as any other image payload.
- 2812 audio focus and ducking policy on Android.
- 2813 an audio-only session mode (no video, low bitrate) — genuinely useful and cheap once 2791 exists.
- 2814 audio pause/resume tied to the session lifecycle and to PiP.
- 2815 an audio latency measurement and budget (see 2201-2210).
- 2816 Opus encode/decode benchmarks (see 2193).
- 2817 an audio test tone and loopback self-test reachable from diagnostics.
- 2818 voice-activity detection, only if it feeds a visible behaviour.
- 2819 a clipping detector on the mic path.
- 2820 audio CPU/memory/battery budgets for the phone.

**Adaptivity — the behaviour users actually judge**
- 2821 a loss-based ABR decision (the `update_loss` hook from 2053 is the seam).
- 2822 a bandwidth estimate that distinguishes "congested" from "encoder cannot keep up".
- 2823 an explicit ABR state machine: probe, hold, reduce, recover, with logs that name it.
- 2824 hysteresis symmetry fixed (1756) with measured thresholds.
- 2825 a resolution-reduction rung before a frame-rate reduction rung, or the reverse — decided by
  measurement, then implemented.
- 2826 a stall-then-recovery path that requests an IDR and reduces bitrate together, instead of only
  requesting an IDR.
- 2827 a bandwidth reserve for audio and input, so video cannot crowd them out (needs 2448).
- 2828 a relayed-path floor with a documented, measured value instead of a constant.
- 2829 per-preset behaviour that changes fps and resolution, not just bitrate — which is what the existing
  presets promise and do not deliver.
- 2830 a quality-degradation notice, so the user knows the link changed rather than blaming the app.
- 2831 a "why did quality drop" one-liner from 2211 in the HUD.
- 2832 an adaptive-controls screen (manual override of any automatic decision, with a reset).
- 2833 an ABR test suite over the chaos presets (2304-2306) asserting bounded time-to-recover.
- 2834 a bitrate/variance metric used as a stability signal in the diagnostics screen.
- 2835 a "first-frame time" metric per session — the number that decides whether an app feels fast.
- 2836 a reconnect-to-picture metric for the same reason.
- 2837 both reported in the session record (2167).
- 2838-2850 reserved for codec-specific work that must wait for its hardware: HEVC negotiation completion,
  AV1 evaluation once a target device with AV1 *decode* exists (see 2995), 10-bit and P010 paths, HDR
  metadata, SVC/scalable-encoding experiments, simulcast research, and the interop matrix per codec. Each
  entry needs a device before it needs a ticket.

### AC. Files, clipboard & notifications 2851-2930

**The transfer protocol first.** Ids 801-816 in the old range assumed a chunked, resumable transfer layer
existed and asked for features on top of it. It does not: a transfer is one `share.request`, a
back-connection, and a 64 KB copy loop that dies on any interruption. Everything below that is not a
protocol edit is blocked on 2851-2862.

- 2851 specify the transfer as a **message class**, not a connection: reliable, resumable, deadline-free,
  and explicitly allowed to take seconds (needs 2531's class table to exist as code).
- 2852 a transfer manifest: id, path, size, mtime, mode, direction, checksum algorithm, chunk size.
- 2853 chunk framing with an id so a chunk is addressable independently of the stream.
- 2854 an offset-based resume: the client asks for `[from, to)`, the server answers with bytes, no restart.
- 2855 a per-chunk integrity check and a whole-file check on completion, with the mismatch surfaced rather
  than silently retried.
- 2856 an explicit transfer state machine (`requested → consented → transferring → verifying → done|failed|
  cancelled`) with every transition logged under 2141.
- 2857 cancellation that actually stops the reader (today `send_file` runs to EOF or socket death).
- 2858 backpressure: a slow phone must not grow the desktop's send buffer unboundedly.
- 2859 a transfer quota and concurrency cap, so a folder drag cannot starve input and audio (2827).
- 2860 symlink and special-file policy applied to the *transfer* path, not only to browse (817).
- 2861 filename policy: encoding, invalid characters, case collisions, longest-path truncation.
- 2862 directory and multi-file transfers as a tree walk over 2852, with per-entry failure reported instead
  of swallowed (851).
- 2863 tests for the protocol: resume across a kill, corrupt-chunk detection, cancel mid-transfer, path
  traversal rejection.

**Phone-side transfer UX**
- 2864 a transfers screen: in-flight list with progress, rate, and remaining time.
- 2865 per-transfer cancel, retry, and a "show in files" affordance on completion.
- 2866 a transfer history that survives app restart (894's in-memory queue is the thing to replace).
- 2867 a completion notification with a tap target, not a toast that expires.
- 2868 a failure notification carrying the reason (`permission denied` ≠ `link dropped`).
- 2869 storage permission and target-directory selection handled once, not per send.
- 2870 overwrite / merge / skip confirmation with the conflicting names listed.
- 2871 an executable-bit and large-file warning at the moment of receipt.
- 2872 the Android share target (825) moved onto 2852 instead of an inline thread send.
- 2873 a queue when the session is down: "will send on connect" rather than a silent failure.

**File browsing**
- 2874 wire `listRemoteFiles` — it has zero call sites today, so the whole browse surface is dead code on
  the client and a working plugin on the server.
- 2875 a browser screen: list, breadcrumbs, pull-to-refresh.
- 2876 thumbnails and a MIME-aware row icon, size, mtime, and type per entry.
- 2877 sort (name/size/time) and a filter.
- 2878 search within a directory, and a bounded recursive search with a cancellation.
- 2879 hidden-file toggle.
- 2880 a preview sheet for text and images without downloading the whole tree.
- 2881 long-press selection and multi-select download.
- 2882 desktop-side "open containing folder" via the existing exec path, gated by 2351's approval model.
- 2883 paste/upload into the current directory.
- 2884 delete and rename, each behind an explicit confirm.
- 2885 browse-page unit tests on the server plugin plus one Compose UI test for the list states.

**Clipboard**
- 2886 a clipboard *type* negotiation: plain text today, image and file-list tomorrow, with the receiver
  declaring what it accepts.
- 2887 an image clipboard path (phone screenshot → desktop, desktop selection → phone).
- 2888 a file-list clipboard path built on 2852, not on a base64 blob in a text field.
- 2889 size limits per type, stated in the UI when a paste is refused.
- 2890 conflict rule for the poll ordering gap (873): last-writer-wins is a decision to record, not to imply.
- 2891 a clipboard banner/toast naming which direction moved, so a silent failure is not invisible.
- 2892 per-session clipboard opt-in, independent of the global toggle (privacy grab semantics).
- 2893 history pinning and per-entry expiry.
- 2894 history search.
- 2895 a "clear both ends" action that reaches the desktop clipboard, not only local state.
- 2896 secret-manager exclusion (a password field's content should not sync; needs a desktop-side signal).
- 2897 clipboard tests for echo suppression across two devices and for the 8192-char boundary behaviour.

**Notifications**
- 2898 per-app notification channels instead of one channel (882).
- 2899 urgency mapping that the client honours instead of discarding (881).
- 2900 grouping by app and by conversation, with a group summary row.
- 2901 an app icon per notification where the desktop supplies one.
- 2902 reply into an existing thread, extending the shipped 886 path to grouped notifications.
- 2903 actions beyond reply: dismiss, open, mute-app, forwarded back to the desktop.
- 2904 snooze, with the snooze respected on the desktop side too.
- 2905 a privacy mode: hidden content on the lock screen and in heads-up, per app.
- 2906 a rate limit and a flood guard so a chatty app cannot starve the control path.
- 2907 delivery receipts: sent / shown / dismissed / failed, recorded under 2141.
- 2908 notification history in-app with the same expiry rules as clipboard.
- 2909 a "mute this desktop for the session" control.
- 2910 battery-death and doze behaviour tests: a queued notification must survive a phone sleep.
- 2911 an FGS contract statement for transfers + notifications, distinct from streaming (826).
- 2912 notification tests on the server plugin and one end-to-end delivery test.

**Cross-cutting**
- 2913 one consent model shared by files, clipboard, notifications, and camera/mic: a per-capability grant
  with a session lifetime, surfaced in one place (2351-2360).
- 2914 a data-usage report: what crossed in the last session, how much, and why.
- 2915 a pause-all-sync control for the moment you forget the phone is paired.
- 2916 wire these three surfaces onto the QUIC control plane rather than the TCP v1 side (2431-2450).
- 2917 an interop matrix: send/receive against three Android vendors' file managers and two clipboard
  managers, since ContentResolver and OEM clipboard behaviour diverge.
- 2918 a documented, versioned schema for the manifest, so a 2027 client can read a 2026 server's spool.
- 2919 a capability bit per surface (files / clipboard-types / notifications), negotiated at handshake, so an
  old phone never sends a chunked manifest to a new server or the reverse.
- 2920 a spool on the desktop for transfers that outlive the session, with an explicit expiry.
- 2921 the same spool on the phone, so a download survives an app close.
- 2922 disk-space preflight on both ends before a transfer starts, with the number in the refusal.
- 2923 a power-state rule: no bulk transfer on the desktop when on battery below a threshold, unless asked.
- 2924 a metered-link rule on the phone (tailnet traffic is not free on cellular), surfaced as one setting.
- 2925 content-hash dedupe across transfers, so re-sending a folder that barely changed costs the delta.
- 2926 optional compression for text-heavy payloads, measured so the default is honest about its cost.
- 2927 an audit line per transfer/clipboard/notification event, readable by `linux-link sessions` (2939).
- 2928 a per-session "data transferred" counter in the HUD, so the abstraction in 2914 has a number.
- 2929 a soak test: 1000 mixed events (files, clipboard, notifications) over a lossy link, asserting nothing
  is silently dropped and the control path keeps its latency budget.
- 2930 a decision record for each of 2919-2928 in `docs/`, because this range is where the roadmap's first
  genuinely new protocol lives and the next reader needs the reasoning, not just the code.

### AD. Product surface: Android & Linux 2931-2980

**Trust and device management — the screen the bridge already supports.**
`list_trusted_peers` / `forget_trusted_peer` and the computed certificate fingerprint exist in the bridge
with no caller; 1437 and the O-range ABSENT list are one screen, not five.
- 2931 a "Paired computers" detail sheet: name, address history, certificate fingerprint, last session.
- 2932 verify-the-fingerprint UX: show it at pairing *and* on the device card, and mark a change loudly.
- 2933 revoke from the phone, wired to `forget_trusted_peer`.
- 2934 revoke from the desktop, wired to `linux-link unpair`, with both ends converging.
- 2935 per-device policy: view-only default, file access, clipboard, notifications, mic — each a real grant
  record, not a settings string.
- 2936 a "who else is connected" list on the phone (server registry, D2).
- 2937 session history on the phone, read from 2939 rather than from a log.
- 2938 tests that a revoked peer cannot reconnect without a fresh PIN, on both transports.

**Records and diagnostics**
- 2939 a durable session record store: start/end, peer, path (LAN/tailnet/relay), codec, encoder rung,
  mean/p50/p95 rtt and e2e, first-frame time, reconnect count, outcome.
- 2940 surface 2939 in-app as a session list with a per-session detail view.
- 2941 a diagnostics screen in the app: link path, encoder in use, measured percentiles, and a "why did
  quality drop" line from 2211.
- 2942 a support bundle: redacted logs + 2939 + device/build info, exported as one archive.
- 2943 the same archive from the CLI (`linux-link doctor --bundle`, 2959).
- 2944 an in-app "run self-test" that drives 2963 against the connected desktop.

**Settings, onboarding, permissions**
- 2945 settings sections for privacy, power, notifications, files, and advanced, each currently absent (O).
- 2946 chrome fade duration, HUD contents, and a "never fade" mode as preferences (1495).
- 2947 a permission centre: what each runtime permission unlocks, its state, and a rationale screen on
  denial (1469-1471).
- 2948 battery-optimization guidance where OEM throttling is detected (ColorOS first, given the test device).
- 2949 first-run onboarding: install the server, read the PIN, pair — one flow, no prose wall.
- 2950 a QR pairing path where a camera is available, as an alternative to typing a PIN.
- 2951 deep links for reconnect and pairing, with the trust check that makes them safe (2932).
- 2952 host tags, notes, favourites, search and sort on Home (1416/1422 are the store to build on).
- 2953 host import/export so a second phone does not need re-pairing ceremony per machine.
- 2954 a reduced-motion path and a font-scale audit for the HUD's hardcoded 11.sp (1407).
- 2955 touch-target and TalkBack audit of the session chrome, recorded as a checklist item that can fail.
- 2956 RTL and plural forms, and a translation review pass (1487-1488 ship by count, not by vetting).

**Linux CLI surface**
- 2957 `--json` on `status`, `sessions`, `capabilities`, and `list`, so scripts stop parsing prose (1507).
- 2958 a `--config <path>` flag, which the binary does not accept today and the README used to claim it did.
- 2959 `linux-link doctor`: permissions, uinput, PipeWire/Wayland, VAAPI node enumeration, firewall ports,
  systemd user env, running-service version — the runtime checks that already exist internally (1575-1579).
- 2960 `doctor --fix` restricted to things that are safe to auto-apply, with the rest printed as commands.
- 2961 doctor checks for the hybrid-GPU node selection and for the NVENC driver-version gate, reporting the
  reason a rung was skipped rather than the fact.
- 2962 `linux-link selftest`: a bounded capture→encode→decode roundtrip against a pinned clip.
- 2963 `linux-link benchmark`: fps/bitrate/latency over a fixed workload, emitting 2939-shaped output.
- 2964 shell completions for all verbs and flags (1501-1505, needs `clap_complete`).
- 2965 a `-v`/`--verbose` that is actually read (1514 declares it and never uses it).
- 2966 CLI pickers for monitor, window, quality, codec, and audio route — desktop-side equivalents of what
  only the phone can do today.
- 2967 `linux-link trust export|import` for headless setup.
- 2968 log selection and rotation from the CLI, not only from `RUST_LOG`.
- 2969 a `--quiet` machine mode and a non-zero exit code contract for scripting.
- 2970 tests for every verb's `--json` shape as a golden fixture (1598 currently tests one parser).

**Linux desktop UX**
- 2971 a tray icon (appindicator) with live-session count, privacy-shield toggle, and one-click disconnect.
- 2972 a desktop entry + icon so the app is launchable from a launcher, not only from a terminal.
- 2973 a connect-request dialog on the desktop: who, from where, with what capabilities, accept/deny.
- 2974 a persistent "being controlled" indicator that complements the E4 perimeter shield.
- 2975 `WatchdogSec` and `Type=notify` on the user unit so a wedged server is detected, not assumed alive
  (1565-1567).
- 2976 a desktop-side "last sessions" viewer reading 2939.

**Product plumbing**
- 2977 one place that answers "what version is this phone talking to", enforced at handshake (D4).
- 2978 a changelog generated from conventional commits at release time, replacing the hand-written file.
- 2979 feature flags for anything shipped-but-dark, so a release builds can be gated without a revert.
- 2980 an a11y + i18n + strings-count gate in CI, since 161/161/161 is currently held by discipline alone.

### AE. Ecosystem, packaging & research 2981-3000

- 2981 an AUR release flow that produces a verifiable `PKGBUILD` checksum instead of `SKIP`.
- 2982 a signed release artifact set: tarball, checksums, detached signature, and a documented verification
  step in the README.
- 2983 a Fedora/openSUSE spec or a `cargo-generate`-style dist pack, if anyone outside Arch actually asks.
- 2984 a Flatpak spike for the *client* only, to learn whether sandboxing kills uinput/portal assumptions.
- 2985 a Docker/no-GPU CI runner matrix: kernel + headless capture, so the encode ladder gets exercised
  against a machine that is not this laptop.
- 2986 packaging docs that state the systemd **user**-unit requirement and why a system unit can never work
  (capture needs the session's Wayland/PipeWire).
- 2987 a supported-distribution matrix written from test results, not from ambition.
- 2988 a security contact and a disclosure policy on the repo page.
- 2989 a documented threat model: pairing TOFU, PIN brute force, post-pairing exec scope, relay trust.
- 2990 a privacy data statement — what crosses, what is stored, for how long (pairs with 2914).
- 2991 upstream contribution candidates: the Hyprland socket1 write-dispatcher break (#16224), the
  `ext-image-copy-capture-v1` capability detection, libinput absolute-pointer quirks.
- 2992 an alternatives study, re-run yearly: RustDesk, Sunshine, WayVNC, Input Leap — what they do better
  and what we should steal as technique, never as code (AGPL/GPL rule).
- 2993 a KDE Connect interop statement: which packets we reuse, why, and where the compatibility ends.
- 2994 a WebRTC comparison note for the record, so "why not WebRTC" stops being an open question in
  reviews; it is a transport decision with a cost, not an oversight.
- 2995 AV1: evaluate **only** when a target device with AV1 decode exists. This is the deferral, stated as
  an id so it cannot be mistaken for backlog that is ready to build.
- 2996 virtual displays: revisit when `ext-output-management-v1`/portal-based approaches can be
  capability-detected on the compositors we support (Z range), not before.
- 2997 plugin marketplace: explicitly deferred. The current plugin registry is an internal seam, and a
  public ABI is a promise we are not positioned to keep.
- 2998 a web client: deferred behind fundamentals; if it ever happens it is a WASM decoder over the same
  message classes, not a second protocol.
- 2999 AI features: deferred. Nothing in the roadmap needs them, and the measurement loop does not benefit
  from them.
- 3000 the macro/automation epic (L) deferred in full — 97 % of that range describes an engine that does not
  exist, and the prerequisite is 2939-shaped session records plus an input path with delivery semantics
  (AA), not a scripting language.




