# Device Verification Checklist

The ordered battery to run against a real phone (arm64, Android 8.0+/API 26, debug APK with the bridge
`.so`) and a real desktop. Work top-to-bottom: later sections assume earlier ones pass. Record failures as
`FAIL: <symptom>` next to the box; each item maps to the commit/feature noted so fixes land in the right
place.

The open items, by where they can run (here, Linux, a SIM, a second phone, away from home, or never on
the owner's box), are in `docs/owed-checks.md`, generated from this file by `scripts/owed_checks.py`: run it
after ticking items, or CI fails.

Several sections are now ticked from real runs (OPPO CPH2359 over LAN and tailnet, 2026-09-22 onward).
What is *not* ticked is not verified, however confident the code looks — and §22 is the standing register
of exactly that, including the one fix that merged without its confirming observation.

## 0. Prerequisites

- Desktop: `cargo run -p pocketdesk-server` (or installed server) on the
      Hyprland machine; FFmpeg + PipeWire + xdg-desktop-portal present.
- Phone: `adb install -r android/app/build/outputs/apk/debug/app-debug.apk`
      (build it first: `cargo ndk` for the bridge `.so`, then `assembleDebug`).
- Both on the same Wi-Fi LAN; phone can reach desktop port 1716 (TCP)
      and the QUIC streaming port (default 50000-range per config).
- Grant the app POST_NOTIFICATIONS when prompted (connect screen).
- Desktop: user in `input` group (uinput + `/dev/input/event*` read for
      privacy mode) — `groups $USER` should show `input`.
- Note which items are Hyprland-specific (window picker, workspace HUD,
      monitor geometry) — retest on a plain session if that's a target.

## 1. Connect + pairing gate (Tier 1 #5, Tier-2 #11b)

- [ ] Enter desktop IP + streaming port + control port → "Connect" opens a session.
- [x] First connection to an unpaired computer: the session's card says it was turned down and offers Pair again, which opens the pairing sheet (the sheet no longer opens by itself). Seen 2026-10-23 on the OPPO (S1).
- [x] "Show PIN on desktop" → desktop pops a notify-send PIN → phone confirms "Paired ✓", button flips to "Paired ✓". (seen 2026-10-23 on the OPPO against a throwaway server (S2): "Show a PIN on the computer" put a Windows toast with the PIN, and that PIN paired the phone.)
- [x] Second connect to same host: no pairing prompt (persisted both sides). (seen 2026-10-23 on the OPPO against a throwaway server (S1): after pairing, Connect opened a session with no prompt.)
- [x] Manual flow: `pocketdesk pair` on desktop → type 6-digit PIN on phone → paired. (2026-10-23 (S1): `pocketdesk pair 482913` on a throwaway server, the PIN typed on the OPPO, paired.)
- [x] Wrong/expired PIN → clear error, retry works. (seen 2026-10-23 on the OPPO against a throwaway server (S2): "Wrong or expired PIN.", then the computer's PIN paired.)
- [ ] With an *unpaired* second phone: control features (clipboard, monitors) must stay locked out.
- [x] QUIC gate (v2 + video): before pairing completes, video must not flow — desktop log shows (2026-10-23 (S1): an unpaired OPPO's session refused, the log saying "session refused: the device is not paired"; after pairing it was taken.)
      "Streaming session rejected: device not paired" (or "Unpaired device … on v2 QUIC") and the
      phone's status chip goes Down; after pairing, video works with no app restart beyond Retry.
- [x] QUIC gate over WAN (iroh): unpaired dial is rejected the same way (desktop log + no frames). (2026-10-25, the runner's `al2-unpaired-road`: the OPPO taken out of a throwaway's paired list, its keys kept, dialing by the internet road: refused at its control link in the server's log ("Refused …'s control connection over the internet"), never linked, no session with frames. An earlier try that reached a session straight was refused at the session's gate, its record `rejected`.)

## 2. Session shell + video (Tier 1 #5, R2#2, R2#5)

- [x] With "Show connection details" on, the HUD shows fps/kbps/rtt/delay/lost, plus the caption naming what
      the rates and the link figure were measured over (§2d). (Seen 2026-10-23 on the OPPO against a throwaway server (S1): `fps 12`, `kbps 950`, `rtt 23 ms`,
      `delay 18 ms`, `lost 0`, "rates over 2.5 s · rtt median of 8", "WAN · direct".)
- [ ] Video appears within ~1 s of Connect on a LAN session. (where: owner) A throwaway server's LAN port needs
      a firewall rule on this box; through `adb reverse` the phone waits out the LAN dial first, and its
      first picture came 11–14 s after Connect by the internet road.
- [ ] The session's notification (titled "Pocketdesk", with Disconnect) survives Home and a swipe; its Disconnect
      action ends the session. (2026-10-23, S1: posted with its Disconnect action; ColorOS's shade does not show it to
      uiautomator and screenshots are blocked during a session, so the tap is by hand.) (where: by hand)
- [x] Screen off for a minute, then wake: the stream stops with the screen (its surface goes, as it has since the first
      decoder) and comes back by itself. (Seen 2026-10-23 on the OPPO against a throwaway server (S1): a new session 10.9 s after waking, the picture at 16.8 s.)
- [ ] Drop phone far from AP / saturate link → frames stall, then recover; the HUD's `lost` rises but no permanent freeze (gap-driven keyframe request, R2#5). (where: by hand)
      "Drops" means **frames the phone never received** — the holes in the server's per-frame sequence
      numbers, counted where only the client can count them. So a saturated link must move it (the
      desktop's transport task trims its backlog before a stall clears), while a frame that arrives late
      and intact must not: the number can come back down when an out-of-order frame lands, and that is
      correct, not a bug. A decoder that cannot keep up is a different failure and shows as fps below the
      desktop's `src` rate, not here.
- [x] Status chip: kill the server → "Down: <reason>" chip + Retry rebuilds after restarting the server. Seen: "The computer took too long to answer. Make sure it is awake and reachable." with Retry about 45 s after the server stopped; once it was back the session came back by itself, no Retry needed (2026-10-19, A71, throwaway server, by script).
- [ ] R4 B1 screencopy backend (desktop-observable): starting a stream on (where: linux)
      Hyprland must NOT raise the portal screen-share grant dialog; server log shows
      "Capture started via backend Screencopy". If instead it shows "Capture backend
      Screencopy unavailable (…); trying next" followed by "Capture started via backend
      Portal", the reason is in the line — that's the fallback working, but note it as a
      finding. `POCKETDESK_SCREENCOPY=0` forces the portal path.
- [ ] R4 B3 capture_backend config override: set `capture_backend = "portal"` in (where: linux)
      config.toml, restart, start a stream → log shows "Capture started via backend Portal"
      and NO screencopy attempt line (pipeline pinned). With `capture_backend = "screencopy"`
      on a NON-wlroots/portal-only Wayland, the server must fail the session with a
      "Capture backend Screencopy unavailable" error rather than silently falling back to the
      portal (explicit intent surfaces). Omitting the key (or `"auto"`) restores the
      screencopy→portal fallback above. An unknown value (e.g. `"v4l2"`) makes the server
      refuse to start with a TOML parse error.
- [ ] R4 B1 idle pacing + freshness: keep the desktop static → `src` fps drops to ≤10 (where: linux)
      (damage-driven VFR); move a window → back to target fps within a frame or two, no tearing
      or upside-down frames (YInvert handling).
- [ ] R4 B1 multi-monitor: pick monitor N in the picker → log line "Screencopy capture: output at (where: linux)
      (x,y)" shows that monitor's wl_output origin and frames are its size.
- [x] R4 C1 codec negotiation, default off: connect normally → server log shows
      "Client codec caps negotiated" with `codec=H.264` even on a HEVC-capable phone
      (allow_hevc defaults false); video unaffected.
      (2026-10-24, the OPPO, which announces HEVC, over the internet road against a throwaway server:
      `codec=H.264` by default; the runner's `x3-codec`.)
- [ ] R4 C1 forced HEVC round-trip: `allow_hevc = true` in config.toml, restart server,
      reconnect → log shows `codec=H.265 (HEVC)`; phone decodes (sniffed `video/hevc`)
      with video within ~1 s and input still mapped. A pre-C1 phone build (no caps
      stream) must still get H.264.
      (The log part seen 2026-10-24 by `x3-codec`, the session up 8 s later; the decode not checked.)
- [x] R4 C1 HEVC + window crop: pick a window during a HEVC session → encoder rebuild at (2026-10-25, the OPPO by the internet road against a throwaway server with `allow_hevc`, the runner's `ae1-crop-hevc`, the window picked by position from the phone's picker: cropped to 1395x786, rebuilt at 1395x786 H.265 (HEVC), the session up; back to the whole desktop, 2560x1440 H.265 (HEVC), the session up. Twice.)
      window size keeps HEVC and the phone reconfigures on size change without codec change.
- [ ] R4 C2 startup ladder (desktop-observable): on a box where hardware encoding is broken
      (e.g. force `hardware_encoder = "nvenc"` here — this box's NVIDIA driver predates FFmpeg 9
      NVENC, so the sidecar dies), start a session → server log shows "Hardware encoder unavailable
      (…); falling back to software (C2)" and video still appears within ~2 s on x264.
- [ ] R4 C2 mid-session encoder kill (HEVC session): during a live `codec=H.265` session,
      `pkill -f 'ffmpeg .*vaapi'` (or kill the encoder child seen in `pstree` of the server) →
      log shows "Rebuilt encoder on software rung (C2)", video resuming within ~2 s and the phone
      toasts "Desktop switched encoder — now H.264" (MediaCodec swap re-sniffed from the first
      software IDR).
- [x] R4 C2 mid-session stall, same-codec: force the supervisor on an H.264 session (kill its
      sidecar child) → video recovers on software x264 with NO phone toast (identical bitstream —
      the server log line is the only signal; that is expected). (seen 2026-10-23 on the OPPO against a throwaway server (S2): the throwaway server's NVENC sidecar killed mid-session, "Rebuilt encoder on software rung (C2) reason=\"repeated encode failures\"", the picture back.)
- [x] R4 C2 bottom rung exhausted: with software already live, keep killing encoders until the
      log shows "Encoder bottom rung exhausted; ending session" → stream goes Down cleanly
      (status chip + Retry works), never a silent infinite stall. (seen 2026-10-23 on the OPPO against a throwaway server (S2): the software sidecar killed too, "Encoder unrecoverable after 90 failures; ending session" (today's words), and Retry opened a new session on NVENC; on a still screen the phone's 10 s stall watch ends it first and reconnects by itself.)
- [x] R4 C2 crop stability: during a software-fallback session, pick/clear a window crop →
      encoder rebuild must NOT re-log the hardware fallback (sticky `encoder_preferred` — a
      rebuild must not re-probe the dead hardware).
      (2026-10-25, the runner's `am1-sticky-software` on the OPPO: a throwaway whose hardware rungs all fail as they open (`POCKETDESK_FAIL_ENCODERS`, a test seam) fell back to software at the session's start, once; a crop to a 1395x786 window and back rebuilt the encoder with no hardware rung tried again, the session up, its record's encoder the software sidecar.)
- [ ] R4 C3 NVENC power pin (**NVIDIA hardware only — skip on non-NVIDIA boxes**): start an NVENC session (server log "Video encoder sidecar" on `h264_nvenc`/`hevc_nvenc`) → server log "pinned NVENC GPU 0 graphics clock to N MHz"; `nvidia-smi -q -d CLOCK -i 0` shows the graphics clock held at its max. End the session → log "restored NVENC GPU 0 to default clock management" and `nvidia-smi -q -d CLOCK` shows clocks back under adaptive management. On an unprivileged desktop the pin self-declines (log "clock lock refused ... running unpinned") and no reset is issued. (where: linux)

## 2c. In-process VAAPI encode (R4 C4) — desktop/box-observable, no device needed

Note: there is **no `hardware_encoder` key in config.toml yet** — the streaming server constructs
`StreamingConfig` with `HardwareEncoder::Auto` (`streamer.rs`), which `VideoEncoder::new` resolves via
`resolve_encoder(Auto, probe_encoders())`. So on a live session the exact rung is whatever Auto picks;
the deterministic C4 proof on the host is the unit test below, and the desktop-observable checks are the
backend log line + the `POCKETDESK_VAAPI_DEVICE` override (honoured by both the in-process and sidecar
paths regardless of the config knob). Forcing a *specific* encoder end-to-end is a known future knob,
not part of C4.

- [ ] R4 C4 host proof (no phone): `cargo test -p pocketdesk-core --lib encoder_vaapi` →  (where: linux)
      `test_vaapi_encode_roundtrip` encodes 40 real frames through `h264_vaapi` on this box's Intel
      iHD node and asserts Annex-B packets + ≥1 keyframe (self-skips green where no VA node exists).
- [ ] R4 C4 device-node selection: on the hybrid Intel+NVIDIA box the probe must land on the (where: linux)
      VA-capable node, NOT the hardcoded NVIDIA `renderD128` — `POCKETDESK_VAAPI_DEVICE=/dev/dri/renderD129`
      starts a session → server log "Video encoder: in-process VAAPI (h264_vaapi) on /dev/dri/renderD129"
      and **no `ffmpeg` sidecar child** is spawned for the vaapi path (`pstree` of the server). Point the
      override at a VA-less node (e.g. `/dev/dri/renderD128`) → it degrades to sidecar-then-software (C2),
      never hangs.
- [ ] R4 C4 quality (needs a phone): a VAAPI-encoded session decodes with correct color (no NV12 chroma (where: linux)
      shift) and the GOP (fps×2) keyframe lets first-frame render land within ~2 s.

## 2a. Desktop-side session visibility + kick (R4 D2)

- [x] Consent notice: first phone connect → desktop shows a "Pocketdesk: a device is
      streaming this desktop" notification with device id + LAN/WAN + peer (headless: the same
      line appears in the server log instead). (seen 2026-10-23 on the OPPO against a throwaway server (S1): a Windows toast for each session, the reconnect after waking included, read back from Windows' notification store.)
- [x] `pocketdesk status` during the session → tailscale output plus a "Streaming sessions
      (live):" line (short device id, transport, peer, up-seconds). With no session: "none live".
      Found: on a computer without Tailscale it stopped at "tailscale status failed" and listed
      nothing; fixed, it now says "Tailscale: not running (it is optional)" and lists the session (2026-10-19, A71, throwaway server, by script).
- [x] `pocketdesk kick <id-prefix>` (≥6 chars, or full id, or peer IP) → phone shows the Down
      status chip within ~2 s and the status line disappears; other sessions are untouched.
      By an 8-character prefix: gone in 2 s, the phone saying "The connection to the computer
      dropped…" with Retry, which opened a new session (2026-10-19, A71, throwaway server, by script) (one phone, so "other sessions" untried).
- [x] `pocketdesk kick all` ends every live session at once (2026-10-19, A71, throwaway server, by script).
- [x] kick with no running server → "no running Pocketdesk server to kick through", nothing
      written; a bogus target logs "kick: no live session matched" and the live session survives.
      Now worded "no running Pocketdesk server to end sessions through"; nothing in `kicks/` (2026-10-19, A71, throwaway server, by script).
- [x] Stale request safety: with no daemon running, hand-write `$XDG_STATE_HOME/pocketdesk/kick`
      (`all\n<unix-secs older than 60s>`), start the daemon, connect a phone → the session must
      NOT be killed (the expired request is dropped with a warn on first tick). Done with a request
      file in `kicks/` (since N7, one file each) two minutes old: taken, and the session held (2026-10-19, A71, throwaway server, by script).
- [x] Phone disconnect (normal Exit) → status line goes away within ~2 s; no leftover session in
      `live_sessions.json` (registry deregisters on pipeline exit) (2026-10-19, A71, throwaway server, by script).

## 2b. Compositor-true latency read (R4 E3)

- [ ] Sanity: on-LAN idle desktop → HUD `delay` (the e2e estimate) should sit in the low tens of ms and track the
      `rtt` chip sensibly (e2e ≈ capture age + rtt/2); heavy desktop motion → `delay` rises
      (encoder/queue age grows), never reads 0 once video is flowing.
- [ ] The old bug-shape check: `delay` must NOT equal `rtt` while frames are under load (it used
      to be a literal RTT proxy).
- [ ] Cross-check (eyes, optional): point the phone camera at the desktop with an on-screen
      clock/stopwatch — the phone's rendered image lags the physical panel by roughly the HUD
      `delay` figure (± the phone's own present latency, which the probe can't see).
- [ ] WAN session: `delay` should ≈ rtt/2 higher than a LAN session on the same desktop motion
      (the network-leg term is RTT/2 by construction).

## 2d. Rate and link basis (roadmap 2056)

The HUD's fps/kbps are now diffs over the newest 3 s of received video and its `rtt` is the median of
the session's last ≤8 one-second polls; a caption under the numbers names both, and a figure whose basis
is still filling in is dimmed. What to confirm on a real phone:

- [ ] Right after connect: the caption starts at something like `rates over 0.5 s · rtt not measured yet` and
      reaches `rates over 3.0 s · rtt median of 8` within ~8 s; fps/kbps are dim until the caption says
      ~2.5 s or more, and `rtt` is dim until its first poll (~1 s). A number that arrives already bright
      means the basis is not being reported.
      (2026-10-24, the OPPO over the internet road, the caption only: `rates over 1.0 s · rtt not measured
      yet` at 3.5 s, `rates over 2.6 s · rtt median of 6` at 18.5 s. The first poll came ~14 s in, as the
      connect itself took that long by the internet road; the 8 s and the dimming still owed on the LAN.)
- [ ] The stall check this replaces: freeze the desktop's output (sleep the display, or `SIGSTOP` the (where: linux)
      server) → HUD `fps` and `kbps` must reach 0 within ~3 s and stay there, with the caption still
      naming a real span. Under the old lifetime average this was the bug: a dead link read healthy for
      minutes. Resume → the rates recover within ~3 s, not within a session's worth of averaging.
- [x] Basis reset: end the session and reconnect to the same (or a different) desktop → the caption must
      come back up from a small span and `rtt median of 1`, never showing a figure or a sample count
      carried over from the session that just died.
      (2026-10-24, the OPPO over the internet road against a throwaway server, read from UI dumps: it
      failed first, the new session's caption reading `rtt median of 8` for its first ~10 s while the
      connect went home then by the internet road (V7); with the fix it started at `rates over 1.0 s ·
      rtt not measured yet` and counted up from the new session's own polls.)
- [x] Portrait legibility: the caption is one line of 10 sp monospace; in portrait on a narrow phone it
      must not wrap into a single-character column the way the six metric chips did before they became a
      `FlowRow`, and must not be clipped by the HUD panel's padding.
      (2026-10-24, the OPPO in portrait over the internet road: the caption one line, 48 px tall by 664 wide, read from a UI dump.)
- [ ] Optional cross-check: while the caption reads `rates over 3.0 s`, `pocketdesk sessions` for the same
      session should report a mean kbps within the same order of magnitude — the session record averages
      over the whole session, the HUD does not, and a large disagreement is a measurement bug in one of
      them.

## 3. Input paths (R2#4, Tier 1 #2)

- [x] Direct-touch: tap lands at the touched point. **Verified 2026-09-22 on OPPO CPH2359 → Hyprland:** taps map pixel-exact through the letterbox (phone (540,1230) → cursor (768,432) = desktop center). Under Wayland the injector uses uinput (enigo/XTEST is inert on Hyprland): motion rides the "Pocketdesk Virtual Abs Pointer" device (ABS_X/ABS_Y + BTN_LEFT, created eagerly at startup so the first tap is not lost); keyboard/relative ride "Pocketdesk Virtual Input".
- [ ] ...and a tap is actually a **click** (roadmap 2061, re-verify 2026-09-24). The 2026-09-22 note above (where: linux)
      claimed "click (BTN_LEFT) lands", but the code that ran sent absolute motion plus a left *release* and
      never a press — so that half was either an inference or a window reacting to something else. A tap
      must now put `BTN_LEFT` down (value 1) and up (value 0) at the touched point: check with `evtest` on
      the abs-pointer device rather than by whether a window happened to react.
- [ ] Drag (roadmap 2061, shipped 2026-09-24, device-unverified): start a slow drag-select across desktop text — `evtest` should show the pointer warp to the touched point, `BTN_LEFT 1`, moving `ABS_X/ABS_Y` while held, `BTN_LEFT 0` on lift, and the selection must follow the finger (before this change a drag was motion-with-button-up: nothing began, and the release landed on a button that was never pressed). Then the leak check: begin a one-finger drag and add a second finger mid-drag (pinch) — the button must still come up, and the desktop must not be left dragging. Note there is no server-side safety net for a lost release packet yet (roadmap 2272): a session that dies mid-drag leaves `BTN_LEFT` held on the uinput device until the next release. (where: linux)
- [ ] Trackpad mode: pointer moves without jumping to touch point; tap = click; two-finger = scroll. (where: owner)
- [ ] Mode toggle switches behavior live, no session restart. (where: owner)
- [ ] Keyboard: type letters/digits/modifiers via on-screen shortcuts + remote input — check against `evtest`: Super, Alt+Tab, Ctrl+Alt+Del, PrtSc, Esc, Super+1..9 work (ShortcutBar). (Esc tap verified 2026-09-22; this box binds workspace cycling to Super+Tab, so the bar's Alt+Tab is config-dependent, not a pipeline failure.) (where: linux)
- [ ] The same keyboard list **run twice, once per injection backend** (roadmap 2057-2060). Under (where: linux)
      Wayland the server prefers uinput and emits the evdev code the phone sent, so nothing is
      translated; the enigo/X11 rung is the one that has to name every key, and it is the rung a
      machine without `/dev/uinput` access lands on. Read which one is live from the server log
      ("using uinput" vs "using enigo"), then re-run the list with the other by starting the server
      as a user who cannot open `/dev/uinput`. Symbols matter most here: `- = [ ] ; ' \` , . /` and
      the dialpad `*`/`+` were refused outright on that rung before the table was completed.
- [ ] holdKey/repeat (e.g. hold an arrow key) behaves. (where: owner)
- [ ] R4 D1 view-only: with the "View-only" switch on in the session menu, EVERY remote input goes dead on the
      desktop (taps, shortcuts, workspace chips) while video, the HUD, clipboard sync and the screen and window
      pickers keep working; switched off, input is restored without a reconnect; on across a screen switch, input
      stays blocked through the rebuild. (where: owner)
- [x] The view-only switch logs "View-only mode changed" (enabled=true, then false), and a screen switch's new session
      logs it again while it is on. (Seen 2026-10-23 on the OPPO against a throwaway server (S1).)

## 4. Zoom + display mapping (Tier 1 #6, R3#7)

- [ ] Pinch-zoom in direct-touch mode pans/clamps correctly; taps still land on the visible desktop point while zoomed. (where: owner)
- [ ] Trackpad mode: two-finger still scrolls remotely (no zoom capture). (where: owner)
- [ ] Letterbox: on a phone/desktop aspect mismatch, taps map through the letterbox offset (no systematic tap drift). (where: owner)

## 5. Window + monitor streaming (R3#7, Tier-2 #10)

- [ ] Window picker lists Hyprland windows with title/class/workspace/size + "focused" marker. (where: linux)
- [ ] Picking a window: video re-encodes at window size (encoder rebuild), crop tracks the window, taps map to desktop coords via the screen box. (where: linux)
- [ ] Moving the window mid-session: crop follows only after re-pick (known limitation — crop is static rect). (where: linux)
- [ ] "Whole desktop" clears the crop. (where: linux)
- [ ] R4 B2 (Hyprland): pick a window → server log shows "Streaming compositor-cropped window frames"; partly-occluded window streams correctly (occluder NOT in the video), and only the window's pixels are encoded. (where: linux)
- [ ] R4 B2: close the streamed window mid-session → stream keeps running on full-output frames with software crop (warn log "did not stage"; no session drop). (where: linux)
- [ ] R4 B2 (non-Hyprland wlroots, no export global): window pick still works via software crop exactly as pre-B2. (where: linux)
- [ ] Monitor picker lists monitors via xcap; selecting index N streams that monitor's region; reconnect persists the choice per host.
- [x] Mid-session monitor switch tears down + rebuilds cleanly (no zombie session — bridge "already active" path). (seen 2026-10-23 on the OPPO against a throwaway server (S1): switching between two screens ended the old session and started one at the new screen's size each time, its picture back.)

## 6. Workspace HUD + battery (R3#8, Tier-2 #11a)

- [ ] Workspace chips appear under StatsHud; open/close/focus a window on desktop → HUD updates within ~4 s. (where: linux)
- [ ] Tap a chip ≤9 → Super+N injected → workspace switches. (where: linux)
- [ ] R4 E1: a row of window chips (title/class) shows under the workspace chips on a Hyprland session; tap one → video re-encodes cropped to that window (same as the picker), the tapped chip goes solid/highlighted. (where: linux)
- [ ] R4 E1: re-tap the highlighted window chip → crop clears back to whole-desktop (monitor) view; a *different* window chip tap switches the crop directly. (where: linux)
- [ ] Roadmap 2066 (needs a second output or a hotplug-capable desktop): with the HUD open, unplug/replug a monitor → server log shows `Hyprland output changed, refreshing compositor state` within a second of the event (not up to 4 s) → HUD chips and window-crop geometry re-layout immediately. Never observed here: this box has one built-in output and Hyprland's socket1 write dispatchers are broken upstream, so no fake hotplug exists. (where: linux)
- [ ] Battery chip shows desktop percentage + charging state; matches `upower`.

## 7. Clipboard + files (Tier 1 #4)

- [x] Copy on phone → desktop clipboard updated within ~4 s; copy on desktop → phone receives. (seen 2026-10-23 on the OPPO against a throwaway server (S1), with the fix in #280: the computer's copy pasted on the phone during a session; the phone's copy on the computer 2.5 s after the app came back. Before the fix the throwaway server answered from this computer's real clipboard.)
- [x] No echo loop (toggle sync off/on, paste both ways repeatedly). (2026-10-24, one way: the runner's `z1-echo` in a session over the internet road, a copy made on the throwaway's stand-in clipboard was sent to the OPPO once (the server's new "Clipboard sent to" line) and not sent back in the next 20 s, the computer's copy kept; with AA1's line the phone also said it took it.)
- [ ] The same from the phone's side, and with the sync turned off and on: it needs the phone's Copies by themselves (Pocketdesk control).
- [x] History sheet: 20 entries survive app restart; pick restores local + pushes. (seen 2026-10-23 on the OPPO against a throwaway server (S1): 20 entries kept across a force-stop and a reboot; a pick put the entry on the computer at once.)
- [x] Share text to the app from any Android app → lands in desktop clipboard, toast "Sent to clipboard." (seen 2026-10-23 on the OPPO against a throwaway server (S1): a text shared into the app on the computer in 2.5 s; the toast is not visible to uiautomator.)
- [x] Share a file (EXTRA_STREAM) → appears in the desktop's Downloads folder. The desktop connects to the phone and presents the one-time key from the request; the phone serves the bytes only then. Verified on 2026-10-13 against the Windows host over LAN: 300,000 and 1,500,000 bytes with matching MD5, a second share of the same name saved as `name (1).ext`, the Mark of the Web on the received file, no `.part` left behind. Before this the phone and the server both listened and neither connected, so no file share had ever completed. It needs the TCP control channel on a LAN address; not over `adb reverse` (the desktop would dial itself), and not over the v2 channel or WAN.
- [x] Reboot phone → history + saved hosts survive. (seen 2026-10-23 on the OPPO against a throwaway server (S1): after a reboot the 20 entries and the saved computer were there.)

## 8. Notifications + find-my-device (R3#11c, Tier-2 #11)

- [ ] `notify-send -a Test "Hi" "body"` on desktop → phone notification appears (channel "Desktop notifications"). Tried on the Azure VM (Xvfb, `dunst` as the daemon, 2026-10-24): the server's monitor started (dbus-monitor) and `notify-send` succeeded, but the phone showed nothing: outside a session, as it should (the phone shows the computer's notifications only in a session's screen), and the session tried with it did not start, so the item stays open.
- [ ] Inline reply from the shade → desktop `replies.log` records it, text on desktop clipboard, confirmation popup; originating notification disappears. (where: linux)
- [ ] "Ring PC" → desktop plays sound 30 s (pw-play/paplay path; silent no-op is FAIL here since desktop has audio). (where: by hand)
- [x] Desktop pushes findmydevice ring → phone alarm loops at max volume 30 s (phone in alarm-volume >0; check DoNotDisturb doesn't block USAGE_ALARM). (2026-10-23 (P11, Q1's `p11-ring`): `pocketdesk ring` rang the OPPO 30 s at its alarm volume (no longer the maximum since N4), with the alarm vibration and the torch.)

## 8a. Control-channel versioning (R4 D4)

- [ ] Reply still works end-to-end after the `kdeconnect.notification-reply` → (where: linux)
      `kdeconnect.pocketdesk.notification_reply` rename (desktop-side check): reply from the phone
      shade → `replies.log` records it + confirmation popup (the server only matches the new type
      now, so an old client build's reply would silently no-op — both ends must be from this build).
- [ ] `llVersion` tag is present on our extension packets only: capture the control-channel line
      (e.g. `journalctl`/strace the JSON, or a debug log of `to_wire` output) — a
      `kdeconnect.pocketdesk.privacy`/`.audio`/`.state` packet carries `"llVersion":1`; a native
      `kdeconnect.identity`/`kdeconnect.pair`/`kdeconnect.clipboard` packet does NOT.
- [ ] Forward-compat: hand-inject a `kdeconnect.pocketdesk.privacy` line with an extra unknown
      top-level field and an unknown body key → server still parses and acts (unknown fields ignored).

## 9. Privacy mode + lock (Tier-3 #15, R4 E4)

- [ ] "Privacy: on" → physical keyboard/mouse on desktop stop responding (`evtest`: grabs held), while phone input still flows. (where: linux)
- [ ] Compositor shield (R4 E4, Hyprland/wlroots): the same "Privacy: on" also paints a full-perimeter red frame on every monitor within ~1 s; the desktop centre stays visible and interactive from the phone (the ring is `KeyboardInteractivity::None`, click-through). Desktop log shows "privacy: layer-shell privacy shield engaged". (where: linux)
- [ ] Leave session with privacy on → release happens (phone app exit); also verify 10-min TTL auto-release by force-killing the phone app mid-grab. (where: linux)
- [ ] Shield tears down with the grab: "Privacy: off" → the red frame disappears immediately; force-killing the phone app mid-grab → frame gone after the TTL auto-release. (where: linux)
- [ ] Non-wlroots desktop (GNOME/KDE/X11, or `POCKETDESK_SCREENCOPY=0` headless): no shield is attempted — grab still works, no crash, no "shield engaged" log line. (where: linux)
- [ ] "Lock PC" / session-notification "Lock desktop" → screen locks (loginctl path).
- [ ] Without `input` group: clean error, no crash. (where: linux)

## 10. Audio control (Tier-3 #16)

- [ ] Audio sheet opens with volume %, mute state, sink list (matches `wpctl status`).
- [ ] Slider → desktop volume changes on release; mute switch works; selecting a sink routes default output (verify with `wpctl inspect` + real playback).

## 10a. Phone mic share (R4 E2)

- [x] "Mic: off" tap → RECORD_AUDIO permission dialog; denying it toasts "Microphone permission denied" and the toggle stays off. (2026-10-25, the runner's `af3-mic-refused`, once: Phone mic share asked, Don't allow left the switch off and the permission not granted. The toast cannot be read by uiautomator. The OPPO's permission now carries that one refusal (USER_SET; the shell may not clear it): it still asks next time, and the check SKIPs.)
- [ ] Granting it → button reads "Mic: on" (amber) and the desktop gains a source: `pactl list short sources | grep pocketdesk_mic` (or pavucontrol → Recording). (where: linux)
- [ ] Speaking moves the pavucontrol input meter for "Pocketdesk Mic"; `pw-record --target pocketdesk_mic /tmp/mic.wav` captures audible speech (play it back). (where: linux)
- [ ] "Mic: on" tap → source disappears immediately; `pgrep -f pocketdesk_mic` empty (no leaked pw-loopback children after repeated toggling). (where: linux)
- [ ] View-only latched → mic keeps flowing (it is session media, not injected input); input is still dropped. (where: linux)
- [ ] Exit the session while mic is on → source disappears (release-on-dispose); force-killing the phone app → source gone within the QUIC idle timeout (~45 s). (where: linux)
- [ ] Screen off / PiP while mic is on (Android 14+): audio keeps flowing — the FGS carries the microphone type (logcat: no SecurityException, no silent AudioRecord). (where: linux)
- [ ] WAN (relayed or direct) session: the same checks pass over the iroh path. (where: linux)

## 11. WAN over iroh (R1)

- [ ] On same LAN: identity cached (logcat), status shows LAN. (where: owner)
- [ ] Phone on LTE (Wi-Fi off): dial from cached identity succeeds → badge shows "WAN · punching…" then settles to "WAN · direct" or "WAN · relayed — trying direct…", video + input + keyframe-on-gap all work through relays (R4 A1). (where: sim)
- [ ] Desktop reconnects (restart server) → identity re-announced within 30 s, phone re-dials.
- [ ] Hole punching through a real NAT (relay-free if possible): check `directAddrs` paths get used. (where: away)
- [ ] After both a LAN and a WAN session: `pocketdesk sessions` shows one line per session with distinct `outcome=` (`lan_direct` vs `wan_punched`/`wan_relayed`), plausible `rtt_ms`/`kbps`, and `dev=` = the paired deviceId. (where: owner)
- [x] Tail latency (roadmap 2141-2146): the same lines carry `enc_n`/`enc_p50`/`enc_p90`/`enc_p95`/`enc_p99`/`enc_max` and the matching `rtt_*` set. `enc_n` counts frames the encoder actually produced a packet for, so it must be non-zero and of the same order as fps × session duration — a large shortfall is its own finding (frames dropped upstream, or a path that never samples). `enc_p50` must sit near the typical frame time for the encoder rung in use (the log line names the backend at session start), and a deliberate stall — crop to a window, or unplug/replug Wi-Fi mid-session — has to move `enc_max` or `rtt_max` above `enc_p50`/`rtt_p50`. **A line with no `enc_*` at all means the sampling was never wired to the live pipeline: that is a bug, not a healthy session.** (seen 2026-10-23 on the OPPO against a throwaway server (S1) on a punched WAN session: `enc_n` 89 for 8 s at about 12 fps, `enc_p50` 8 ms on the NVENC sidecar, the `rtt_*` set present; the stall's effect is its own line below.)
- [x] A deliberate stall (crop to a window, or Wi-Fi off and on mid-session) moves `enc_max` or `rtt_max` above their p50. (2026-10-25: not by the session's record. Its encode tail times `encode_frame` alone, so a crop's rebuild never reaches `enc_*`, and `rtt_*` is sampled every 5 s apart from the crops; `max` sits above `p50` with no crop at all. Owed by another reading, such as the HUD's during the crop.) *Then (AH2, 2026-10-25): the record now times the encoder's rebuilds apart from its frames (`rebuild_*`), and twelve crops showed there: 19 rebuilds, p50 about 400 ms, max 413–573 ms, against an encode p50 of 10 ms. The stall is seen there, not in `enc_*` or `rtt_*`, by design.*
- [x] Device-reported tails (roadmap 2143-2146, the phone's half of the chain): the same lines must carry `dec_*`, `rnd_*` and `e2e_*` plus `phone_rtt`/`phone_lost`. `dec_n` counts frames the decoder actually rendered, so it has to track `enc_n` within the loss the link imposed (a `dec_n` far below `enc_n` is dropped-or-stalled frames, not a telemetry bug), `dec_p50` must be single-digit ms on a hardware rung and grow when the desktop is cropped to a bigger window, and `e2e_p50` must sit near the HUD's e2e figure for the same session. `rnd_p50` is the rendered-frame interval, so it should be near 1000/fps ms and must jump when the bitrate clamp or a preset slows the link. On a session that never delivered video (rejected, or killed before the first frame) `dec_*`/`rnd_*` are legitimately absent — that is the "nobody measured" case, and `key=0` in their place would be a bug. **The server side of this path is already proven without a phone: `cargo test -p pocketdesk-server --test session_record -- --ignored --nocapture` drives a real capture→encode→send→receive session over loopback and asserts the record carries all of these tails; what remains device-only is that `H264Decoder.kt` feeds `dec_*`/`rnd_*` with real MediaCodec timings rather than the numbers the test injects.** (seen 2026-10-23 on the OPPO against a throwaway server (S1): `dec_p50` 4 ms on NVENC, `rnd_p50` 81 ms at about 12 fps, `e2e_p50` 20 ms against the HUD's 18-19 ms, `phone_rtt`/`phone_lost` present; a 5 s session that sent video carried `e2e_*` but no `dec_*` (shorter than the phone's batch); the crop and preset effects are their own line below.)
- [ ] `dec_*` grows when the desktop is cropped to a bigger window, and `rnd_p50` jumps when the bitrate clamp or a preset slows the link.
- [x] Transport exposure (roadmap 2151-2158): every line from a session that lasted longer than the 5 s sampler must carry `lost_pk`/`lost_b`/`dgrams`/`tx_b`/`path_chg`/`relayed_s`, and `cong_ev`/`cwnd_pk`/`mtu`/`black` **on either transport**: quinn exposes them on the connection, and the iroh arm reads them off the *selected* path (`Connection::paths()`), so a WAN session riding a punched direct path reports the same set. A WAN line missing them is no longer the expected shape — the earlier "iroh drops the per-path values" claim was read off its connection-level aggregate, which really does discard them — and the only legitimate reason for a gap now is a snapshot taken with no selected path, which is why the field is `Option` and never a `0` pretending to be a measurement. `mtu` must be ≥ 1200 (quinn's `INITIAL_MTU`) and may climb as PLPMTUD probes pay off; `cwnd_pk` must be non-zero on any session that sent video. Walk between Wi-Fi bands, or kick the phone onto cellular mid-session, and `path_chg` has to count it; start a relayed WAN session and `relayed_s` must be non-zero with `outcome=wan_punched` if punching eventually won. (seen 2026-10-23 on the OPPO against a throwaway server (S1) on a punched WAN session: `lost_pk`, `dgrams`, `tx_b`, `path_chg`, `relayed_s`, `cong_ev`, `cwnd_pk`, `mtu` 1452 and `black` all present; moving paths is its own line below.)
- [ ] `path_chg` counts a walk between Wi-Fi bands or a switch to cellular, and `relayed_s` a relayed session. (where: by hand)
- [ ] Relayed → direct upgrade while streaming: badge flips to "WAN · direct" on its own (iroh keeps punching) and the bridge logs "upgraded from relay to direct path" (logcat). Relayed session must stay usable the whole time, not stall on the transition. (where: away)
- [ ] Relayed session (R4 A3): within ~2 s of the badge reading "relayed", the desktop log shows "Encoder bitrate target changed" (relay_cap engaged) at effective ≤ 2 Mbit/s followed by "Encoder rebuilt for new bitrate", and picture is visibly softer than LAN/direct. (where: away)
- [ ] Full-quality override (R4 A3): while relaying, the badge area offers "Full quality: off" — tap → log shows "Relay quality override changed" then a fresh "Encoder bitrate target changed" back to configured (`pocketdesk sessions` kbps over the next session line). Tap again to re-clamp; toggle survives a stream retry (re-arms). (where: away)
- [ ] Clamp release on punch-through: start relayed (override off), wait for the direct upgrade → within ~2 s the log shows "Encoder bitrate target changed" back to configured without touching anything. (where: away)
- [ ] LAN session (R4 A3): the relay clamp never engages (`stats().relayed` is always false on quinn → relay_cap is unbounded); the arbiter task still runs but emits no bitrate-change line until a preset is chosen below or the link starts losing packets. (where: owner)
- [ ] Loss response (roadmap 2053): degrade the link — `tc qdisc add dev <iface> root netem loss 5%` on the desktop, or walk the phone toward the far edge of the Wi-Fi mid-session — and within a few 2 s ticks the desktop log shows "Encoder bitrate target changed" carrying a `loss_cap=` field with effective below configured, then "Encoder rebuilt for new bitrate" and a visibly softer picture. The cut is 20 % per congested tick down to a 1 Mbit/s floor (a configured rate below the floor is never raised to it). Restore the link → the same line reappears **without** `loss_cap=` as the rate climbs back ~10 % per tick to configured, then stops being a term at all. (where: linux)
- [x] Loss response must not fire on a quiet desktop: hold the screen still (no window movement, no video) for a minute and the log must show no bitrate-change line. Below 100 datagrams in a tick the controller treats the sample as no information, which is the only thing distinguishing "the link is lossy" from "there was nothing to lose" — a bitrate cut on an idle LAN session is a bug, not a measurement. (2026-10-24, the OPPO over the internet road against a throwaway server, the runner's `w2-quiet`: a session held a minute with this computer's screen still, no "Encoder bitrate target changed" or "Adaptive bitrate" line.)
- [x] V8: a session started while the control link is by the internet road gives home 2 s, not 10: the OPPO against a throwaway server (no `adb reverse`), connected 3.0 s after the tap, where it had waited out the home dial's 10 s first (2026-10-24).
- [ ] V8: back home with the link still by the internet road, the session's home dial answers within its 2 s and the session is a LAN one. (where: away)

## 11b. HUD link-profile presets (R4 E5)

- [x] Quality is four choices in the session menu (Auto, Max, Balanced, Economy), Auto by default. (Seen 2026-10-23 on the OPPO against a throwaway server (S1).)
- [x] On LAN, pick "economy": desktop log shows "Link-profile preset changed" then "Encoder bitrate target changed" with effective ≤ 1.5 Mbit/s and "Encoder rebuilt for new bitrate"; the HUD bitrate/`pocketdesk sessions` kbps drop and the picture softens. Native resolution is NOT reduced (only bitrate) — verify the HUD reports the same width×height. (seen 2026-10-23 on the OPPO against a throwaway server (S1): Economy logged "Link-profile preset changed preset=3" and "Encoder bitrate target changed effective_bps=1500000", the encoder rebuilt at 1.5 Mbit/s (by the internet road; the LAN wording stands for any road).)
- [x] Pick "balanced": effective ≤ 5 Mbit/s. A configured rate below a band (low-res monitor) is never raised above it (the band is a ceiling, not a floor). (seen 2026-10-23 on the OPPO against a throwaway server (S1): Balanced gave 5 Mbit/s with 5 configured; Max changed nothing at the configured rate; the low-resolution case not tried.)
- [x] Preset survives a stream retry / monitor switch: choose "economy", trigger Retry — the new pipeline re-arms the preset (stays ≤ 1.5 Mbit/s without re-tapping). Server latches are per-session, so a full reconnect to a fresh `StreamingServer` resets to Auto (phone must re-pick) — expected. (seen 2026-10-23 on the OPPO against a throwaway server (S1): with Economy chosen, a screen switch's new session logged "preset=3" and rebuilt at 1.5 Mbit/s.)
- [ ] View-only interaction: while a preset is active, toggling View-only still drops injected input but the bitrate preset keeps applying (both are control-plane).
- [ ] A preset change mid-action is seamless: no freeze longer than one keyframe; the post-rebuild IDR reseeds the decoder cleanly (no green/garbled persist). (2026-10-25, the freeze half: the runner's `an1-presets` on the OPPO changed the preset four times mid-session, Economy, Max, Balanced and Auto, each logged by the server, the session up with no redial and its longest rebuild 460 ms. The picture after each is by eye: a screenshot would hold this computer's desktop.)
- [x] Auto is a no-op on (re)connect: with "Quality: auto" and a LAN session, tapping Retry sends no preset packet and logs no "Encoder bitrate target changed" (only the relay floor would fire, and not on quinn). (seen 2026-10-23 on the OPPO against a throwaway server (S1): with Auto chosen, a screen switch's new session logged no preset line.)


## 12. Roaming (Tier-3 #13 remainder)

- [ ] Mid-stream, toggle Wi-Fi off → phone falls to LTE → within a few seconds status chip shows connecting and a fresh WAN/LAN session comes up (reconnect-on-new-radio, not live migration). (where: away)
- [ ] Wi-Fi↔LTE ping-pong with 3 s cooldown: flapping radio must not thrash a healthy stream. (where: away)
- [ ] In PiP: network change must NOT tear the float session (pip guard). (where: away)

## 13. PiP + multitask (Tier-3 #14, DeX)

- [x] "PiP" button → chrome hidden, video-only float with correct aspect (matches decoded frame size after first SPS); input goes to system. (seen 2026-10-23 on the OPPO against a throwaway server (S1): Picture-in-picture from the session menu, the window pinned at 589×333 for a 2560×1440 screen, no chrome, the session kept.)
- [x] Expand back → chrome returns, session uninterrupted. (seen 2026-10-23 on the OPPO against a throwaway server (S1): opened again from the launcher (ColorOS's PiP window offers uiautomator no expand control), the chrome back and the session kept.)
- [ ] DeX/docked or split-screen: session is resizeable, letterboxing recomputes, taps still map correctly. (where: other hardware)

## 14. Blackout / pocket mode (Tier-3 #15 remainder)

- [x] "Blackout" → black screen, "double-tap to unlock" hint, screen dimmed; stream continues (watch desktop from another device or check fps in logcat). (seen 2026-10-23 on the OPPO against a throwaway server (S1): "Blackout — double-tap to unlock" over the session, a brightness override of the window, the session kept on the computer.)
- [x] Screenshot during blackout → blocked (FLAG_SECURE): the window flagged secure and a screencap all black; the flag gone after Back (2026-10-19, A71, throwaway server, by script).
- [ ] Recents thumbnail blank during blackout. (where: by hand)
- [x] System Back unlocks and does not exit the session (2026-10-19, A71, throwaway server, by script).
- [x] Double-tap unlocks. (seen 2026-10-23 on the OPPO against a throwaway server (S1): a double-tap lifted it and its brightness override.)
- [x] PiP is entered only from the session menu (Home does not enter it), and a blackout covers that menu, so PiP
      cannot be entered while blacked out; entering PiP clears a blackout in code (`RemoteScreen.kt`, `inPictureInPicture`).
      (2026-10-23, S1: Home during a blackout left the app with the blackout kept, as it should.)
- [ ] Pocket test: with overlay on, phone in pocket should not send spurious input (touch consumed by overlay). (where: by hand)

## 15. Language / i18n (Tier-3 #17 remainder)

- [x] Settings has a Language row on Android 13+ (absent on 12), opening Android's per-app language picker. (Seen 2026-10-23 on the OPPO against a throwaway server (S1).)
- [x] Switch system per-app language to Español / தமிழ் → all screens, sheets, FGS notification text localize (telemetry units, key labels, window titles stay as-is by policy). (seen 2026-10-23 on the OPPO against a throwaway server (S1) with `cmd locale set-app-locales`: the session's chrome and menu in Spanish and in Tamil; the notification's words not read.)
- [x] Mid-session language change (if the OS applies live) → no crash; format strings (`%1$s`) render correctly in both locales. (seen 2026-10-23 on the OPPO against a throwaway server (S1): Spanish then Tamil during a session, no crash and no raw placeholders; Android recreates the activity, so the session reconnects.)

## 16. Long-run / hygiene

- [x] 1 h continuous session: memory stable, the session up at the end (2026-10-24, the OPPO over the internet road against a throwaway server, the runner's `w1-hour`: the app's memory 161.0 → 161.4 MB over 60 minutes, at most 164.2, the session up at the end.)
- [ ] The same across resolution changes (no decoder leak), fps steady, no watchdog kills. (2026-10-25, the runner's `ae2-crops`: twelve changes of size in one session (1395x786 and 2560x1440, six rounds) with no redial, the app's memory level 175.9 → 186.3 MB, at most 186.7; three runs. The frame rate was not read: the HUD's numbers need Show connection details on the phone.)
- [x] A crop never ends the session. One run of `ae2-crops` in six (2026-10-25) saw the session close right after a crop's first rebuild (1394 wide; the second, at 1395, never came) and the phone redial; the phone's log had rotated past it, so the cause is unknown. Each crop also rebuilds the encoder twice (1394, then 1395 wide), about 0.4 s each. *Fixed (AI3, #374): the capture dropped a window's first frame for coming within a frame period of the monitor's last, and a still window sent no other, so the phone heard nothing for 10 s; now an early frame waits its turn. With it, 6 runs of 6 passed. One rebuild per crop since AI1, #373; the same drop fixed in the DXGI backend, AI4, #375.*
- [x] Force-stop app mid-stream: desktop auto-releases privacy within TTL, encoder sessions stop (no server-side zombie capture), siren/WoL relays unaffected. (seen 2026-10-23 on the OPPO against a throwaway server (S1): the session ended on the computer 30.9 s after a force-stop (the 30 s unheard rule), no live session and no ffmpeg left; privacy was not on.)
- [x] Airplane-mode mid-session then back: clean Down chip + retry, no ANR. (seen 2026-10-23 on the OPPO against a throwaway server (S1): "Connecting…" during 20 s of airplane mode, a new session 4.1 s after it went off, the picture at 9.8 s, no ANR or crash.)

## 17. Foldable / tablet dual-pane (R4 E6)

- [ ] On a foldable folded to half-open (vertical hinge): session auto-splits — desktop video on one pane, trackpad + shortcut dock on the other; tapping/dragging the dock moves the desktop cursor and the finger never covers the video.
- [ ] Rotate to a horizontal hinge → split flips to top/bottom (stream top, dock bottom) and vice-versa.
- [ ] Tablet / unfolded (≥600 dp, no hinge): dual-pane auto-engages with a left|right split.
- [ ] "Pane: auto/dual/single" button cycles the override; forcing Single on a foldable restores the old full-screen layout; forcing Dual on a phone splits it.
- [ ] Toggling panes mid-session does NOT drop the stream (the decode session survives the fold — check fps continuity / no re-Keyframe gap).
- [ ] Direct-touch taps on the video pane still hit the right desktop point (letterbox/aspect-fit recompute for the half-width pane). (where: owner)
- [ ] Shortcut bar appears once — in the dock while dual, under the stream while single.
- [ ] Blackout still covers BOTH panes (whole window), not just the stream.
- [ ] es/ta: "Pane:" and trackpad hint localize.

## 18. Login autostart + tailnet access (server-side)

- [ ] Reboot the desktop → after reaching the Hyprland session, `systemctl --user status pocketdesk` is active without any manual start (WantedBy=graphical-session.target). (where: linux)
- [ ] Phone reconnects to the auto-started server WITHOUT re-pairing (QUIC identity persisted in `~/.local/state/pocketdesk/certs/`; a regenerated cert would trip the TOFU pin). (where: linux)
- [ ] Log out / session end → service stops with the session (PartOf), never orphaned. (where: linux)
- [ ] `install.sh` run → unit lands in `~/.config/systemd/user/`, ExecStart points at the real prefix path, prompt enables it; `--status` reports the user unit; `--uninstall` removes it and any legacy `/etc/systemd/system` unit. (where: linux)
- [x] Phone on the tailnet (not on the LAN): connect to the laptop's 100.x Tailscale IP → pairing + video + taps work end to end. **(verified 2026-09-22: oppo-reno8-5g <phone tailnet IP> → <laptop tailnet IP>)**
- [x] Re-verified 2026-09-22 with the new home screen over tailnet (both ends on the same Wi-Fi, Tailscale direct path): auto-connect + card tap → session Up (26 fps, 1.4 Mbit/s, rtt ~55 ms, drops 0), server log shows the peer as <phone tailnet IP>, and a direct-touch tap moved the desktop cursor (1142,37 → 768,390). Note: Alt+Tab and PrtSc shortcut buttons are no-ops on THIS desktop (no Hyprland binding for either) — not transport failures.
- [ ] Phone on cellular (Wi-Fi off): same connect via tailnet IP works (Tailscale traverses NAT). Then iroh WAN fallback (`## 11`) with the two machines on genuinely different networks — still pending (phone cellular was OUT_OF_SERVICE at test time). (where: away)

## 19. Session chrome refresh (action disc + quick-settings sheet + fading HUD)

Verified on OPPO CPH2359 over the tailnet, 2026-09-22:

- [x] Bottom bar is just the mode chip + floating disc over the ShortcutBar; the disc opens the grouped "Session settings" sheet (Input / Streaming / Desktop).
- [x] Toggle rows act live (clipboard sync, view-only flipped in place; pairing row reads "Paired").
- [x] Value rows cycle (input mode chip + sheet row stay in sync: Direct touch ↔ Trackpad).
- [x] Rows that lead elsewhere dismiss the sheet first (pairing sheet opened over a closed quick sheet).
- [x] Stats/workspace HUD fades to a ghost ~6 s after the link is Up; tapping it restores full alpha and re-arms the timer.
- [x] Exit pill + WAN badge untouched at the top; PiP still hides all chrome.
- [x] es/ta strings shipped with the new keys (31 each, locales in sync).

## 20. Saved-computers home screen (multi-host store + add-computer form)

Verified on OPPO CPH2359 over the tailnet, 2026-09-22:

- [x] Legacy single-host prefs migrate into a card list — "Your saved computers" shows the
      saved desktop with Auto badge, Paired ✓ label, and "stream 4716 · control 1716" subtitle.
- [x] Tapping the card connects straight into a live session (pairing reused, no re-prompt); Exit returns to home.
- [x] "Add computer" opens the form: address field, collapsed "Advanced (ports, wake-on-LAN)" expander
      (auto-expanded when editing a known host), auto-connect switch, disabled Connect until address is valid, Cancel back to home.
- [x] Advanced expander reveals streaming/control port fields (pre-filled defaults) + WoL MAC field with a Send button gated on a valid MAC.
- [x] Remove shows a confirm dialog naming the address and warning that pairing + cached settings go with it; Cancel dismisses without deleting.
- [ ] Wake row + wake status appear only for hosts the phone knows how to wake (since N6: what the computer announced, or a typed MAC; see §42).
- [x] Multi-host scenarios (second card ordering, remove-then-readd). (2026-10-25, the runner's `aq2-second-card` on the OPPO, three saved computers with it: a computer added through Home's form at 192.0.2.10, where nothing answers, came first as the last one asked for; removed through its menu, asked first, the saved computers were exactly as they were, twice in a run, three runs, auto-connect untouched. Found there first: with the phone linked to another computer, the new card took that computer's name and WAN identity and its session streamed from it, fixed by AQ3.)
- [ ] es/ta strings for the new keys.

## 21. UI polish pass C1–C9 (design system, immersive session, error UX, settings)

First device run (OPPO CPH2359, tailnet) found the icon layer broken on-device —
the runtime-Canvas glyph set drew as a corner dot — so `ui/Icons.kt` now wraps
`res/drawable/ic_ll_*.xml` VectorDrawables tinted through `painterResource`.
`StatsHud` is a `FlowRow`, session chrome is inset-aware and self-describing, and
any touch on the stream summons faded chrome (`PointerEventPass.Initial`, so the
tap still reaches the desktop). The app also gained an adaptive launcher icon.
Items below are re-scored against that build.

- [ ] Theme follows the system light/dark; toggling the mode in Settings restyles every screen immediately (no restart), and the window background matches on cold start (no white flash in dark mode or vice-versa). *Partly verified: the Dark radio restyles Settings immediately; cold-start window colour not yet checked.* *2026-10-25: the system's dark mode turned on and off restyled Home at once, the app's process running throughout (the runner's `ag3-theme`, Home's background 251 light, 20 dark). The cold-start window colour is still to see.*
- [ ] Material You: on Android 12+ the palette follows the wallpaper when "Dynamic color" is on; turning it off falls back to the brand palette.
- [ ] Edge-to-edge on all screens: content draws behind the status/nav bars and no control is unreachable under them.
- [x] Session goes immersive: system bars hidden on entry, transient reveal on swipe, restored on Exit/PiP/blackout. After a reveal, video keeps rendering and taps stay pixel-exact (decoder-stability spot check — if OPPO shows black bars, see plan C2 fallback). *Bars stay hidden through a full session and the decoder is stable across reveal + exit; the black-bars fallback never had to be used.*
- [x] Chrome auto-hide: HUD, mode chip, action disc, WAN badge and shortcut bar fade out together ~6 s after link-up; **any tap on the stream** reveals them (the old invisible 30 dp edge strips are gone — they made the menu unreachable); the timer is suspended while any sheet is open; dual-pane keeps chrome forced-visible.
- [x] Error UX: a disconnect shows a centered card with a humanized reason (never a raw Rust string), Retry reconnects, "Re-pair" opens the pairing sheet when the error suggests it; session messages arrive as snackbars, not toasts. *Verified by `pocketdesk kick all` mid-session: warning-glyph card, "The video link dropped…" copy, and Retry rebuilt the QUIC session end to end (server log + live HUD). The re-pair branch and the snackbar paths were not exercised by this run.*
- [x] Silent-link detection: a desktop-side teardown (kick, sleep, Wi-Fi loss) reaches the error card within ~10 s instead of leaving the last frame frozen under normal-looking chrome — the bridge never reports the drop, so the decoder's frame gap is the liveness signal. *Verified on-device: card appeared 10 s after the kick and a healthy session ran 25 s+ past the threshold without a false positive.*
- [x] Monitor/window/audio pickers: selected row shows the radio + primary-colored headline (the old "· current" suffix is gone); quick-settings rows have leading icons and chevron/switch affordances. *Session-settings sheet confirmed on-device with real vector glyphs, chevrons and switches; the window picker opens and renders its empty state (list contents need a desktop with windows on the streamed workspace).*
- [ ] Pairing sheet: progress bar while waiting, PIN field auto-focused, eye toggle honors the reveal-PIN pref, animated check on success then auto-dismiss.
- [x] Settings screen: every pref persists across an app restart; Language opens the system per-app language picker; About shows version + core version; Home gear opens it, back arrow returns with a slide/fade transition between routes. *Gear + back arrow verified on-device; the Dark radio survived an app relaunch.*
- [ ] Home empty state (no saved computers) shows the desktop glyph + explainer.
- [x] Siren: a desktop-pushed `kdeconnect.findmydevice {ring:true}` rings the alarm AND shows the full-screen dialog; Silence stops both; the 30 s auto-stop still works if the dialog is ignored. (2026-10-25, the runner's `aq1-siren` on the OPPO in a session, the phone's input off, against a throwaway server: the dialog with the alarm playing; Silence stopped both within 2.7 to 3.3 s as read, the session still up; a second ring a minute later, left alone, stopped both 34 and 35 s after it was asked (the ring's 30 s, from when the phone took it), in two runs, and the alarm's volume was read back at its 6. The alarm plays from SystemUI's process, not the app's.)
- [x] Clipboard sheet: Clear asks for confirmation first (Cancel keeps history): its dialog asked, and after Cancel the 7 rows stayed (`y3-history-sheet`, the OPPO, 2026-10-24).
- [x] Clipboard sheet: the empty state has the clipboard icon. (2026-10-25, the runner's `al3-empty-history` on the OPPO: seven entries cleared through the sheet's Clear, confirmed; then the empty words, no Clear, and the clipboard icon above them, read from a screenshot and seen once by eye.)
- [ ] Haptics: tick on the action disc, mode toggle, picker picks and shortcut chips; heavier buzz on Exit; the Settings haptics switch mutes all of them live. (2026-10-25: not readable on the OPPO. Android's vibrator record showed the session's menu tap as Pocketdesk's CLICK at strength 0 and nothing for the mode toggle or Exit: the phone is on silent, which mutes touch vibrations. By hand, with the ringer on.)
- [x] Session notification: real Pocketdesk status-bar icon (not the stock eye), Disconnect + Lock-desktop actions carry icons, channel shows its description in system settings. *`dumpsys notification` shows the custom `ic_stat_pocketdesk` resource and both actions; the shade renders the monitor glyph plus power/lock action icons, and the session channel carries its description.*
- [x] Shortcut bar: System | Workspaces groups with divider and localized captions; chips are ≥32 dp touch targets; key caps stay literal in es/ta. *Now a dark floating pill, legible over video; scrolls horizontally past PrtSc.*
- [x] es/ta: every Phase-C string shipped (locales in sync at 161 each).
- [x] Launcher icon: the adaptive icon renders on ColorOS (phone-into-monitor mark on the brand pine plate) instead of the default Android blob. Needed filled paths in a scaled group — stroke-only adaptive foregrounds are not rasterized there.

## 22. Owed verification register

**This was the gate for Phase 0, item 5 — nothing
in Phase 1 starts while it is open.** Every line here is a claim the repo already makes that no human has
checked on hardware, or a fix that shipped without its one confirming observation.

### 22.1 One shared control connection (`90d92df`) — the check that was promised

The reconnect storm fix is merged but its own success criterion needs the phone attached:

```bash
adb devices -l                                   # confirm the phone is really there
journalctl --user -u pocketdesk.service -f > /tmp/ll-journal.log &
# then, on the phone: connect, open the quick-settings sheet, the monitor picker,
# the window picker, the clipboard sheet, watch the battery row for 60 s, exit.
kill %1
```

- [x] **PASS condition:** zero `Incoming v1 TCP connection from` lines (`server/src/service.rs:330`) after
      the single session-establishing connection, while every sheet and picker above is used. (seen 2026-10-23 on the OPPO against a throwaway server (S1): none while the session menu, the quality choices, the screen switches, the clipboard history and the view-only switch were used; the six in the log came at the server's start (the background links) and after a force-stop.)
- [x] **PASS condition:** zero `Broken pipe (os error 32)` WARN lines from any plugin. (seen 2026-10-23 on the OPPO against a throwaway server (S1): none.)
- [x] **PASS condition:** zero `Kicked 1 stale session(s) due to reconnect storm` from the server. (seen 2026-10-23 on the OPPO against a throwaway server (S1): none.)
- [x] Battery/clipboard/monitors/window rows still show *current* values — the shared connection must not
      trade churn for staleness. (seen 2026-10-23 on the OPPO against a throwaway server (S1): the computer's battery read 59 % on AC on the chip and in Windows; the clipboard rows current.)
- [ ] The no-session fallback still works: a query issued with **no** live control session must take the
      `oneshot_request` branch (`android/bridge/src/api.rs`, reached from `control_request` when
      `CONTROL_WRITER` is `None`) and still get an answer — that is the path that runs before you connect,
      and it is the one the rewrite could plausibly have broken. Same for `push_packet`'s dedicated socket.

### 22.2 Phase-C items §21 never re-scored on the current build

Eight boxes in §21 are still open and cannot be closed by reading code (they are visual, timing or OEM
behaviour): theme-follows-system + cold-start window colour (358), Material You on/off (359), edge-to-edge
with nothing unreachable (360), the pairing sheet's progress/focus/eye/animated-check flow (366), the Home
empty state (368), the siren full-screen dialog plus its 30 s auto-stop (369), clipboard Clear confirmation
(370), and the haptics matrix with the Settings mute (371).

### 22.3 §20 host-management gaps

Wake row only appearing for a host with a saved WoL MAC (345), and multi-host ordering + remove-then-readd
with es/ta strings (346).

### 22.4 The only genuinely-blocking device test: WAN over cellular (§11)

The iroh path has never been exercised across a real network boundary — the phone's cellular radio was
OUT_OF_SERVICE during the tailnet run, so §1.1 LAN and §18 tailnet results do **not** stand in for it.
Requires: SIM with data, Wi-Fi off, and an external relay or a second network.

### 22.5 Claims that must not be tested because the feature is absent

Recording these here so a tester does not file them as regressions:

- Desktop audio audible on the phone — there is no client-side Opus player (`receiveAudio` has no caller);
  see the old backlog's **2791-2800** (in git history), and §10's "audio" results are about *control* (routing/volume), not playout.
- The file browser — the server plugin answers, `listRemoteFiles` has zero call sites (**2874**).
- Notification per-app channels, grouping, icons, privacy modes (**2898-2905**).
- Every K (virtual display) and L (macro/automation) behaviour — neither subsystem exists.

## 23. Windows host (W1: core remote desktop)

Built on `feat/windows-host`. Box-verified
already (ignored live tests, run on the dev PC): WGC and DXGI capture of a real frame, NVENC/AMF/MF
encodes, SendInput pointer + key, clipboard round-trip, toast accepted.

**Run 1, 2026-10-12 — Galaxy A71 (Android 16), USB.** Control rode `adb reverse tcp:1716`; video
fell back to iroh with a punched direct path, because `adb reverse` carries no UDP. The PC has an
RTX 3060 + Radeon iGPU, a 2560x1080 primary, and a 2048x1152 screen to its right, 91 px higher.
Defects found here are fixed on the branch:
- black video (sidecar NAL framing; the bridge dropping the last frame of every batch);
- frame-number gaps after a decoder reconfigure;
- taps landing on the wrong monitor;
- four encoder rebuilds at every session start;
- HUD window titles lost to connection eviction;
- silent bridge logs.

Video ran at 11–16 fps (damage-driven) with e2e 13–19 ms and rtt ~10 ms. Ticked items below were seen
on this run; the rest are still open.

Setup, once, from the build output or the release zip:

```powershell
pocketdesk.exe install          # logon task + firewall rules (one UAC prompt)
pocketdesk.exe pair             # prints the PIN (also shown as a toast)
Get-Content "$env:LOCALAPPDATA\pocketdesk\logs\pocketdesk.*.log" -Wait
```

- [ ] `install` registers the "Pocketdesk" task (Task Scheduler → Task Scheduler Library), removes a
      "Pocketdesk" task an older version left, starts the server with no console window, and the log
      file appears under `%LOCALAPPDATA%\pocketdesk\logs`. (where: owner) (it registers a logon task and firewall rules on this computer.)
- [x] A toast "Pocketdesk pairing PIN: …" appears when the phone asks for a PIN ("Show a PIN on the computer"); `pocketdesk pair` prints its PIN instead. (Seen 2026-10-23 on the OPPO against a throwaway server (S2), read back from Windows' notification store.)
- [ ] The phone discovers the PC on the LAN (mDNS) and over the tailnet, pairs with the PIN, and the
      log shows `Auto-detect: selecting NVENC` (or AMF/MF on other GPUs) and `WGC capture started`.
      *Run 1: PIN pairing, NVENC and WGC confirmed over USB; LAN discovery and the tailnet not tried.*
- [ ] Video is live and correct-sized on a 100 % and on a scaled (125/150 %) display; a still desktop
      keeps the HUD alive (the 1 s heartbeat) instead of tripping the 10 s dead-link card.
      *Run 1: live, correct-sized and kept alive on a still desktop at 100 %; no scaled display tried.*
- [ ] Taps land on the pixel tapped, on a single monitor and on a secondary monitor left of/above the
      primary (negative origin); drag selects text.
      *Run 1: taps on the primary land within 5 px with a second screen present, after the monitor →
      layout remap fix. Not tried: streaming the secondary, a monitor left of the primary, drag.*
- [ ] Phone keyboard: letters, digits, shifted capitals, arrows, F11/F12, Esc, Super (Start menu),
      right Alt, the app key; pasted text with capitals, accents and an emoji arrives intact.
      *Run 1: not testable — the app has no in-session keyboard (only the shortcut bar); Android
      Share sends text to the PC clipboard, not as typing.*
- [ ] Focusing an elevated window (e.g. an admin terminal) makes input stop with one `Windows refused
      injected input` log line, and input resumes on returning to a normal window.
- [x] Clipboard: phone → PC paste and PC → phone fetch, including non-ASCII. *Run 1: `äö` and `✓`
      both ways; the PC → phone direction is the app's 4 s poll.*
- [ ] Window crop: picking a window streams just that window; closing it returns to the monitor. (where: owner) (closing a window on this computer's desktop.)
- [ ] A UAC prompt or Win+L mid-session: the stream recovers after it is dismissed (WGC keeps running; on
      the DXGI fallback, access-lost rebuilds the duplication).
- [ ] `pocketdesk stop` from a second terminal stops the server cleanly (the log ends with
      `Shutdown requested (pocketdesk stop)`) — run this yourself; it is deliberately not automated.
- [ ] `uninstall` removes the task and the firewall rules (`wf.msc`), including the "Pocketdesk" ones an
      older version created.

## 24. Desktop app and QR pairing

Built on `feat/pocketdesk`.

Checked on the Windows dev box on 2026-10-12, with a Galaxy A71 over USB:
- every tab against the live server;
- close-to-tray, and re-show by relaunch;
- the on-screen QR decodes to the expected `pocketdesk://pair` URI;
- that URI opened on the phone, with the address swapped to `127.0.0.1` for `adb reverse`: it confirms, pins, pairs and streams;
- a forged link with a different fingerprint is refused;
- the scanner screen opens, with camera preview and analysis running.

- [ ] Scan the desktop app's code with the phone camera (the USB-tethered phone could not see the screen).
- [ ] Pair over the LAN address in the code, not `adb reverse` (needs the firewall rules `install` adds).
- [ ] Tray: Open, Pair a phone… and Quit from the tray menu itself.
- [ ] Start / Restart server from the app, on Windows (logon task) and Linux (systemd user unit). This is not run on the developer box by rule.
- [ ] Linux: the app builds and opens on X11 and on Wayland (Hyprland), closing it quits (no tray on Linux yet), and the single-instance socket works.
- [ ] Linux tray: build one on a GTK thread (or tray-icon's ksni backend) and verify it with a StatusNotifier host, e.g. waybar's tray.
- [ ] Settings → Save on a real `config.toml` keeps comments; the server picks the change up after Restart. (where: by hand)
- [ ] One code, one phone: after a phone pairs, the Pair tab shows "Paired ✓" and no code, `pairing.pin` is gone, and "Pair another phone" shows a new code. Five wrong PINs typed on a phone make the tab say the code no longer works, and no new code appears until it is clicked. Leaving the tab, closing the window to the tray, and Quit each delete `pairing.pin`.
- [ ] Leaving the scanner (after a scan, and with Back) turns the camera off: the privacy indicator goes out at once, not when the app closes.
- [x] Open a `pocketdesk://pair` link, finish or dismiss it, then switch dark mode or reopen the app from Recents: the pairing is not offered again. Switching theme while the confirmation is showing keeps it. (seen 2026-10-23 on the OPPO against a throwaway server (S2): a link dismissed with Cancel was not offered again after dark mode, light mode or reopening the app; finishing one not tried.)

## 25. Desktop sound on the phone

Built on `feat/phone-audio`. First device run 2026-10-12 (Galaxy A71, USB + WAN direct, Windows desktop): the player starts with the session, a 48 kHz stereo low-latency AudioTrack reaches `state:started` and keeps serving frames, and the playout thread averages about 4.5 % of a core. Not yet checked by ear: how it sounds, latency, the switch, focus with calls or other apps, PiP.

- [ ] Play a video on the desktop during a session: the phone plays its sound, and the lag behind the picture is small (≤ ~150 ms by ear).
- [ ] Quick Settings → "Desktop sound on this phone" off: silent at once; on again: sound returns within a second. The setting survives an app restart. (where: by hand)
- [ ] Pause the desktop's audio for 10 s, then resume: it comes back cleanly, without a crackle burst. (where: by hand)
- [ ] Connecting to a silent desktop does not pause music already playing on the phone; the first desktop sound takes over. "Silent" means no app holds an audio stream open: Windows loopback sends packets of silence while any app has one (seen on the A71 with a browser open: focus was taken 1.3 s after connect), and focus follows packets, not loudness. (where: by hand)
- [ ] A phone call silences it, and it resumes after the call. Another app starting music takes over for good: switching "Desktop sound on this phone" off and on brings it back. (where: sim)
- [ ] In PiP the sound continues; Exit stops it. (where: by hand)
- [ ] With sound on, the HUD's fps and delay match a session with it off, and taps on a still desktop feel immediate (the bridge lock fixes).
- [x] `adb logcat -s DesktopAudio`: "desktop sound started", and on exit "stopped (N underruns)" with N small (C6, A71 → Windows: 1 underrun in 34 s).

## 26. In-session keyboard

Built on `feat/in-session-keyboard`.

Checked on 2026-10-12 with the Galaxy A71 against the Windows host, typing into Notepad and reading it back through UI Automation:
- the keyboard chip opens Gboard on the sink view, the bar docks above it, and the stream sits above both;
- "hello world" typed letter by letter;
- autocorrect "teh" → "the";
- `adb shell input text` (the hardware-key path);
- Ctrl latched + a, then Ctrl latched + c (the Windows clipboard held the text);
- Backspace, then Home, Del and End from the bar;
- Back closes the keyboard and the bar.

- [ ] Swipe typing a word, and voice dictation.
- [ ] Double-tap locks a modifier (ringed cap); typing with Shift locked; Super latched + a key on the desktop.
- [ ] F-keys, Ins, PgUp/PgDn and Ctrl+Alt+Del from the bar.
- [ ] A Bluetooth or USB keyboard on the phone: letters, Ctrl shortcuts, arrows, F-keys.
- [ ] Landscape: the stream stays visible above the keyboard, and zoom survives opening and closing it. (where: by hand)
- [ ] Linux host: ASCII typing over uinput. Non-ASCII and capitals in pasted text wait for roadmap F4 (2666).
- [ ] Tamil and Spanish keyboards: composing text arrives whole and correct.

## 27. Trackpad gestures

Built on `feat/trackpad-gestures`; roadmap F3.

Checked on 2026-10-12 with the Galaxy A71 against Windows:
- a tap, then a touch-and-drag within 300 ms, selected a Notepad line (read back through UI Automation);
- the Trackpad speed slider renders and its value survives an app restart.

- [ ] Two-finger tap opens a context menu; three-finger tap middle-clicks (e.g. opens a link in a new tab). adb cannot make multi-finger touches.
- [ ] Two fingers resting without moving do not right-click, and a two-finger tap does not scroll the page first.
- [ ] Two-finger scroll is natural (content follows the fingers, like the phone itself) at about one wheel notch per 24 dp, in both axes. Unverified: adb cannot make two-finger touches. (where: by hand)
- [ ] Tap, then drag a window by its title bar: it moves without maximising (no double click), and a tap left alone clicks about 0.3 s later.
- [ ] A drag survives a second finger landing mid-drag and releases on lift.
- [ ] Speed 0.5× and 3× feel right; slow movements still move the cursor.
- [ ] The foldable dock's trackpad behaves the same.

## 28. Permissions and session approval

Built on `feat/approvals-permissions`.

Checked on 2026-10-13 with the Galaxy A71 against the Windows host, over WAN. The answers were written to the answer file directly, as the app does; nobody clicked the prompt.
- with "ask first" on, the session is held, the request appears in `pending_approvals.json`, and the phone shows "waiting for the picture" with the approval hint;
- Allow once: the server logs `AllowOnce`, the list empties, and video plays;
- Deny: the server logs `Deny` and `session refused`, and the phone shows the refusal card about 2 s later.

- [ ] The desktop app's prompt: it comes to the front (also from the tray) and its three buttons work; Always allow turns the switch off.
- [x] No answer: the phone shows the refusal about 30 s after connecting. (seen 2026-10-23 on the OPPO against a throwaway server (S2): held for the owner 9.5 s after Connect, the phone's refusal some 30 s later.)
- [x] Clipboard off: the phone's clipboard actions are dropped (`Denied … clipboard` in the log); turn it on again and they work at once. Four denied in a session's first 15 s, none in the 15 s after it was turned on (2026-10-19, A71, throwaway server, by script).
- [x] Running commands: the phone app has no way left to send a raw command (named tasks replaced it, ecosystem 1c; `executeRemoteCommand` has no caller), and the computer's Exec permission stays off by default. (Checked in the code 2026-10-23, S2.)
- [x] Control off: the session is view-only and the phone cannot lift it. Sound off: no desktop sound. Microphone off: the mic does not reach the desktop. (seen 2026-10-23 on the OPPO against a throwaway server (S2): with Sound and Microphone off the gate logged `allow_audio=false allow_mic=false` and no audio task started; control off is how these checks run (`force_view_only=true`).)
- [x] Turning sound off during a session ends it (within 1 s: "This computer's owner changed what this phone may do…"); Retry starts one with the change (2026-10-19, A71, throwaway server, by script). Found there: on a phone held upright the card was squeezed into the stream's quarter of the screen, its text over the chrome and Retry under the mode chip; fixed, the card now has the whole screen while the session is not up.
- [x] Turning control or the microphone off during a session ends that session. Removing a connected phone ends its session. (seen 2026-10-23 on the OPPO against a throwaway server (S2): the microphone off ended the session in 1.0 s, the sound in 0.5 s, an unpair in 1.1 s; the phone's words for the unpair fixed in #282.)
- [ ] LAN session: the prompt shows the phone's address. (where: owner) (a throwaway server's LAN port needs a firewall rule.)

## 29. Computers on this network

Built on `feat/lan-discovery`.

Checked on 2026-10-13 with the Galaxy A71 and the Windows host on the same Wi-Fi:
- the host was found in about 0.2 s at its LAN address (not its link-local or Tailscale address), and the saved card, saved as 127.0.0.1 and matched by its paired id, showed "On this network";
- a second computer announced from the PC (a test announcer) appeared under "On this network" with its name, and disappeared live when it withdrew;
- with `lan_discovery = false` the server logs that it is not announced;
- a server killed without a goodbye left a cached record, which the phone did not list ("announced but does not answer").

- the confirmation dialog shows on tapping a found computer; Connect opened a session at the found address with the PIN sheet (an address not paired before), and nothing was saved;
- an announcement that lists an unreachable address first is listed at its reachable one.

- [ ] Pair from a found computer: after the PIN it appears under Saved. (where: owner) (a throwaway server's LAN port needs a firewall rule.)
- [ ] Android 17: the local-network permission prompt at first launch; LAN sessions and discovery work after Allow, and time out after Deny.
- [ ] Two computers on the network, both listed.
- [ ] Android 13 or older (the resolveService path).
- [ ] The Linux host is found (Avahi may also be announcing on the same host).
- [ ] "Visible on this network" in the desktop app, then a server restart: the computer no longer appears. (where: owner) (a throwaway server's LAN port needs a firewall rule.)

## 30. Sending files to the desktop

Built on `feat/file-transfer`.

Checked on 2026-10-13 with the Galaxy A71 against the Windows host over LAN. Three shares 1.5 s apart (2, 4 and 6 MB) queued on the phone and were sent one after another with matching MD5s. All three appear in `received.json`, and the final notification read "Sent 3 files to …".

- [ ] Share several photos at once from the gallery (`SEND_MULTIPLE`): "Sending 2 of 3" with a moving progress bar. (where: by hand) (`am` takes no list of URIs, so only the gallery sends one.)
- [ ] The desktop shows a notification per file; the app's Files tab lists them, and Show in folder selects each in Explorer. (where: by hand)
- [ ] A file from a provider without a size (e.g. some cloud apps) still arrives (copied first). (where: by hand)
- [x] A share with the desktop off or unreachable ends in "Not sent: 1 of 1" with the reason. (seen 2026-10-23 on the OPPO against a throwaway server (S2): "Sending 1 of 1", then after its retries (123 s) "Not sent: 1 of 1" and "Nothing answered on that address. Is Pocketdesk running on the computer?".)
- [x] A non-English file name arrives with its name. (seen 2026-10-23 on the OPPO against a throwaway server (S2) by the internet road: `s2-தமிழ்-ñ.jpg` arrived under that name.)

## 31. Files both ways and the remote browser

Built on `feat/file-transfer-b`.

Checked on 2026-10-13 with the Galaxy A71 against the Windows host over LAN:
- Files on this computer opened at the home folder, a sub-folder, and Up.
- A 3 MB download landed in Downloads/Pocketdesk with a matching MD5, and the notification read "Saved 1 file to Downloads".
- A 5 MB file sent from the desktop was fetched within a second, with a matching MD5, and `sent.json` read `sent`.
- A phone-to-desktop share waited for the desktop's save and read "Sent 1 file to …", with a matching MD5.
- The desktop app's Files tab and "Choose file…" worked: the file arrived as `name (1).bin` beside the earlier copy, with a matching MD5.
- A send to a phone that had left its session read "failed: phone not connected".

- [x] Browse the home folder from a session; folders open, Up returns, hidden files are left out.
- [x] Download a file; MD5 matches; it is in Downloads/Pocketdesk.
- [x] Send from the desktop app with "Choose file…"; the phone saves it; the Sent list says so.
- [x] A phone-to-desktop share reports sent only after the desktop saved it.
- [ ] A download refused by the desktop (an offer used twice) fails at once with its reason, not after 30 s.
- [x] Send a file while a large share from the phone is running; the offer still arrives. (2026-10-24, the OPPO by the internet road against a throwaway server, the runner's `ac3-send-during-share`, with the phone's Files from the computer in the background on: the phone moves one file at a time, so 1 MiB sent at 10 % of a 1 GiB share waited behind it and arrived after it, 103 s, both MD5s matching. Behind a 7.5 GiB share, 10.6 minutes, the offer lapsed first, "the phone did not take it in time": fixed by AC7, #347, a busy phone's offers waiting while it moves a file; the same run then passed.)
- [ ] A phone that has no room declines an offer; the desktop's Sent list says "the phone declined it: …". (where: other hardware) (a full phone.)
- [ ] Drag a file onto the desktop app's window; it is sent to the chosen phone (Windows, and X11 Linux).
- [ ] Tap "Saved … to Downloads"; the file opens. (where: by hand) (ColorOS's shade shows uiautomator no notification of the app.)
- [ ] Download into a phone without enough space: "Not enough space for …", nothing left in Downloads. (where: other hardware) (a full phone.)
- [x] A file over 1 GB to the phone, in a session by the internet road: 1,074,790,400 bytes in 130 s (7.9 MiB/s), the MD5 the same on the phone (the runner's `w3-big-send`, the OPPO, 2026-10-24).
- [x] A send to the phone cut at 30 % (the server stopped, as a dropped network would): no file published under its name, only its partial (`w3-cut-send`, the OPPO, 2026-10-24).
- [x] A file over 1 GB from the phone to the computer by the internet road: 1,074,790,400 bytes, the same MD5, 114 s on the wire (`x1-share-big`, shared by the fixture app, the OPPO, 2026-10-24; a first run's share never reached the computer and was not explained, the next passed).
- [ ] A download from the phone cut halfway (Wi-Fi off).
- [ ] Android 8 or 9: the file lands in the app's own Downloads/Pocketdesk.
- [ ] The Linux host: browse `/home/<user>`, a download, and a send from the app's path field.

## 32. Switching screens

Built on `fix/monitor-switch`. Checked on 2026-10-13 with the Galaxy A71 against the Windows host (two monitors, 2560×1080 and 2560×1440):
- Quick Settings → Monitor switched both ways six times over LAN, with no crash.
- Three switches made while a WAN connect was still in flight all landed on the last pick. The old build crashed here with "Invalid to call at Released state".
- The chrome's chips 1 and 2 switched the stream each way with one tap.

- [x] Switch screens from the chips and from Quick Settings → Monitor; the picture follows, and the app stays up.
- [x] Switch while the stream is still connecting (WAN); no crash, and the last pick wins.
- [ ] Unplug the monitor being streamed; the session falls back to the primary screen. (where: owner)
- [ ] Three or more monitors: a chip each, in the desktop's order. (where: other hardware)
- [ ] The Linux host (portal or screencopy capture per monitor).

## 33. Shortcuts outside the app (F11)

Built on `feat/shortcuts`. Checked on 2026-10-13 with the Galaxy A71 against the Windows host:
- The launcher lists "Connect to …" for each saved computer, most recent first.
- Firing the "Connect to <desktop LAN IP>" shortcut opened a LAN session.
- A connect request for an address that is not saved opened nothing.
- The Quick Settings tile, added and clicked with `cmd statusbar`, connected to the last computer.
- The widget's provider is registered.

- [x] Long-press the app icon: a "Connect to …" shortcut per saved computer (up to four), opening its session.
- [x] A request for a computer that is not saved opens nothing.
- [x] The Quick Settings tile connects to the last computer; with none saved, it is unavailable.
- [ ] Place the widget on the home screen: it names the last computer and connects on a tap; with none saved it opens the app. (where: by hand)
- [x] A Wake shortcut appears for a computer the phone can wake (§42), and sends the packet. (seen 2026-10-23 on the OPPO against a throwaway server (S2): "Wake Test Desk" among the launcher shortcuts; fired, it sent 12 magic packets, caught by a listener on this computer (the result is a toast uiautomator cannot read).)
- [ ] Pin a shortcut to the home screen; it survives the computer being reconnected, and stops working once the computer is removed. (where: by hand)
- [ ] Android 13 or older: the tile opens the session (the older collapse path).

## 34. Desktop diagnostics (F10)

Built on `feat/diagnostics`. On the Windows box, an ignored test (`cargo test -p pocketdesk-desktop the_checks_run -- --ignored --nocapture`) printed the report: right after a start the port checks wait; once running all six were ok.

- [x] Windows: the six checks against a running server; the "starting" window.
- [ ] Open the Diagnostics tab in the app and click Run checks; Copy debug report pastes the same text.
- [ ] A network Windows files as Public, with only `install`'s rules: the firewall check says no rule lets phones in. (where: owner)
- [ ] A Block rule for pocketdesk: the firewall check reports it. (where: owner)
- [ ] Linux: the checks with the systemd user service (no firewall line).

## 35. Reaching the desktop from outside (N1)

Built on `feat/wan-insight`. On the Windows box, the server recorded UDP working, a NAT that allows direct connections, and no public IPv6, and Diagnostics showed both lines.

- [x] Diagnostics shows "From outside" and "IPv6" from the server's own measurement.
- [ ] **Cellular:** the phone on mobile data, the desktop at home. A session connects, and the HUD says direct or relayed. (Owed: the test phone has no mobile service.)
- [ ] A strict-NAT network (or a hotspot behind carrier NAT): "From outside" warns, and the session rides the relay.
- [ ] A network with public IPv6: the IPv6 line is ok. (where: away)

## 36. Your own relay (N2)

Built on `feat/own-relay`. On the Windows box, a phone WAN session ran through `pocketdesk relay --allow-paired` with `relay_urls` pointing at it, and the record said `wan_relayed`.

- [x] The desktop announces its own relay, and a phone session rides it.
- [x] Diagnostics says "ready through your relay", and "From outside" says "not measured".
- [x] The phone's own home relay is the user's relay too (logcat: `home is now relay http://…:3340/`), and its log names no n0 relay.
- [ ] **From outside:** the phone on mobile data, the relay on the desktop with port 3340 forwarded. A session connects through it.
- [ ] HTTPS mode on a cloud VM with a real name: the certificate is issued, and a session connects with `--allow` ids from `relay-ids`. (where: away)
- [ ] A phone that is not paired (or an id not listed) is refused by the relay. (where: away)

## 37. Found by id on the Mainline DHT (N3)

Built on `feat/dht-lookup`. On the Windows box (n0's relays, `dht_publish` at its default), the server published its record within 4 s of start and read it back a minute later. The A71's cached identity was then made stale on purpose (run-as on its prefs): a closed stream port so LAN fails, an n0 relay the desktop is not on, and no direct addresses. Input was off for the phone throughout.

- [x] Diagnostics' "Found by id" says "published on the Mainline DHT and read back (relay address only)".
- [x] The phone caches the identity with `"dht": true` after a LAN session.
- [x] Control: with `"dht": false`, the stale identity's WAN dial times out (`DHT lookup: false`).
- [x] With `"dht": true`, the same dial finds the record on the DHT in about 4 s, connects through the relay it names, and upgrades to a direct path (logcat: `discovered endpoint info`, `WAN session upgraded from relay to direct path`).
- [x] After the review fixes, again: the server's DHT joined in the background on a random port, published within 4 s and read back a minute later; the stale-identity dial found the record in about 3 s and went direct.
- [ ] **From outside:** the phone on mobile data after the desktop's home relay or public IP changed. A session connects, found by the DHT.
- [ ] `dht_publish = "addresses"` on a network with public IPv6: the record holds the IPv6 address, and a phone with relays unreachable connects directly. (where: away)
- [ ] A network that blocks outgoing UDP: "Found by id" warns that it was not read back. (where: away)
- [ ] The Linux host: the same checks.

## 38. Image clipboard (N4)

Built on `feat/image-clipboard`. On the Windows box, a 320x200 image put on the clipboard as a bitmap only (no PNG format, so the DIB path ran) reached the A71 during a LAN session as a 1196-byte PNG whose SHA-256 matched its id. It was sent once and never sent back. Input was off for the phone throughout.

- [x] Desktop → phone: a bitmap copied on Windows lands in the phone's clipboard cache as PNG, once.
- [x] After the review fixes: the same image copied again after some text in between moves again, and the text reaches the phone; only the newest image stays in the cache.
- [x] The Windows clipboard holds a PNG as both "PNG" and `CF_DIB`, and reads it back byte for byte (live ignored test).
- [ ] Desktop → phone: paste it into a chat app on the phone. (The A71 has no app that pastes images.) (where: by hand)
- [ ] Phone → desktop: copy a photo on the phone and paste it into Paint. (The A71 has no app that copies an image: no browser.) (where: by hand)
- [ ] With the phone's Clipboard permission off, neither direction moves. (2026-10-25, the computer-to-phone half: the runner's `ad1-image-copy` with the permission off on a throwaway server: the image not told to the OPPO, not fetched. The phone-to-computer half is owed: the OPPO cannot copy an image by script.)
- [ ] A 4K screenshot (over 10 MB): it moves, and the phone's memory holds.
- [ ] The Linux host (`wl-copy` / `wl-paste` with `image/png`).

## 39. The phone's notifications on the computer (N5a)

Built on `feat/phone-notifications`. On the A71, with no session open: the app's switch opened Android's access page, access was granted, and shell notifications (`cmd notification post`) were posted. Afterwards the switch was turned off and access revoked again.

- [x] Turning the switch on opens Android's notification-access page; after the grant, Settings shows the normal summary.
- [x] A notification reaches the Windows desktop as a toast. The shell's is private, so it showed only "New notification".
- [x] 31 within a minute: 20 shown, the drop logged once.
- [x] An app switched off in Settings sends nothing (no connection is made).
- [x] After the review fixes, apps are opt-in: Shell, listed but off, sent nothing; switched on, its notification arrived.
- [x] With the phone's Notifications permission off on the desktop, the server logs "Denied kdeconnect.notification" and shows nothing.
- [ ] A real app's public notification (a chat message) shows its title and text.
- [ ] A messaging app that hides its content on the lock screen shows only "New notification". (where: by hand)
- [ ] With no computer reachable, the phone's battery and the queue stay calm over an hour. (where: by hand)
- [ ] The Linux host (`notify-send`).

## 40. Closed on the phone, closed on the computer (N5c, part 1)

Built on `feat/notification-dismissal`. On the A71 with sharing on for Shell: a notification was posted, its toast found in the Windows Action Center by its tag, and the notification swiped away on the phone. The phone sent one cancel, and the toast left the Action Center. Afterwards sharing was turned off and access revoked.

- [x] A notification swiped away on the phone leaves the Windows Action Center.
- [x] A live ignored test shows a tagged toast and takes it back.
- [x] Only a notification the computer was shown is mentioned (one cancel for a group of 35, of which one had been sent).
- [ ] The Linux host: a notification closed on the phone closes in the notification server (D-Bus `CloseNotification`).
- [ ] A chat app that clears its notification when read on the phone. (where: by hand)

## 41. Texts (SMS) on the computer, and replies (N5b)

Built on `feat/sms`. The A71 has no SIM, so receiving is unverified. The reply path was run end to end:
- with "Reply from my computer" on, SEND_SMS granted and notification access allowed, the listener connected to the computer by itself;
- the server handed it a queued reply;
- the phone tried to send it;
- the radio answered "no SIM chosen for texts" (error 32), and the desktop recorded the reply as not sent, with that reason.

Afterwards everything was turned off again.

- [x] A reply reaches the phone with no session open, and its real result comes back (failure without a SIM).
- [x] A reply handed over before is not handed over again (fixed after the run showed duplicates).
- [x] After the review fixes: a reply is handed over once, and comes back failed with "the phone has no SIM chosen for texts".
- [x] A reply the phone never confirms is handed over at most 3 times, then fails with "the phone did not confirm it".
- [ ] A request from a computer that is not the phone's own is ignored.
- [ ] A sixth reply within a minute fails with "the phone sends at most 5 replies a minute".
- [ ] **With a SIM:** a text that arrives while the computer is off reaches Messages once it is back.
- [ ] Take the computer off the network with the phone connected: within about a minute of it coming back, the phone has dialed again (keepalive).
- [ ] **With a SIM:** an incoming text appears in Messages and as a notification.
- [ ] **With a SIM:** a reply from Messages arrives on the other phone, and the tab says sent.
- [ ] A long reply (over 160 characters) arrives whole.
- [ ] "New text" to a new number.
- [ ] The phone's Texts permission off on the desktop: texts are not kept, and replies fail with the reason.
- [ ] The Linux host.

## 42. Wake from the phone (N6)

Built on `feat/wake`. Sleeping this computer is off limits, so the run checked what the phone sends, with a UDP listener on this box. Whether a sleeping computer wakes is the user's to check.

- [x] A session tells the phone how to wake the computer: it saved the computer's MAC, its subnet broadcast (192.168.1.255) and "wired" under that computer's address (the session was forced view-only).
- [x] Wake on the card sends 9 magic packets for this computer's MAC: three rounds of the subnet broadcast, 255.255.255.255 (port 9), and the outside address at the outside port. Then "192.168.1.101 is awake." (the control port answered).
- [x] Wake settings shows what the computer said and the saved method, outside address and port.
- [x] Connect on a computer that can be woken but does not answer offers "Wake and connect" / "Connect anyway". "Wake and connect" sent the wake (a typed MAC, nothing learned: the 255.255.255.255 broadcast only) and counted down 30 s with Cancel.
- [x] Diagnostics on this box: "Ethernet (wired) allows waking by magic packet" (read through the NDIS keyword; this Realtek driver fails the power-management query).
- [ ] **Sleep, then wake from the phone at home** (Ethernet): the computer wakes, and the card connects once it answers.
- [ ] **From outside:** forward a UDP port on the router to the computer, set it as "Outside wake port", sleep the computer, and wake it from a phone on mobile data. Note how long after sleep the router still delivers it.
- [ ] Hibernate and shutdown (Windows fast startup off), and Wi-Fi only.
- [ ] "Through another computer": an always-on computer relays the wake.
- [ ] Away from home (another Wi-Fi): no broadcast is sent, only the outside packet; with no outside port, Wake says the phone is away and nothing is set up.
- [ ] A computer saved by its tailnet address counts as awake once its server answers. (where: away)
- [x] Turn "Wake from the phone" off on the desktop: after the next connection the phone forgets the MAC, and Wake disappears. (seen 2026-10-23 on the OPPO against a throwaway server (S2): with `wake_announce = false` and a restart, the next connection left the phone's wake info for that computer empty, so Wake has nothing to send.)
- [ ] The Linux host (ethtool's Wake-on, and the adapter chosen).

## 43. Several phones at once (N7)

Built on `feat/multi-phone`. Only one phone (the A71) is at hand.

- [ ] Two phones in sessions: the first to send input drives, and the other's taps do nothing while its session says "Another phone is driving".
- [ ] After 10 s without input from the driver (or its session ending), the other phone can take control. (where: second phone)
- [ ] Sessions tab: the driver is marked "Driving"; Give control hands it over within a second. (where: second phone)
- [ ] Settings "Several phones": every phone drives; the hand-back time. (where: second phone)
- [ ] Two quick Ends in the Sessions tab both end their sessions. (where: second phone)

## 44. Phase R1: end-to-end on Linux, across networks

The Linux host is a throwaway Azure VM (Ubuntu 24.04, B2ats v2, Southeast Asia) running the release workflow's Linux build under a headless Sway desktop, with PipeWire. The phone (A71, home Wi-Fi in another country) reaches it over the internet, which is also a real cross-network check. Results of 2026-10-14:

| Workflow | Result |
|---|---|
| Server start on Linux (screencopy capture, software encoder, PipeWire audio, mDNS, iroh WAN, DHT) | works |
| Add computer by address, then PIN pairing over the internet (control channel in TLS) | works |
| First contact: certificate pinned on first use, an unpaired phone refused cleanly ("session refused") | works (one earlier attempt stalled 10 s; not reproduced) |
| Direct QUIC session over the internet | works: 75–100 ms RTT, 45–55 ms e2e, no drops |
| iroh WAN with direct QUIC blocked | works: relayed for about a second, then a direct hole-punched path |
| Clipboard, phone → Linux and Linux → phone | works |
| A still Linux (wlroots) screen | **was broken**: one frame, then the phone gave up after 10 s. Fixed (a 1 s heartbeat) and re-verified for over a minute |
| Pairing from the refusal card | **was broken**: the stream did not come back. Fixed (the stream view is rebuilt) and re-verified |
| Retry after a dropped link | **was broken**: it stayed on "Connecting…". Fixed and re-verified |
| A dead link | **was kept open** behind the error card, so the desktop held the session. Fixed: closed at once, and the desktop ended it within a second |
| Input with `/dev/uinput` unavailable at start | **was gone** until a restart. Fixed: "Input injector ready" on the first input after access was given |
| The computer's name | **was "pocketdesk-host"** for a server started by a script. Fixed: the kernel's hostname |
| `pocketdesk unpair` during a session | **the session kept streaming**. Fixed (the server ends sessions of unpaired phones each second) and re-verified: ended within a second |
| Tailscale missing | a warning every few seconds; now one info line (re-verified) |
| Connect right after the phone wakes from doze | offered "Wake and connect" for a computer that was up; the probe now tries twice |

Still owed on Linux:
- [ ] Input reaching a real desktop (a headless Sway has no input devices; uinput was created but nothing consumed it).
- [ ] Files both ways, phone notifications and texts on Linux, desktop audio heard on the phone.
- [ ] Hyprland and GNOME/KDE (portal) hosts, and a GPU encoder.
- [ ] A WAN session's peer shows as `0.0.0.0:0` in the live-session list (cosmetic).

## 45. Phase R3 and R4: hostile connections, and idle cost

R3 is tested in code (see its status-history entry). On a device:
- [x] A LAN session still connects with the stream port's QUIC Retry (A71 → Windows, 2026-10-14; view-only held).
- [x] Three shares back to back (0.8, 1.6, 2.4 MB): all three saved with matching MD5s, about 0.3 s apart, none refused (A71 → Windows, 2026-10-14). A single SEND_MULTIPLE of three is still owed: `am` cannot build one.
- [x] A folder of 15 000 files: the sheet says at the top that only the first items are shown, and lists them (A71 → Windows, 2026-10-14).
- [ ] Turning off a phone's Power permission while it holds privacy mode (limit 6 in SECURITY.md): the grab ends at its 10 minute expiry.

R4, the desktop's idle cost. With the server running and no phone connected, in PowerShell:

```powershell
$p = Get-Process pocketdesk; $c1 = $p.TotalProcessorTime.TotalSeconds; Start-Sleep 120
$p = Get-Process -Id $p.Id; "{0:N2}% of one core, {1:N1} MB" -f (100 * ($p.TotalProcessorTime.TotalSeconds - $c1) / 120), ($p.WorkingSet64 / 1MB)
```

- [x] Windows, release build, WAN and the DHT on (2026-10-14): 0.68 % of one core, 33 MB working set (8.7 MB private), 26 threads.
- [ ] Linux.
- [ ] The phone's battery over an hour with the app in the background, and while a session is open. (where: by hand)
- [ ] Network traffic while idle (both ends). (where: by hand)

## 46. Phase R2: long sessions and recovery

On the A71 against this Windows box (LAN, release build, input off), 2026-10-14. The sampler logs, once a minute, the server's CPU, memory, threads and handles, and the phone app's memory, battery and temperature (`dumpsys meminfo`, `dumpsys battery`).

- [x] 35 minutes streaming: server 10–24 % of one core, private memory 84 → 96 MB, handles 674 → 705; the phone flat.
- [x] Wi-Fi off for 30 s: the stream came back by itself 12 s after the Wi-Fi did. **The control connection did not** (fixed; re-run: one reconnect, then steady).
- [x] The desktop's server stopped for 20 s: **the phone waited for Retry** (fixed; re-run: back 4 s after the server listened again, with no tap).
- [x] 30 minutes after the fixes: private memory 85 → 89 MB, handles flat at about 680, CPU 10–13 %.
- [x] Session start: one control connection that stays, plus two one-off queries.
- [x] An hour (2026-10-14): steady for 30 minutes (82 → 87 MB), then a reconnect left the old session running on the desktop, at up to 177 MB and double CPU. **Fixed:** the phone closes a stopped stream on the wire, and the desktop ends a session unheard from for 30 s. Re-run: a monitor switch leaves one live session.
- [x] An hour with one session from start to end, to confirm the memory levels off (2026-10-24, the OPPO over the internet road against a throwaway server, the runner's `w1-hour`: the app's memory 161.0 → 161.4 MB over 60 minutes, at most 164.2, the session up at the end.)
- [ ] The computer sleeping and waking (the user's to run), and the phone switching between Wi-Fi and mobile data (needs a SIM).
- [ ] The same on the Linux host.
- [x] A send to the phone cut by a drop continues where it stopped when sent again: 1 GiB cut at 30 % (the server stopped), the same file sent again continued from 31 % on the phone, and ended with the same MD5 (the runner's `x2-resume`, the OPPO over the internet road, 2026-10-24).
- [x] The same from the phone to the computer. (2026-10-24, the runner's `ac1-share-resume`: 1 GiB shared from the OPPO by the internet road, the throwaway stopped at 30 %, the phone's own retry continued from 30 % ("Continued … from …" in the server's log), the same MD5.)

## 47. Check connection (T1)

On the A71 against this Windows box, 2026-10-14:
- [x] From Home, with the server running, every check passes: Wi-Fi, same network, answers, the security key matches (now with the handshake signature checked), the video port answers (the UDP probe gets a real Version Negotiation reply). No battery warning without a background feature on.
- [x] With the server stopped, the computer "doesn't answer" (Windows' firewall turns a stopped server into a timeout, not a refusal), and the key and video port say "not checked".
- [x] From the session's error card, "Check connection" opens the same check.
- [ ] A changed security key (pin a wrong one), and "Scan code again" from the finding: the computer and its key removed, the scanner opened, and the new code trusted.
- [ ] A VPN that captures home traffic, and Tailscale off for a tailnet address. (where: away)
- [ ] Guest Wi-Fi with AP isolation, and another subnet. (where: away)
- [ ] A UDP-only block (the computer's firewall dropping the stream port). (where: owner)
- [ ] Android 17's local network permission denied.
- [x] Copy report on a device: no address or id in the text. (seen 2026-10-23 on the OPPO against a throwaway server (S2): the report pasted into a field, 173 characters starting "Pocketdesk connection check", with no address, id, MAC or the computer's name.)
- [ ] The Linux host (a stopped server there should be "nothing is listening").

## 48. Staying connected in the background (T2)

On the A71, 2026-10-14:
- [x] With phone notifications on and battery optimization on, Home shows "Android may stop Pocketdesk in the background", with "This phone's own setting" (Samsung's dontkillmyapp.com page).
- [x] "Allow" opens Android's battery optimization page, and Back returns to the app.
- [x] Settings, Background shows the status; "Remind me on Home" off hides the card, and on brings it back.
- [x] After allowing it in the battery settings, the card is gone on return. (seen 2026-10-23 on the OPPO against a throwaway server (S2): with the app on Android's idle allowlist (`cmd deviceidle whitelist`) the card was gone; its buttons ran off the card, fixed in #283.)
- [ ] Battery use set to Restricted: the card says so, and "Allow" opens the app's page. (where: by hand) (ColorOS keeps no restriction set from the shell.)
- [ ] With the app allowed, phone notifications and texts keep arriving overnight with the screen off (the reason for all this).
- [ ] Other makers (Xiaomi, OPPO): the guide link and their own battery managers. (where: other hardware)
- [x] V2: with the health card saying Android paused Pocketdesk, this card is not shown too: one card on Home, "This phone's own setting" once (the OPPO, 2026-10-24).
- [x] V2: the health card's "Allow" beside that pause opens what this card's does (on ColorOS the app's page, where battery use is set), Back returns to the app, and "Not now" on the health card brings this card back (the OPPO, 2026-10-24).

## 49. Protocol audit (what Pocketdesk sends, and to whom)

`docs/protocol.md` is the map. On the A71 and this box, 2026-10-14:
- [x] A WAN dial (stream port closed to force it) resolves n0's relay through the network's DNS: no `android context was not initialized` panic and no `failed to read system DNS configuration, using fallback` in logcat (the build before logged both).

Owed:
- [x] Linux: after `pocketdesk start`, the state folder is `700` and the first start said "owner-only" once (the Azure VM, Ubuntu 24.04, 2026-10-24).
- [ ] Linux: a `POCKETDESK_STATE_DIR` the user cannot chmod logs a warning and the server still starts. (where: linux)
- [ ] Wake through a relay computer with Power off for the phone: the phone says the relay did not answer and asks about Power, not "Wake sent". With a target off the relay's networks: "The relay computer refused: not on this computer's network".
- [ ] The desktop app's Devices tab reads "Lock, sleep, privacy mode and waking others".
- [x] Android backup: `adb shell bmgr backupnow dev.pocketdesk.android` backs up nothing ("Backup is not allowed", 2026-10-23 on the OPPO against a throwaway server (S2)).
- [ ] A device-to-device move (Smart Switch) leaves the app unpaired. (where: other hardware)
- [ ] Windows: after `pocketdesk install`, the firewall has TCP 1716-1725, UDP 4716 and TCP 1739-1764 rules only, and a phone on a fresh machine still lists the computer by mDNS (no UDP 5353 rule is made).
- [ ] A Wi-Fi with Tailscale up: a phone browsing mDNS sees the LAN address and no 100.x or fd7a: address. (where: away)
- [ ] Transfers both ways still end "saved" over many runs (the clean close after the status byte), and a desktop share to a full disk fails at once instead of after 10 s.

## 50. Resumable transfers (T6a)

On the A71 against Windows, 2026-10-15:
- [x] An offer of 150 MB with the phone's Wi-Fi off 2 s in: the retry continued ("The phone had 16580608 of 157286400 bytes"), one file in Downloads/Pocketdesk, MD5 matches.
- [x] A share of 120 MB the same way: the retry replaced the stale attempt and continued from 12.6 MB, MD5 matches, nothing left in `<state>/partials`.

Owed:
- [x] A file from the remote browser broken and continued. (2026-10-25, the OPPO by the internet road against a throwaway server, the runner's `af1-browse-resume`: 1 GiB from the remote browser cut at 30 %, the phone's own retry continued from 30-31 %, the same MD5; three runs.)
- [x] "Retry failed files" off: a broken file fails at once, and no part is kept. Tried 2026-10-19: the phone's Wi-Fi off for 200 s in a 900 MB download did not break it; QUIC carried it on when Wi-Fi came back and the whole file arrived ("sent"), so a real failure is still owed. (2026-10-25, the runner's `af2-retry-off`: a 1 GiB send cut at 30 % with the server left down; the phone's transfer ended 29 and 32 s after the cut (about the time it takes to call the link dead; retries would add 65 s and more), nothing of it kept.)
- [x] "Unfinished files: not kept" on the desktop: a retry starts over. (2026-10-24, the runner's `ac1-not-kept`: `transfer_keep_hours = 0`, a share cut at 30 % was started over by the phone's retry, no "Continued", the same MD5.)
- [x] A file that changes between attempts starts over (the prefix hash). (2026-10-24, the runner's `ac2-changed`: a send to the OPPO cut at 30 %, the file's first MiB rewritten with its size and date kept, sent again: "changed since the last attempt", started over, no "continued", the new file's MD5 on the phone.)
- [x] The app killed mid-download, then the offer fetched again within its 10 minutes. (seen 2026-10-23 on the OPPO against a throwaway server (S2) with #285: force-stopped at 42 MB of 150 MB, relaunched, the computer offered it again and the phone continued from 42 MB, MD5 matching; before #285 nothing fetched it again.)
- [ ] Android 8–9 (the legacy download file), and the Linux host.

## 51. Folders (T6b)

On the A71 against Windows, 2026-10-15:
- [x] Phone → computer: "Send a folder to this computer", a folder of three files in three levels: the same tree in Downloads, MD5s matching, `.hidden` left out.
- [x] Computer → phone: a folder queued for the phone during a session: the same tree in Download/Pocketdesk, MD5s matching, the hidden file left out, the Files tab's row "sent".
- [x] Sending the same folder again lands in "Name (1)".
- [x] After the review fixes: a request as the desktop app writes it (hyphenated id), 20 files in three levels: every file in its folder on the phone, MD5s matching; the dot-file and a zero-width-space name left out, and the Files tab's row says "1 left out".
- [x] The same folder from the phone: one received row for the folder (20 files, 8 MB), MD5s matching.

Owed:
- [ ] A folder from the phone shows one "folder arriving" toast, not one per file. (where: by hand) (the phone's folder picker.)
- [ ] The phone shows "Not all of … was sent" when it leaves names out, and "Stopped sending …" when the computer goes away mid-folder. (where: by hand) (the phone's folder picker.)
- [ ] A folder broken by Wi-Fi mid-way, continued. (where: by hand) (the phone's folder picker.)
- [ ] A folder over the limits (10 000 files, 32 levels) says what was left out. (where: by hand) (the phone's folder picker.)
- [ ] The desktop app's "Choose folder…" and dropping a folder on the window. (where: by hand)
- [ ] Names Windows refuses on the phone (`a:b`) left out and counted. (where: by hand) (the phone's folder picker.)
- [ ] Android 8–9, and the Linux host.

## 52. Download a folder from the remote browser

On the A71 against Windows, 2026-10-15 (`2026-10-15-download-folder-design.md`):
- [x] Home shows no "Download this folder"; a folder below it does.
- [x] An 8-file folder in three levels, with a dot-file and a file Windows marks hidden: all 8 at their places in Download/Pocketdesk, MD5s matching, the hidden two left out; the desktop's Files tab row "sent".

Owed:
- [x] A refusal's reason: with Files turned off after browsing, "The computer refused the folder: Files is turned off for this phone on t…", cut at the toast's two lines (2026-10-19, A71, throwaway server, by script).
- [ ] The toast "Asked the computer for …" (the folder arrived, the toast was not caught on screen), and a third folder while two are on the way.
- [ ] Closing the sheet before the answer, and the toast still showing. (where: by hand) (a toast uiautomator cannot read.)
- [x] A folder of thousands of files: the answer arrives, and other queries keep working while the offers come. (2026-10-25, the OPPO against a throwaway server whose browser shows its own folder, the runner's `ad2-big-folder`: 2000 files asked for whole, another folder listed 26 s later with 135 of them in, all 2000 on the phone by name in 422 s.)
- [ ] The Linux host.

## 53. The local API (T7)

On this box with the A71, 2026-10-15 (`docs/local-api.md`):
- [x] `status --json` with the server stopped (running false), and with the phone in a LAN session.
- [x] `devices --json`: three paired phones, the connected one marked, Input off as set.
- [x] `send <prefix> <file> --wait 120 --json`: "sent", exit 0, MD5 matching on the phone; with no phone connected, refused at once; with no server, exit 3; a usage error as JSON, exit 2.
- Seen on the way: the desktop's "sent" can come a moment before the phone's download is published (the MediaStore entry is still pending for a few milliseconds). Not new to this change.

Owed:
- [x] The Linux host: `status`, `devices`, `send --wait` (a folder to another computer) (the Azure VM, Ubuntu 24.04, 2026-10-24).
- [x] `send` of a folder with `--wait`, and a phone that leaves mid-send (exit 1). Five files and one in a sub-folder "sent" in 4 s, MD5s matching; the app force-stopped 2 s into six 40 MB files: "offered" (exit 4) while the offer's 10 minutes run, then exit 1 at 637 s, "6 of 6 files not sent: the phone did not take it in time" (2026-10-19, A71, throwaway server, by script).
- [x] A broken config.toml with `--json` (exit 1, JSON): `status --json` gave `api` 1 and the TOML error (2026-10-19, A71, throwaway server, by script).

## 54. Automation apps on the phone (T7b)

On the A71 against Windows, 2026-10-15 (`docs/phone-automation.md`):
- [x] A text share naming a computer that is not saved reached no computer: no clipboard packet at the desktop, its clipboard unchanged.

Owed:
- [ ] Tasker itself: the clipboard recipe, a file with "Grant read permission", Connect and Wake from a task.
- [ ] A share naming one of two saved computers goes to that one (the A71 has one saved). (where: owner) (the other saved computer is the owner's own.)
- [ ] From the background, with and without "Display over other apps". (where: by hand) (Tasker.)

Seen on the way: sessions ending every ~10 s, and slow transfers. First taken for this computer's display being asleep; it was the link (§55).

## 55. A slow or lossy link (fix/slow-link-watchdog)

On the A71 against Windows, 2026-10-15, over Wi-Fi that lost about a third of its packets (strong signal, but large packets dropping; ~36 KB/s on the wire, pings 40-60 ms):
- [x] Before: every session ended after 10-15 s ("video feed silent"). The desktop wrote ~2.5 MB of frames per session into the connection while ~0.5 MB reached the wire; the phone waited on one starved frame stream. Downloads during that churn ran at ~26 KB/s, and one file was published ~16 s after the desktop said it was saved (the same starved link).
- [x] After (the phone counts video bytes, and the desktop keeps at most 8 frames on the wire): one session held 135 s until ended by hand, at 36 % loss.
- [x] After the review (the window sized to a round trip, any delivery making room): a session held 90 s with no drop on the same link, pings averaging 77 ms.
- [x] Loopback (`server/tests/session_record.rs`, a real capture and encode): with and without the bound, two runs each, frames encoded (168-196), rate (1.6-2.4 Mbit/s) and e2e p50 (16-21 ms) alike. Its e2e p99 of ~3 s is the same without the bound: older than this change, from the session's start.

Owed:
- [ ] A healthy LAN: frame rate and e2e unchanged (the in-flight bound must not throttle it). (where: owner) (a throwaway server's LAN port needs a firewall rule.)
- [ ] WAN, direct and relayed: frame rate at a 150-300 ms round trip. (where: away)
- [ ] The picture's delay on the lossy link (how stale it runs), and a keyframe after a long stall. (where: away)

## 56. Links both ways (T8a)

On the A71 against Windows, 2026-10-15, LAN, input off for the phone:
- [x] `pocketdesk link <phone> https://example.org/...` printed `queued` (exit 0); the phone showed "A link from <address>" on its own channel, "Links from the computer"; tapping it opened the link in the phone's browser.
- [x] `pocketdesk link` with `javascript:alert(1)`: exit 2, "not a web link".
- [x] "Open on computer" with "A page https://example.org/..." opened the link in this computer's browser ("Opening a link from …" in the log); a share with only a `file:` link sent nothing.
- [x] With the phone's Links permission off: a share from the phone was not opened, and a queued link was not sent ("links are off for this phone"). (Since then `pocketdesk link` and the app refuse such a link up front.)
- [x] After the review fixes: "Read (https://example.org/…)." opened on the computer (from its own thread); "See http://localhost:8080/admin." was refused on the phone, and `pocketdesk link <phone> http://127.1/` with "a link to the machine's own address".

Owed:
- [ ] Chrome's and Firefox's share sheets ("Open on computer" listed, the title and link they share). (where: by hand)
- [ ] The desktop app's Send link, and its refusal for a phone with Links off. (where: by hand)
- [x] `open_links = false`: links shown as notifications, none opened (2026-10-15, six links from "Open on computer").
- [x] Six links within a minute: the sixth refused ("more than 5 a minute").
- [x] The phone's "Links from the computer" off: a link dropped (no notification; on again, it showed). The computer says "sent to the phone" either way, since the phone does not answer (2026-10-19, A71, throwaway server, by script).
- [x] Linux (2026-10-15, the release dry run's build on an Azure VM under a headless Sway, the A71 over the internet): "Open on computer" reached `xdg-open` with the link (the sentence's full stop left out), and no zombie process was left; `pocketdesk link` from the Linux CLI gave the phone its notification.

## 57. The phone's battery on the computer (T8b)

On the A71 against Windows, 2026-10-15, LAN, input off for the phone:
- [x] At the session's start `phone_status.json` held the phone's battery, 100 % and charging over USB, matching `dumpsys battery`.
- [x] `dumpsys battery unplug` and `set level 12`: one notification on the computer, "Phone <id>: battery low (12 %)"; `set level 10` gave none more; `pocketdesk devices --json` showed 10 %, not charging.

Owed:
- [ ] The Devices tab's text, and its age after the session ends. (where: by hand)
- [x] After the review fixes: the report written at the session's start, and no control connection in the next 70 s beyond the two the session opened (the old loop dialed one per 15 s without a link).
- [ ] Catching up after a Wi-Fi drop. (where: by hand) (the phone's battery reads the hardware, so a report changes only with a real one.)
- [x] Unpairing the phone: its report gone from the file within 5 s (2026-10-19, A71, throwaway server, by script).
- [x] The phone's "Share battery with the computer" off: no report in 30 s of a background link; on again, the report within a minute (2026-10-19, A71, throwaway server, by script).
- [ ] The desktop's "Phones' batteries" off: no notification. (where: by hand) (the phone reads its battery from BatteryManager, which `dumpsys battery set` does not move.)
- [x] Linux: a faked 11 % was recorded and warned about in the log, but **no notification was shown**: every Linux notification failed (`notify-send --expire` is not an option). Fixed (`--expire-time`); owed: see it shown on Linux.
- [ ] A real discharge to 15 %, and Android's own low flag. (where: by hand)

## 58. The computer's media from the phone (T8c)

On the A71 against Windows, 2026-10-15, LAN, input off for the phone. A silent track played through WinRT's `MediaPlayer` from a PowerShell process stood in for a player (it reports to the system media controls with no title):
- [x] The Sound sheet's "Now playing on the computer" showed "powershell" with Pause, and the volume and output list beside it read correctly (no reply taken by another call).
- [x] Pause on the phone paused it (the computer's session read `is_playing: false`) and the card offered Play; Play resumed it.
- [x] With the phone's Audio permission off, the card said "Audio is turned off for this phone on the computer".

Owed:
- [ ] A real player with a title: a browser video, Spotify; Next and Previous.
- [x] Nothing playing: "Nothing is playing" (2026-10-19: something was playing on this box, so not reached). (seen 2026-10-23 on the OPPO against a throwaway server (S2) with the media seam: "Nothing is playing", then the song, then "Nothing is playing" again.)
- [x] The Sound sheet's output devices by name alone (2026-10-19, A71, throwaway server). Found there: each had its Windows endpoint id under its name; fixed, the id shows only for two devices with one name.
- [x] The card cleared to the error alone with Audio off (fixed after the run). (seen 2026-10-23 on the OPPO against a throwaway server (S2): with a song playing on the computer (the media seam) and Audio off, the card said only "Audio is turned off for this phone on the computer".)
- [x] Linux: with `mpv` and its MPRIS plugin playing, the card showed "VM Test Tone" by "Pocketdesk · mpv", and Pause on the phone paused it (`playerctl status`: Paused).
- [ ] Linux: `playerctl` missing.

## 59. The phone's media from the computer (T8d)

On the A71 against Windows, 2026-10-15, LAN, input off for the phone. The A71 (LineageOS) has no music player, so a throwaway instrumented test held a "playing" media session in Pocketdesk's own process; notification access was granted for the run and revoked after, and "My media on the computer" set and restored:
- [x] The phone's report reached the computer: `phone_media.json` held "Probe Song" by "Probe Artist", playing, all three buttons.
- [x] A next press and a play/pause press, written as the desktop app writes them, reached the session ("press: next", "press: pause"), and the report followed to paused.

Owed:
- [ ] The Sessions tab itself, and its buttons. (where: by hand)
- [ ] A real player (Spotify, a podcast app): its title, and the app's name (Android 11+ package visibility may give the package name).
- [x] After the review fixes (2026-10-15, the probe again): the song gone within seconds when Media was turned off and when the session ended; reported again 9 s after a server restart mid-session.
- [x] Found on that run: with Media turned back on the song did not return until it changed; fixed (the phone says it again each minute). Owed: re-check that it returns within a minute.
- [ ] The phone's setting turned off clearing the song; a press sent while the app was in the background not acting later. (where: by hand) (a real player on the phone.)
- [ ] Turning the setting on opening the notification access page. (where: by hand) (notification access already granted here.)

## 60. Pinch-zoom in trackpad mode (T9a)

adb cannot make multi-finger gestures; the engine has 21 JVM tests. Owed, by hand on the A71:
- [ ] In trackpad mode, two fingers spreading zoom the view, pinching zooms out, back to 1x. (where: by hand) (adb cannot pinch.)
- [ ] Two fingers moving together still scroll the desktop, and a scroll that drifts apart does not start a zoom. (where: by hand) (adb cannot pinch.)
- [ ] While pinching, moving the fingers pans the view; one finger lifting and landing again does not jump the zoom. (where: by hand) (adb cannot pinch.)
- [ ] A two-finger tap is still a right click, a three-finger tap a middle click.
- [ ] Zoomed in, the cursor's travel matches the finger's on-screen distance, and scrolling is not slowed by the zoom.

## 61. Portrait sessions (T9b)

On the A71 against Windows, 2026-10-15, portrait, input off for the phone:
- [x] The session split: the 21:9 desktop in the top quarter, the trackpad dock and shortcut bar below (before, a letterboxed strip and black).
- [x] After the review fixes: the chrome faded after a few seconds, leaving the whole desktop visible.

Owed:
- [x] The on-screen keyboard opening in portrait: the layout stayed split, the desktop kept its full top pane, and the key bar and keyboard sat over the dock (2026-10-15).
- [x] Turning the phone: on its side the A71 keeps one pane, the stream full screen (C5 changed this: it had split side by side, the stream on half the screen).
- [ ] A 16:9 desktop's pane, a monitor switch between shapes, picture-in-picture, and Pane: Single and Dual. (where: by hand)

After the owner tried it (2026-10-19: the chrome over the picture, a trackpad three quarters of the screen, and a trackpad in Direct touch), on the A71 against a throwaway server, input off:
- [x] Direct touch, held upright: one pane, the desktop centred with the link and window chips above it and the mode chip, keyboard and menu below, none over the picture.
- [x] Trackpad, held upright: the desktop in a pane with room above and below it for the same chrome, none over the picture, the trackpad and shortcut bar about 40 % of the screen.
- [x] Switching between Direct touch and Trackpad mid-session: the pane changes without a new decoder (three switches, the session held, no decoder surface change in logcat; 2026-10-19).
- [x] The chrome stays on screen when held upright (12 s untouched in both modes), and the menu opens with one tap (2026-10-19).
- [x] Turned on its side the chrome fades over the full-screen picture as before; turned upright again it comes back by itself (2026-10-19).
- [ ] Pinch-zoomed upright: the zoomed picture stays inside its area, not under the chrome (adb cannot pinch). (where: by hand) (adb cannot pinch.)
- [ ] Pane: two panes with Direct touch: taps land on the point under the finger in the smaller picture.
- [x] "Show connection details" on, held upright: the details, the link and the window chips sit above the picture in both modes (the bands follow the chrome's measured height).

## 62. A mouse on the phone (T9c)

No mouse was at hand; with none connected, a session on the A71 started and Back ended it as before. Owed, with a USB OTG or Bluetooth mouse:
- [ ] Motion, all five buttons and the wheel (both ways) reach the desktop, and a fast mouse keeps its travel. (where: other hardware) (a mouse.)
- [ ] The phone's pointer is hidden during the session and back over Home after Exit.
- [ ] Opening and closing the in-session keyboard: the mouse keeps working. (where: other hardware) (a mouse.)
- [ ] The mouse's Back button goes to the desktop and does not end the session. (where: other hardware) (a mouse.)
- [ ] Settings off: the phone's own pointer, no capture.

## 63. Contact names in texts (T8e)

On the A71, 2026-10-15 (no SIM, so no text could arrive):
- [x] Settings' Texts section shows "Names from my contacts", off; turning it on asked for contacts access, and denying left it off.

Owed (a SIM):
- [ ] A text from a contact shows "Ann (number)" in the notification and in Messages; a stranger shows the number only; a sender ID with letters gets no name.
- [ ] A text queued while the computer is away, then names turned off: it goes without a name.
- [ ] Desktop "keep the contact names" off: new texts carry none, and after a server restart the kept names are gone.

## 64. The control channel over the internet (T10a)

On the A71 against this box, 2026-10-16: the saved computer's control port was pointed at a closed port, so TCP failed as it does away from home:
- [x] The phone logged "No control link at home; trying the internet road", then connected; the desktop logged a control connection over the internet and admitted it (the phone's iroh key was registered).
- [x] Clipboard both ways, the battery and the 4 s clipboard polls ran on it.
- [x] After the review fixes: `pocketdesk link` to the phone reached it over this link and showed as a notification (a desktop push).

Owed:
- [ ] From outside the home network (mobile data, or another Wi-Fi), with Tailscale off: notifications, texts, battery and the phone's media reach the computer with the app in the background, and Send link reaches the phone.
- [ ] A phone with no iroh key registered (paired, never streamed on the LAN) is refused, with one log line, and works again after one session at home. (where: owner) (a session at home needs the throwaway's LAN port open.)
- [x] Unpairing on the desktop ends the internet link's packets at once: the phone's background link over the internet road gone within 5 s of `pocketdesk unpair` (2026-10-19, A71, throwaway server, by script).
- [x] A file offer to a phone on the internet link rides that link (2c; S2's 150 MB offer, #285), and with files over the internet off a send is refused at once with the reason (#288). (Seen 2026-10-24 on the OPPO against a throwaway server (S3).)
- [ ] The phone's battery over an hour with the link up in the background (iroh's 5 s heartbeat).
- [ ] A session riding a relay (no direct path) with the control link up at the same time. Founded: two endpoints with one key cannot share a relay (`server/tests/one_key_two_endpoints.rs`); since 2b both ride one endpoint. Owed on the VM, relayed (checklist §85).

## 65. The clipboard outside a session (T10b)

On the A71 against this box, 2026-10-16:
- [x] With "Computer's clipboard in the background" on and notification access, and the app in the background, text copied on the computer reached the phone's clipboard within one tick.
- [x] The "Send clipboard" tile sent the phone's clipboard text to the computer's clipboard.

Owed:
- [ ] Away from home (the internet link): both directions.
- [x] Desktop "Clipboard to phones" off: nothing pushed (C6: the phone linked, nothing in 40 s; on, it arrived in 5 s).
- [x] With it off, a session still gets the clipboard on request. (seen 2026-10-24 on the OPPO against a throwaway server (S3): with `clipboard_push = false`, a copy changed on the computer mid-session reached the phone by the session's poll.)
- [x] A phone with Clipboard off on the desktop gets no pushes (C6: linked, nothing in 40 s).
- [x] Linux (the Azure VM, headless Sway, this branch's release build): text copied with `wl-copy` reached the A71 in the background. The VM had no `wl-clipboard` at first; without it the computer's clipboard is never read (`wl-paste` is a runtime need on Wayland).
- [ ] Linux with an image on the clipboard, and a copy offering `x-kde-passwordManagerHint` (KeePassXC).
- [ ] Android 10–12: the tile's window reads the clipboard.

## 66. Answering the phone's notifications from the computer (T10c)

On the A71 against this box, 2026-10-16, with a throwaway chat app that posts a notification with Reply and "Mark as read" (not committed):
- [x] The computer showed it in full and kept it with its button and its reply.
- [x] A reply written as the desktop app writes it reached the app through its `RemoteInput`; "Mark as read" ran; a button it did not offer was refused by the server.
- [x] Found and fixed on the way: nearly every notification arrived as "New notification" (an app's default private was always hidden), and answers were dropped while the phone had no live link.
- [x] After the review fixes: a button that requires unlocking (`Approve`) was not offered, and a press of it was refused; reply and "Mark as read" ran again.
- [ ] "Hide sensitive content" on the lock screen turned on: the probe's notification arrives as "New notification" and offers nothing. (where: by hand)

Owed:
- [ ] The desktop app's Notifications tab itself (its buttons and reply box), and a real chat app (WhatsApp, Signal, Messages).
- [x] The switch off: nothing offered, an answer not run. On the OPPO with the fixture app's chat notification and a throwaway server, no session: with "Let the computer answer notifications" on, the computer kept it with "Mark as read" and a reply; with it turned off, an answer to that note came back `not_sent` ("answering is off on the phone") and the app ran nothing, and a new note offered nothing (2026-10-24).
- [ ] From away from home (the internet link).
- [ ] An action that opens an activity on Android 14+ (background launches may be blocked). (where: by hand)

## 67. Files from the computer without a session (T10d)

On the A71 against this box, 2026-10-16:
- [x] With "Files from the computer in the background" on and notification access, the app in the background and no session, the mirror listed the phone as linked with files, and `pocketdesk send … --wait` ended "saved on the phone", MD5s matching. Android 16 let the listener start the download at once.

Owed:
- [ ] A phone that refuses background starts: the "The computer sends a file" notification, and Save downloading it. (where: by hand)
- [x] A folder sent this way (C6: a folder with a sub-folder, no session, checksums matching).
- [ ] The desktop app's Files tab listing a linked phone with no session. (where: by hand)
- [ ] Away from home: `pocketdesk send` refused at once as not reachable.

## 68. Typing any character on Linux (F4)

On the Azure VM (headless Sway, this branch's release build) from the A71, 2026-10-16:
- [x] "Type phone clipboard" with `Héllo Wörld € 😀 தமிழ் ABC!` reached `wev` character by character through the virtual keyboard (the server logged "Text is typed through the compositor's virtual keyboard").
- [x] Found: with `/dev/uinput` not writable by the server's user, there is no injector at all, so not even text is typed, though the virtual keyboard needs no uinput.

Owed:
- [ ] Hyprland; a desktop with a non-US layout (`fr`), where text must still arrive as sent.
- [ ] GNOME or KDE (no virtual keyboard): capitals and `!@#` arrive through uinput with Shift.
- [ ] The in-session keyboard (IME text) as well as "Type phone clipboard".

## 69. "Send to phone" in the file manager (T10e)

On this box with the A71, 2026-10-16:
- [x] `send-menu` made the Send To shortcut (the server, `send-to-phone`, minimized), and `send-menu --remove` removed it.
- [x] `send-to-phone` with two files, with the phone linked in the background: both were saved, MD5s matching, and the desktop notification said what was sent.

Owed:
- [ ] Send To in Explorer itself, with several files and a folder. (where: by hand)
- [ ] Nautilus, Nemo and Dolphin on Linux.
- [ ] The desktop app's Settings switch, and `install` / `uninstall` adding and removing the entry. (where: owner) (`install` changes this computer.)
- [x] No phone connected: the notification says so. (seen 2026-10-24 on the OPPO against a throwaway server (S3): `pocketdesk send-to-phone` exited 1 and a toast said no phone is connected, read back from Windows' notification store.)

## 70. The phone's folders on the computer (T10g)

On the A71 against this box, 2026-10-16:
- [x] Settings "Folders the computer may open" → Add folder → Android's picker (Download/Pocketdesk) → Allow: listed with Remove; Remove emptied it.
- [x] With the app in the background, a list request (written as the Phone tab writes it) showed the folder and a sub-folder; a fetch saved the file in Downloads, MD5s matching.
- [x] Again after the review fixes: a hidden file neither listed nor sent when named; a fetch of a file deleted on the phone answered "gone.bin is not on the phone any more" with its folder; a list of a removed root answered "That folder is no longer shared"; a fetch with MD5s matching.

Owed:
- [ ] The desktop app's Phone tab itself (roots, folders, Up, Download). (where: by hand)
- [x] A folder of more than 2000 entries: of 2100, the first 2000 listed and marked as cut (`truncated`, the Phone files page's "Showing the first 2000 entries"); a file renamed between list and fetch: the fetch said "f1.txt is not on the phone any more" and nothing arrived (the runner's `y1-big-folder`, `y1-renamed`, the OPPO, 2026-10-24).
- [ ] A second computer asking: nothing answered.

## 71. Input on wlroots without uinput (T10f)

On the Azure VM (headless Sway, `/dev/uinput` root-only) from the A71 over the internet, 2026-10-16:
- [x] The server log: "Input injector: using the compositor's virtual pointer and keyboard".
- [x] Trackpad motion and a tap reached `wev` as pointer motion and a left press and release.
- [x] "Hi PD!" typed (capitals and `!`, through F4's per-character keyboard), and Enter and Esc from the shortcut bar (through the new keys keyboard).

Owed:
- [ ] Hyprland, river, labwc and Wayfire.
- [ ] Direct touch (absolute moves), scrolling, right and middle clicks, and a two-output layout.
- [x] After the review fixes: Ctrl+C and Shift+Left arrived with their modifiers (`wev`: depressed Control, then Shift; before the fix there was no modifiers event, so Ctrl+C was a plain "c").
- [x] Sway restarted under the running server: the next input reopened the virtual devices ("are back") and moved the pointer.
- [ ] Super+N, and the text fallback typed as keys.
- [ ] Privacy mode where `/dev/input` is root-only too: the local keyboard is likely not blocked.

## 72. New phone screenshots on the computer (T10h)

On the A71 against this box, 2026-10-16:
- [x] Settings "Send new screenshots" asked for photo access and turned on.
- [x] A screenshot (`input keyevent 120`, the app in the background) landed on the computer's clipboard (1080x2400) and in Downloads, MD5s matching.
- [x] An image added under `DCIM/Camera` was not sent; with the switch off, a new screenshot was not sent.
- [x] After the review fixes: a screenshot taken with the server stopped waited, and arrived about 20 s after it started (MD5s matching); revoking photo access turned the switch off.

Owed:
- [ ] A screenshot with the side keys, and a JPEG screenshot (One UI's other format). (where: by hand)
- [ ] Android 14+'s "selected photos only" answer, and Android 12 and older.
- [ ] Away from home (the internet road), and the Linux host.

## 73. The phone as a remote (T10i)

On the A71 against this box, 2026-10-16:
- [x] With Input off, Remote was refused: "The remote stopped: input not allowed" (server: "Input-only session refused").
- [x] With Input on, Retry reached the computer (found and fixed: Retry had reused the refused session), the controls showed once admitted, `status --json` said `control_only: true`, and the server log had no capture, encoder or audio for it.
- [x] A trackpad drag moved the computer's pointer; Exit ended the session on the computer.
- [x] Against a computer without T10i (the VM's older build): the remote did not hang once its session ended (found and fixed: the server never closed a finished session's connection; the phone now also gives up after 45 s).

On the Azure VM (headless Sway, uinput root-only) from the A71 over the internet, after the review fixes:
- [x] Admitted with no capture; into `wev`: Page Down, Right, F5, B, Play/Pause and Volume Up from the rows, "Hi PD" from the keyboard, trackpad motion and a tap as a left click.

Owed:
- [x] "Ask before each session" holds a remote before it is ready: held 10 s after Remote, the phone saying "Waiting for the computer to allow it…", turned down some 30 s later with no answer. (Seen 2026-10-24 on the OPPO against a throwaway server (S3).)
- [ ] The desktop app's prompt for a remote, clicked. (where: by hand)
- [ ] Over the internet road, and a phone driving while another has a picture session (driving rules). (where: owner) (a remote needs the Input permission.)
- [ ] I1: the computer's server restarted under a remote: "Reconnecting in N s…" on its card, then the remote back by itself, the talk timer still counting; Kick from the computer: not retried; Settings "Reconnect by itself" off: Retry only; after ten minutes, "Stopped trying" and the session's notification gone. (where: owner) (a remote needs the Input permission.)
  - Checked 2026-10-20 on the OPPO (CPH2359, Android 14) against the throwaway server: the server killed under a remote with a talk timer; first noticed only at 47 s (the transport's idle timeout), then, with I1b, at 14 s, "the computer stopped answering" and its countdown, and the remote ready again by itself 14 s after the server returned, the timer counting through.

## 74. The phone's battery without a session

On the A71 against this box, 2026-10-16, with "Computer's clipboard in the background" keeping a link and no session:
- [x] `dumpsys battery set level 12` (unplugged): `phone_status.json` said 12, and the computer warned "battery low (12 %)".
- [x] With no session listed at all, level 9 reached `phone_status.json`.
- [x] After the review fix (the live link only): with no session listed, 13 % reached `phone_status.json` and was warned about.

Owed:
- [ ] With only folders, answers or files keeping the link; with none on, nothing sent and no connection made; the link dropping just before a report (nothing dialed).
- [x] Over the internet road. (seen 2026-10-24 on the OPPO against a throwaway server (S3): with `adb reverse` removed the control link came by iroh and a battery report arrived as it came up.)

## 75. A second screen at the phone's shape (T10j)

On the Azure VM (headless Sway, no XWayland) from the A71 over the internet, 2026-10-16:
- [x] "Add a screen for this phone" in the picker: Sway made HEADLESS-3 at 1080x2400 right of the desktop, the session switched to index 1 and encoded 1080x2400, and a window moved onto it.
- [x] Found and fixed: Sway moved the desktop's own output (x 0 to 2360) when the screen was added; each output is now pinned first. Re-checked: the desktop's output stayed at x 0, the new screen right of it, streamed at 1080x2400.
- [x] After the phone left, the server removed the screen and its record; the window went back to the desktop's output.

Owed:
- [ ] "Phones may add a screen" off: refused with its reason; on Windows and Hyprland: "only Sway can for now".

## 76. Hotspot and tethering in Check connection

JVM-tested only (the A71's build cannot start its hotspot from adb, and a computer joining it would mean changing this box's network). Owed:
- [ ] A computer joined to the phone's hotspot, saved under that address: "on this phone's hotspot or tethering".
- [ ] The same computer saved under its home address: the network finding, with "This phone is sharing its connection" beside it. (where: other hardware) (the phone's hotspot.)
- [ ] USB tethering (`rndis0`/`ncm0`), and the interface names on other makers' phones (Pixel's hotspot may be `wlan1`, not read).

## 77. This phone's screen on the computer (T10k)

A71 (Android 16) → this Windows box, LAN, debug builds, 2026-10-16:
- [x] "Show screen" on the saved computer; Android's consent ("Share entire screen"); the server admitted the cast, the relay handed it to the desktop app, and a window at the phone's shape (864x1920 encoded) showed the phone live: Home, then Settings, sharp once settled.
- [x] Found: the A71's encoder refuses the Baseline profile; the fallback to its default profile works (and now logs one line).
- [x] Turned to landscape: a new 1920x864 encoder, the window turned to landscape; turned back, back to portrait.
- [x] A Home press reached the window within about a second (sampled every 260 ms; not a stopwatch measurement). ffmpeg used about 6 % of one core and 115 MB on a mostly still screen.
- [x] "Stop sharing" from the status bar's chip: the session, the relay's cast, `cast.json` and ffmpeg all ended, and the window closed.
- [x] "Show its screen here" off for the phone: refused, and the phone's notification said "it does not allow this phone to show its screen".
- [x] Closing the window mid-cast: it stayed closed, ffmpeg stopped, and the cast went on.
- [x] `pocketdesk kick all`: the phone's cast service and its projection ended.
- [x] After the review fixes (cast-only state and stop, frames on the wire bounded): a cast shown again; then the computer's card tapped mid-cast opened a screen session, the cast ended quietly (no failure notification, `cast.json` gone), and the new session stayed live.

Owed:
- [ ] The latency against a stopwatch, and over the internet road (WAN, relayed and direct).
- [ ] The notification's Stop (the A71 showed only the redacted notification in the shade while locked), and Sessions' "Show screen" after the window was closed. (where: by hand)
- [ ] "Share one app" instead of the entire screen; a phone call or a secure screen (banking apps black out); a long cast's heat and battery. (where: by hand)
- [ ] The Linux desktop app (Wayland and X11), and a computer without `ffmpeg` (the window says so).

## 78. Events and hooks (ecosystem 1a)

A71 → this Windows box, LAN, debug server, 2026-10-16, with `pocketdesk events --json` following the log and a `session_started` hook copying its stdin to a file:
- [x] `session_started` (mode `screen`, transport `lan`) and `phone_linked` as the app opened a session; `phone_unlinked` when the app was force-stopped; `session_ended` once the server noticed the silence (about 30 s later, as designed).
- [x] The hook ran, with the event's JSON on its stdin.
- [x] A battery faked at 13 % and unplugged: `phone_battery` (13, not charging) and `phone_battery_low`.
- [x] Nothing about content at the default detail.
- [x] After the review fixes: a hook starting with a quoted program (`"C:\Windows\System32\cmd.exe" /C more > "…"`) ran through the queue with the event on stdin, exit code 0.

Owed:
- [x] `notification` and `link_received` from a phone at both detail levels: metadata gave the app and the host only, "Details in events" added title, text and URL. (Seen 2026-10-24 on the OPPO against a throwaway server (S3).)
- [x] `file_received` at both detail levels: a share from the phone by the internet road gave the size alone with `events_detail = "metadata"`, and the size and the file's name with `"full"` (the runner's `x1-file-received`, the OPPO, 2026-10-24).
- [ ] `text_received` at both detail levels. (where: sim)
- [x] "Event log" off (`events = false`): a notification and a link wrote no line. (Seen 2026-10-24 on the OPPO against a throwaway server (S3).)
- [ ] The desktop Settings hooks box saving a hook the server then runs. (where: by hand)
- [x] A hook killed at 30 s on a real server: a `ping -n 60` hook on `notification` gave `hook_ran` 32 s later with `timed_out: true` and no code. (Seen 2026-10-24 on the OPPO against a throwaway server (S3).)
- [ ] The 33rd waiting hook skipped, on a real server: 37 events of a kind within 30 s (a phone sends at most 20 notifications a minute, so another source).
- [x] Linux: a `file_received` hook using shell syntax ran through `sh -c`, the event on stdin and in `POCKETDESK_EVENT`; `events.jsonl` is `600` (the Azure VM, Ubuntu 24.04, 2026-10-24).

## 79. The Activity Center (ecosystem 1b)

A71 → this Windows box, LAN, debug server, 2026-10-16:
- [x] A screen session, then "Remote" with the phone's Input off: the log held `session_started`, `phone_linked`, `session_ended`, `session_refused`, the `hook_ran` of a `session_refused` hook (exit 0) and `phone_unlinked`.

Owed:
- [ ] The Activity tab itself on screen (its rows, filters, Show in folder): a click posted to the window did not reach egui, and real input into this box's desktop is not used for tests.
- [ ] Pairing and unpairing a phone showing as rows; a folder's Show in folder after it finished arriving. (where: by hand)

## 80. Named tasks (ecosystem 1c)

A71 → this Windows box, LAN, debug server, 2026-10-16, three tasks in config.toml:
- [x] Quick Settings, "Tasks on this computer": the three names, "Asked" marked as asking first; no command line on the phone.
- [x] "Say hello": "done (exit 0)" with its output on the phone; `task_started` / `task_finished` in the event log.
- [x] "Asked": "waiting for the computer's owner to allow it…", held in `task_requests.json`; allowed through `task_answers/` as the desktop app writes it; then "done (exit 0)" with its output.
- [x] "Slow" (`deadline_secs = 3`): "stopped at its deadline"; `task_finished` with `timed_out`.
- [x] After the review fixes: "Say hello" again over the live link, and "Slow" stopped at its deadline with what it wrote before it (its first lines) kept and shown on the phone.

Owed:
- [ ] The desktop app's Allow / Deny prompt itself (clicked, not written), Deny on the phone, and the Settings task editor. (where: by hand)
- [x] "Start my tasks" off: the phone gives up with "no answer" after 20 s. Found there: it showed the bridge's "The desktop did not answer over the control connection"; fixed, the computer answers at once and the sheet says 'Could not ask the computer: "Start my tasks" is turned off for this phone on the computer' (2026-10-19, A71, throwaway server).
- [ ] Linux (`sh -c`), a long output's end on the phone, and two phones at once.

## 81. Send from the browser (ecosystem 1d)

This Windows box and the A71, debug builds, 2026-10-16:
- [x] `pocketdesk browser-host --install`: registered with Chrome, Chromium, Edge, Brave, Vivaldi and Firefox (HKCU keys naming the manifests; the Chromium manifest allows only `chrome-extension://abojofjjnpcmejhdonndlegfaolfkcif/`); `--remove` took the keys and files away.
- [x] The host driven as a browser drives it (`pocketdesk.exe chrome-extension://…/` and framed JSON), with a session on the A71: a link answered "Sent to your phone" and showed in the phone's notification; an image (an SVG behind a redirect) arrived in `Download/Pocketdesk`, its MD5 matching the site's; `localhost` and an unknown request refused.
- [x] Found and fixed: images kept in the state folder were never sent (the server does not send its own folder's files); they wait in a folder of the user's own now.
- [x] After the review fixes, again through the host with a session on the A71: a PNG from Wikimedia arrived in `Download/Pocketdesk`, MD5 matching (it had answered 403 until the download named itself with a User-Agent); an SVG refused as not a picture Pocketdesk sends; a redirect to `127.0.0.1` refused with why; a link sent.

Owed:
- [ ] The extension itself loaded in Brave or Chrome and in Firefox (unpacked), and its menus and notifications. (where: by hand)
- [ ] Linux registration (`~/.config/<browser>/NativeMessagingHosts`, `~/.mozilla/native-messaging-hosts`), images waiting in `~/.cache/pocketdesk-browser`, and the desktop Settings switch.

## 82. Home Assistant over MQTT (ecosystem 1e)

This Windows box and the A71, debug server, 2026-10-16, a local broker (amqtt 0.12 on 127.0.0.1) and a subscriber on `homeassistant/#` and `pocketdesk/#`:
- [x] On connect: `availability` `online`, then the discovery configs (the server, sessions, cast, Activity, each paired phone's connection, the A71's battery and charging from its last report, a button per task) and the states; `home_assistant.json` says connected.
- [x] The A71 connecting: its `connected` and `sessions` went to `ON` and `1` within 2 s, with `session_started` and `phone_linked` on the event topic (kind and phone only).
- [x] A task button (`ask = false`): ran, `task_started` / `task_finished` logged with the device `home-assistant`; an ask-first one held in `task_requests.json` for the owner; a button for no task did nothing.
- [x] The switch off in config.toml: `offline` published within 2 s and the status file removed; on again, connected again.
- [x] The broker stopped: the status said why within a second; started again, the bridge was back within 5 s.
- [x] Found and fixed: a `live_sessions.json` left by an earlier run was published as live sessions for 2 s.
- [x] After the review fixes, the same broker reached as `localhost`: connected after 2.1 s (a connect the old 2 s tick would have cut short); a planted retained config and state for a phone that is not paired were cleared on connect; a planted retained `PRESS` started nothing, a live one started the task once, and an empty retained message (clearing it) started nothing.

Owed:
- [ ] Home Assistant itself (its Mosquitto add-on): the entities on its device page, the Activity entity's history, a button press from its UI.
- [ ] The desktop Settings section on screen (the switch, the fields, the status line), a broker with a password, and Linux.

## 83. What the phone tells other apps (ecosystem 1f)

A71 → this Windows box, debug builds, 2026-10-16, "Tell other apps what happens" on (set in the app's preferences through `run-as`):
- [x] A screen session: `session_started` (`MODE=screen`); Back out of it: `session_ended` (`REASON=left`).
- [x] `pocketdesk link` to the phone: `link_received`, with no link in it.
- [x] `pocketdesk send --wait`: saved, and `file_received` with `MIME=text/plain`, `SIZE=16`, no file name.
- [x] The switch off: a whole session told nothing.
- [x] After the review fixes: `session_started` once the picture came, `session_ended` (`left`) on Back.

Owed:
- [ ] Tasker or MacroDroid receiving them (no automation app on the A71), a remote's events, a send's `FILES_SENT`, a task's `TASK_FINISHED`, a link lost (`REASON=lost`), and the Settings row on screen.

## 84. The reliability pass before ecosystem phase 2 (R6)

The A71 against this Windows box, LAN, the release build of `main` at 0461632, input off, 2026-10-16. The samplers log the server's CPU, memory, threads and handles each minute, and the phone app's memory and temperature (`dumpsys meminfo`, `dumpsys battery`); the phone was on USB power.

- [x] Idle, no phone: 1.35 % of one core over 570 s (0.68 % on 2026-10-14), private 9–10 MB.
- [x] An hour with one session from start to end: 5.3 % of one core on average, private 82 → 87 MB with two one-minute spikes, handles flat; the phone flat.
- [x] Wi-Fi off 30 s, 11 times: back by itself 7–15 s after the Wi-Fi each time.
- [x] **Once, the server crashed** (exit 139) as a reconnected session started; not again in 11 more. A crash now leaves `crash.log` (PR #93).
- [x] The server stopped for 20 s: back 10 s after it listened, no tap.
- [x] **The control link lagged the stream** (18 s, over 50 s), so a file sent then failed "phone not connected". Fixed (PR #94): the link 4 s after the stream, a 1 MB file sent and saved, MD5 matching.
- [x] Seen: two streams opened on a Wi-Fi return (both encoded for about 4 s).
- [x] Seen: a 1 Mbit/s Wi-Fi link (after many toggles) made the session reconnect every 30–60 s and a 150 MB download fail. In a test (R6e, `server/tests/slow_link_session.rs`), 1 Mbit/s held 5 minutes but at 5–12 fps; with R6f's delivery ceiling and frame-rate steps it settled at 30 fps and about 480 kbit/s.

- [x] A 150 MB transfer broken by a 15 s Wi-Fi drop on a healthy link: stopped at 109 MB when the phone's Wi-Fi took about 60 s to rejoin and its retries ran out; sent again, it continued from there, MD5 matching.
- [x] One stream on a Wi-Fi return after PR #96.
- [x] The Linux host (Azure VM, over the internet): a 19-minute session at 1.33 % of one core, 3 Wi-Fi drops back in 10–13 s, a server restart back in about 12 s, idle 0.25 %. Found there: the example config stopped the server, VAAPI chosen with no GPU (PR #95), and files to a phone on the internet road never arrive (phase 2).

- [x] What doubled the idle cost on Windows (R6b): the folder pollers and the mirror's rewrite each second led the harness's ranking. After R6c's watcher the whole idle server is at 0.53 % of one core (35 MB), 570 s after a 30 s settle (1.35 % in R6).
- [x] A crash (exit 139) stopping the server 35 s into a session: about one round in 7 to 10, inside an unloaded GraphicsCapture.dll. With the library pinned, 30 rounds and no crash, then 10 on the reviewed build. Owed: whether the crashes after a reconnected session's setup were the same.
- [x] R6g: overlapping sessions against the real capture and sound on this box (`server/tests/overlapping_sessions.rs`), a release build: 200 rounds, 400 sessions, each captured and with the sound open, no error, none died early, the server ending both after every round, no crash. Owed: a control run without the capture library pinned, and a toast per session.

Owed:
- [ ] The A71's reconnects on a slow link, reproduced: the VM with `tc qdisc add dev eth0 root tbf rate 1mbit burst 32kbit latency 400ms`, 10 minutes with no reconnect, then climbing back once it is removed (R6f).
- [ ] A download that waits for the network (next phase, R6i).
- [ ] The idle cost on Linux after R6c (the VM), and with R6d's clipboard watch.
- [ ] R6d on the VM (Sway) with a phone linked: `pgrep -c wl-paste` stays at 1 over a minute, `wl-copy` text reaches the phone within 2 s, and the watcher ends when the phone unlinks.

## 85. iroh 1.3, one endpoint per computer, files over the internet road (2a–2d)

The A71 against this Windows box, 2026-10-17, debug builds of the branches, input off. "The internet road" here: the phone's saved control and stream ports pointed at closed ones (the §64 method), so only iroh reaches the computer (a direct path on this LAN).

- [x] 2a, iroh 1.3: a WAN session and an internet control link came up ("Handling a control connection over the internet", the session's transport `wan`), 60 s.
- [x] 2b, red test: two relay-only endpoints with one key on one relay: the first never connects (30 of 30 pings lost), the second does; two with their own keys both answer every ping. The shared endpoint is unit-tested (one endpoint for a session and a control dial with one key, closed with the last).
- [ ] 2b on a relayed path: A71 → the VM with direct UDP from home dropped, session and control link both up 10 minutes ("WAN · relayed").
- [x] 2c/2d, computer → phone: a 50 MB file over the internet link, saved, MD5 matching.
- [x] 2c/2d, phone → computer: a 50 MB share, in Downloads in 11 s, MD5 matching.
- [x] A 150 MB offer broken by 15 s without Wi-Fi: the phone's retries continued it ("The phone had 27869840 of 157286400 bytes; sent the rest"), MD5 matching; the iroh control connection itself survived the drop.
- [x] The remote file browser's download and a folder fetch over the internet road. (2026-10-25, the OPPO in a session by the internet road against a throwaway server: the runner's `ad3-dates` downloaded a file and a folder from the remote browser, and `ad2-big-folder` a folder of 2000 files, all arriving.)
- [ ] 2e: a screenshot copied on the computer lands on the phone's clipboard over the internet link (and phone → computer, when an app on the phone can copy an image). (where: by hand)
- [ ] A real WAN (the VM): 500 MB resumed with TCP 1716 unreachable.
- [ ] The too-old message against a computer from before 2c. (where: other hardware) (a computer running a release from before 2c.)
- [x] Both Settings switches off: a send refused at once, with the reason. (seen 2026-10-24 on the OPPO against a throwaway server (S3) by the internet road with `internet_files = false`: refused at once; its words fixed in #288 to name the setting.)

## 86. Two computers: pairing, the link, the clipboard and files (2f–2h)

Two real servers on this box passed `server/tests/two_computers.rs` (2026-10-17): a forged certificate and a wrong PIN paired nothing, the code paired, B listed A as a computer with the narrow profile and held A's iroh key, the link came up over iroh, a copy crossed each way (stand-in clipboards), a 3 MB file each way byte for byte, Forget dropped the link. Owed on two real computers (Windows, the R6a throwaway instance for A so the installed server is untouched, and the Azure VM for B):

- [x] Pairing by B's pairing link (asked through `computers_out/`, as Settings does) across the internet: "paired", then "linked"; B's trust store gives A Clipboard, Files and Links only, and ask first (the Azure VM, Ubuntu 24.04, and this box's throwaway server, 2026-10-24).
- [ ] The same from the desktop app: the link pasted in Settings → Other computers, the outcome shown, B's Devices tab. (where: second computer)
- [x] The link came back after both servers restarted (the Azure VM, Ubuntu 24.04, and this box's throwaway server, 2026-10-24).
- [ ] The link comes back after the network drops ("linked" again within a minute). (where: second computer)
- [x] The clipboard both ways with "Clipboard with my computers" on on both, within a few seconds across the internet (this box's stand-in file, the VM's X11 clipboard) (the Azure VM, Ubuntu 24.04, and this box's throwaway server, 2026-10-24).
- [ ] Off on one, nothing crosses either way; a password manager's copy stays. (where: second computer)
- [ ] A 500 MB file each way from the Files tab, broken (the network off for 15 s) and continued, MD5 matching; `pocketdesk send <name> <file> --wait`. (where: second computer)
- [x] A folder from A to B (a subfolder kept, the empty file left out and said) and from B to A, each under one new folder, MD5s matching, across the internet (the Azure VM, Ubuntu 24.04, and this box's throwaway server, 2026-10-24).
- [ ] Unpair on A (Settings → Other computers): the link goes, and B's Devices page still lists A until unpaired there; after that, a pairing again works. (where: second computer)

## 87. KDE Connect devices (3a–3c)

The A71 with KDE Connect 1.36.0 (F-Droid) against a throwaway server on this box (port 1720), 2026-10-17:

- [x] "Find" in the desktop app (the request file, as the app writes it): the phone dialed in, the v8 handshake completed over TLS, and the phone listed this computer under Available devices; one link, held.
- [x] Found on the way: a broadcast every 2 s while looking made KDE Connect dial again at each one and drop its link; now once per Find.
- [x] 3b, pairing from here: the phone showed the same code (260B880B) and Accept paired it; from the phone ("Request pairing"): the same code (EA788244) here, and Allow (the request file) paired it. Unpairing from the phone removed the pin here; it reconnected paired only with its certificate.
- [x] 3b, ping: from here the phone showed "Ping!"; from the phone ("Send ping") the server took it.
- [x] Found on the way: a null body (a ping's) made KDE Connect drop the link; a new link from one device was dropped from the list by its old one going.
- [ ] The desktop app's Find button, list, Allow/Deny and Pair on screen.
- [x] Find again after the 3a review: the device connected (paired) and held for the minute, one link, no churn.
- [x] After the 3b review: Pair from here (the request file), the phone showed the same code, Accept there held it here as accepted with nothing pinned, then Confirm (Allow with the code) pinned it. A request left past 30 s lapsed and the phone's prompt went with it.
- [ ] Cancel after the device's Accept unpairs it on the phone; a second request from the phone while one waits shows nothing new. (where: other hardware) (KDE Connect's app.)
- [x] 3c, battery: the A71's KDE Connect sent it as the link came up (100 %, charging), kept in `phone_status.json`.
- [x] 3c, the clipboard from here (the clipboard stand-in file): pasted on the phone. Found: the watcher rested while only a KDE device was connected, so nothing went; fixed.
- [x] 3c, the clipboard from the phone ("Send clipboard" in KDE Connect): the computer's set.
- [x] 3c, files: with Files off, a 3 MB file from KDE Connect's "Send files" was refused and nothing written; with Files on (the permissions request), it arrived in Downloads (the stand-in folder), MD5 matching, marked as downloaded (ZoneId=3), listed in `received.json`.
- [x] After the 3c review: Ring (now `kdeconnect.findmyphone.request`) rang the A71, whose find-my-phone plugin turned on for this computer; stopped with FOUND IT.
- [x] Unpaired here while away (the pins edited with the server stopped), the A71 came back thinking itself paired and was told it is not: KDE Connect moved this computer to Available devices.
- [x] Found and fixed: the battery the A71 sent as it accepted was dropped while the owner confirmed; now held and kept at pairing (none before Confirm, 100 % right after).
- [x] Files again after the review's intake changes: Files turned on with the one-switch request, a 3 MB file kept as `pd-kde-3c (1).bin` beside the first, MD5 matching, nothing left in `kdeconnect_parts/`.
- [ ] 3c, a text or link share refused with its notice, the low-battery warning from a KDE device, and the switches, battery and Ring on screen.
- [ ] A device that broadcasts first (this computer dialing it): not built (the receive half waits). (where: other hardware) (KDE Connect's app.)
- [ ] Linux, with `kdeconnectd` holding UDP 1716.

## 88. The desktop app's shell (U1)

On this box, the app's own window captured (no input into it):

- [x] The navigation down the left in its groups; Settings at 720×480 scrolling, its heading and purpose shown.
- [x] File names in Hindi, Korean, Thai, Arabic, Chinese and Tamil drawn (they were boxes).
- [x] Windows UI Automation reads the navigation, the group titles, the page's heading and its text.
- [x] A second run opened on the page the first had shown.
- [x] Settings' Save bar after an edit, Undo taking it away and Save writing the file and saying "Saved." (C4's harness).
- [ ] A notice clearing after 6 s, "Copied.", and the two settings' switches while their change runs. (where: by hand)
- [ ] Narrator reading the app; the Linux build (Orca, the Noto fonts through fc-list).
- [ ] A line mixing Arabic or Hebrew with Latin (egui has no bidirectional layout), read by someone who reads it. (where: by hand)

## 89. The phone's navigation and feedback (U2)

On the A71, against this box and a throwaway server reached through `adb reverse` (phone-only taps; the throwaway's Input off for the phone):

- [x] Back from Settings, Add computer and the scanner to Home; from the pairing confirmation, cancelling it; from the remote, ending it.
- [x] A tapped computer that does not answer says "Checking whether it is awake…", and the internet try ends in about 16 s (31 s before).
- [x] A remote that could not connect says why in plain words.
- [x] Lock's question from the session's sheet and from the notification's "Lock desktop"; Cancel both times, and the server logged no privacy request.
- [x] The pairing sheet: the PIN field, Pair, "Show a PIN on the computer", Cancel; the eye says Show PIN and Hide PIN; paired with a PIN.
- [x] A wake's result as a snackbar that goes by itself; the scanner's Cancel and its hint.
- [ ] Lock confirmed, and refused with Power off (the server's answer is unit-tested): never against this box, so on the VM or a spare computer.
- [ ] Ring's snackbar (it rings the computer), and a shortcut key's failure message on a dropped link. (where: owner) (it sounds a siren on this computer.)
- [ ] TalkBack reading the snackbars and the checking card.

## 90. Names on both sides (U3)

On the A71, against a throwaway server named "Test Desk" (`COMPUTERNAME`) reached through `adb reverse`, and the desktop app's own window:

- [x] A computer's card shows its name after a connection, with its address and ports under it; Rename to "Lab box", and clearing it, back to its own.
- [x] The computer keeps the phone's name at pairing (Android's device name), and a name set in the phone's Settings, changed mid-run too (the link reopens), and the device name again once cleared.
- [x] `pocketdesk devices --json` gives `name`; the desktop app's Devices page shows the name over the id.
- [ ] Rename on the desktop's Devices page, and the approval prompt's "(calls itself)" (they need clicks).
- [ ] A computer's name from a scanned pairing code, the session notification and launcher shortcuts by name, and "Lock <name>?". (where: by hand)
- [ ] The Linux host (its kernel host name), and a phone whose Android name holds a person's name.

## 91. Ring the phone from the computer (N1)

On the A71, against a throwaway server reached through `adb reverse`; each ring kept to seconds:

- [x] "Ring from the computer" on, the app in the background and the screen off: `pocketdesk ring` rang it on the alarm stream within about 4 s.
- [x] It went by itself after 30 s; Stop from the shade silenced the next one at 8 s; the alarm volume was back as it was.
- [x] With the switch off, the computer sent the ring and the phone dropped it ("not rung"); the switch shows its change at once (it had not, with eight others).
- [x] After the review: with the remote open (and the background link up), a ring sounded through the remote's own watcher with its Silence dialog, the alarm stream at 7 of 7 while it rang and back at 6 after Silence, the notification gone.
- [ ] The desktop's Ring button on screen (it needs a click), Ring off in Devices refusing it, and once a minute from the button.
- [ ] Do Not Disturb with alarms allowed and not; a phone with a secure lock screen (the full-screen ring and Stop); a phone where the full-screen permission is not granted. (where: by hand)
- [ ] The Linux host.

## 92. Phone → computer text in one tap (N2)

On the A71, against a throwaway server with a stand-in clipboard (`POCKETDESK_CLIPBOARD_FILE`):

- [x] Android lists "To computer" among the apps that take selected text, and its intent put the text on the computer's clipboard.
- [x] The connect shortcuts carry the share category (long-lived); a share through a computer's shortcut id reached that computer, and one through an unknown id went nowhere.
- [x] After the review: the connect shortcuts' ids carry the token; a share through the real id reached the computer, one through the old `connect:<address>` form (as another app would forge it) went nowhere, and the selection menu's text still arrived.
- [ ] "To computer" in a real app's selection menu, on screen; the Sharesheet's Direct Share row showing the computers (adb opens only the app chooser).
- [ ] A copy marked sensitive (a password manager's) asking first through the tile. (where: by hand)

## 93. The phone's Home card, Add form, Settings and session menu (U4)

On the A71, against a throwaway server (`adb reverse`, input off for the phone):

- [x] Home's card: the name with its status line, Connect, Remote and Show screen on one row, the ⋮ menu with Remove last in red.
- [x] The Add form: the top bar's arrow goes back, the example address shows, the URI keyboard with Go, and the label toggles the switch.
- [x] Settings scrolled through in its groups; an ⓘ shows a summary in full without toggling its switch; Ring from the computer still off.
- [x] A session: no dot while connecting, "Good connection" once streaming over the internet road; a refused session shows none.
- [x] The session menu: the groups, segmented choices (Trackpad chosen, the chip followed), no chevron on Ring, Lock, Blackout and picture-in-picture.
- [ ] TalkBack on a card ("Connect"), a Settings row (one stop) and a heading.
- [x] "Show connection details" on, during a session: the numbers and the computer's battery (fps, kbps, rtt, delay, lost, "computer 55%⚡" and the caption) (2026-10-19, A71, throwaway server, by script).
- [x] A row's "Allow notification access" with a feature on and access off. (seen 2026-10-24 on the OPPO against a throwaway server (S3): access revoked with `cmd notification disallow_listener`, Settings offered the row, which opened Android's notification access page.)
- [x] Spanish and Tamil (the app's own language set by `cmd locale`): the card's buttons fit (Tamil wraps Show screen to a second row), and the four quality choices fit after the review.
- [ ] Landscape: the bottom chrome with 48 dp chips. (where: by hand)

## 94. The desktop's Settings and words (U5)

On this box through the app's own window (screenshots only), with a throwaway config and state:

- [x] Settings in sections, Connection and Phones open, each heading with what it holds; rows with their notes under them.
- [x] Devices: "Unpair…" and a battery's age as "9 min ago"; Files with nothing to send to says which phones and computers can take a file.
- [ ] Opening and closing sections; a change shown as applying now or after a restart in the save bar, and the restart notice naming it. (where: by hand)
- [ ] A narrow window wrapping every row; a section with a problem held open. (where: by hand)
- [x] Unpair's question for a phone, Cancel keeping it and Unpair removing it (C4's harness, `desktop/src/ui_tests.rs`).
- [ ] Unpair's question for a computer. (where: by hand)
- [x] Pair a phone and Overview buttons on the empty pages (C4's harness: Devices, Sessions and Files).

## 95. The phone app in Spanish and Tamil (U6)

On the A71 with the app's language set by `cmd locale set-app-locales` (reset after), against a throwaway server:

- [x] Tamil: Home's bottom row wraps to two lines; a session's chip says நேரடித் தொடுதல் as its menu does; the connection details in Tamil, clear of Exit after the fix.
- [x] Spanish: the details (retardo, perdidos, equipo, "tasas en 1,5 s"), the chip "Toque directo", and the shortcut bar's Ctrl+Alt+Supr and Impr Pant.
- [x] The keyboard bar's keys in Spanish: Ctrl, Alt, Mayús, Super, Esc, Tab, Inicio, Fin, Re Pág, Av Pág, Supr, Insert. (Seen 2026-10-24 on the OPPO against a throwaway server (S3).)
- [ ] The remote's presenter keys in Spanish. (where: owner) (A remote needs the Input permission.)
- [ ] Every screen in Spanish and Tamil for text that runs out of its box. (where: by hand)

## 96. Answer a phone notification in the toast itself (N3)

On this box with the A71 and a throwaway chat app (not committed) posting Reply and "Mark as read":

- [x] The phone's notification reached the computer as a toast whose delivered XML (read from the notification history) holds the reply box, Send and "Mark as read"; the server kept it answerable; clearing it on the phone took the toast back.
- [ ] Typing a reply in the toast and pressing Send: the phone's app gets it (its RemoteInput), at most 10 a minute. (where: by hand)
- [ ] Pressing "Mark as read" in the toast, and from the Action Center after the banner has gone. (where: by hand)
- [ ] Settings "Answer in the notification" off: the toast is plain. (where: by hand)
- [ ] Linux: a notification server with actions (mako's `makoctl invoke`, dunst's menu) and KDE's inline reply.

## 97. The phone as the computer's speaker and mic (N5)

On the A71 against this box (a throwaway server, the internet road):

- [x] "Sound" in the saved computer's menu opens a sound session: admitted, "sound only" in `pocketdesk status --json`.
- [x] The computer's sound decoded and playing on the phone (its `AudioTrack` started, not muted), going on with the phone's screen off.
- [x] The phone's mic share started and its frames reached the computer's mic relay (this box has no virtual cable: the relay said so).
- [x] With Audio and Mic off for the phone: refused, the phone naming the switches to turn on.
- [x] Disconnect in the notification ends it on both ends.
- [x] After the review fixes: with Mic off, the mic switch greyed with its reason; the computer's notification says "a phone is this computer's speaker and mic"; the server stopped and started again, the session counted down and came back with the sound playing; Lock desktop from the notification asks (cancelled); a change of permissions ends it with that reason, no retry, the service released.
- [x] Ten minutes without the computer: it stops trying and releases the service. (2026-10-25, the runner's `af4-sound-gives-up`, the media volume at 0: a sound session admitted, the computer stopped, the service released 10.9 and 10.8 min later.)
- [ ] By ear: the computer's sound on the phone (lag, quality, across the room).
- [ ] Windows with VB-CABLE: the phone's mic heard in a recording or a call on the computer.
- [ ] Linux: the computer's sound through PipeWire and the phone's mic as "Pocketdesk Mic" (`pw-loopback`).
- [ ] An hour with the screen off: the session holds, and what it costs the phone's battery. (where: by hand)

## 98. An Android 17 pass (N4)

Owed, on an Android 17 device or the API 37 emulator (the emulator here stalled under both software and host rendering, so none of this has run; the owner will check it):

- [ ] The app starts, asks for local network access, and a session works once it is granted. (where: other hardware) (Android 17.)
- [ ] Ring from the computer with the app in the background and `adb shell cmd audio set-enable-hardening enable`: the notification sounds at the alarm volume, the volume is not raised, and no `AudioHardening` entry names the app. (where: other hardware) (Android 17.)
- [x] On Android 16 and older (the A71): the ring still raises the alarm volume and puts it back. (seen 2026-10-24 on the OPPO against a throwaway server (S3) on Android 14 with #284: two rings raised it 6 to 16 and put it back to 6.)
- [ ] Settings shows the Android 17 notes beside "Ring from the computer" and "Show texts on my computer".
- [ ] A touchpad connected to the phone drives the computer's pointer and scrolls during a session: a tap and a press of a clickpad click, two-finger scroll goes the right way at a sensible step, and a game controller's touchpad does nothing unwanted.
- [ ] A text with a code: does it reach the computer at all after the three hours, and with its own time? (where: sim)
- [ ] A ring that raised the volume on Android 16, cut short, then the phone on 17: the volume is put back when the app is opened. (where: other hardware) (Android 17.)
- [ ] A screen, remote and sound session with the screen off: the computer's sound keeps playing (the session's foreground service holds while-in-use). (where: by hand) (by ear.)
- [ ] A tablet-size window: the session lays out, rotates and resizes (orientation limits are ignored there).

## 99. The phone panel and a Linux tray (N6)

- [x] Overview on this box with the A71 (a throwaway server): in a session, then linked with its last session; Send link… opens its box, Cancel closes it; Send file… opens Files with the phone chosen.
- [x] Linux tray in WSL on a private D-Bus session with a stand-in watcher: registered with its title, category and icon; the menu's three entries; a click on the icon, Pair and Quit reach the app; no watcher, no tray.
- [ ] Send link… sends a typed link from the Overview, and Ring rings from there.
- [ ] The tray in a real panel: KDE Plasma, waybar or swaybar, GNOME with the AppIndicator extension. Closing hides the window to it, Open brings it back, Pair opens Pair, Quit quits.
- [ ] A panel restarted while the window is hidden: the item comes back, and starting the app again raises the window.
- [ ] Wayland (Sway, Hyprland, KDE's Wayland session): closing the window quits; the tray's entries work while it runs.

## 100. GNOME and KDE hosts that work unattended (N8)

Owed, on a GNOME VM (Ubuntu 24.04, a GNOME session) and KDE where one allows; none of this has run:

- [ ] "Remember the screen-sharing grant" on: a session from the phone asks once at the desk; a second session and a reconnect start with no dialog.
- [ ] Without uinput access: the phone's taps, typing (accents, emoji), keys and the wheel (its direction) reach GNOME through the RemoteDesktop portal; a remote without the screen is told it gets no input there.
- [ ] Two phones at once, then one leaving: the other's picture and input go on.
- [ ] Revoking in GNOME's settings brings the dialog back; Forget the grant and turning the setting off make the next session ask.
- [ ] No RemoteDesktop portal (or refused): the screen alone.
- [ ] The server started as a systemd user service: does the portal keep its restore data with no desktop app id?

## 101. KDE Connect, next steps (N9)

On the A71 with KDE Connect 1.36 (F-Droid), paired with a throwaway server on this box, notification sync on:

- [x] A notification posted on the phone shows here as a toast (its app, title and text); closing it there takes the toast away.
- [x] Notifications already on the phone when it connects (marked silent) are not shown.
- [x] With the device's "Show its notifications here" off, nothing shows.
- [ ] A chat app's notification, an update to one (replacing the toast), and a flood held to the limit. (where: other hardware) (KDE Connect's app.)
- [ ] Linux: the notification over D-Bus, and closed by its id.

## 102. The phone's background cost (N7)

On the A71 against a throwaway server, with the clipboard, files and ringing on in the background, the screen off for an hour, USB in, the battery logically unplugged:

- [x] Before the cuts: 21.0 s of CPU in the hour (0.58 % of one core), no wake locks, no alarms, deep idle all hour.
- [x] After the cuts: 15.9 s (0.44 %), the same otherwise.
- [ ] The same hour on battery, without USB: the battery's drop, and the CPU while the phone really sleeps. (where: by hand)
- [ ] Over Wi-Fi rather than `adb reverse`: the radio's wakes and bytes for the link's keepalives. (where: by hand)
- [ ] A release build (R8): the CPU again. (where: by hand)

## 103. Transfers you can watch and stop (C1)

On the A71 against a throwaway server on this box, a 1.5 GB file over the internet road (the computer's ports out of the phone's reach), the phone in the background with files on:

- [x] A large file sent to the phone: "Under way" on the Files page with its bar and speed ("32 % · 10.2 MB/s"), and the phone's notification with the speed ("9.4 MB/s") and Cancel.
- [x] Cancel on the computer (the app's button, through UI Automation): the phone's retry was refused and it stopped ("cancelled on the computer"), and the Sent list says "cancelled on this computer".
- [x] Cancel on the phone (the notification's button): the computer's row went, the Sent list says "the phone declined it: cancelled on the phone", and the phone's transfer service stopped.
- [x] Sending the same file again continued from what arrived (615 MB), its speed starting from 0 rather than counting the jump, and was saved whole.
- [x] A folder cancelled on the computer: one Cancel stops all of it ("4 of 4 files not sent"), nothing more arrives, and no part is left on the phone (C6 fixed the part kept after the computer's Cancel; the phone's own Cancel keeps it, so a resend continues).
- [ ] A folder cancelled on the phone: the app's notifications sit folded under "Silent" in the shade, so its Cancel was not reached by script (a single 1.5 GB file's was, in C1). (where: by hand)
- [x] A share from the phone cancelled on the computer, then shared again. (seen 2026-10-24 on the OPPO against a throwaway server (S3) by the internet road: 150 MB cancelled through `transfer_cancels/` at 20 MB, the phone said "Not sent: 1 of 1" and "The computer refused the file: cancelled on the computer", nothing kept; shared again, whole in 16 s, MD5 matching.)

## 104. Control the phone from the computer (C8)

On the A71 against a throwaway server, the cast over the internet road through a relay, control sent by a test client that speaks the window's relay messages:

- [x] Home, Recents, Back and the shade.
- [x] A tap into Settings' search, text typed into its field, and Backspace and Enter; quick typing then Backspace.
- [x] A drag scrolled a list, the wheel scrolled it both ways, and a long press opened an icon's menu.
- [x] The service turned off: the phone said control is off within a second, and its notification changed.
- [x] Latency: 40 to 75 ms from a click to the first changed frame from a still screen; 85 to 200 ms to a transition's first large frame.
- [ ] The window by hand: a click, a drag, the wheel, the right and middle buttons, typing, the bar's buttons, Watch only, and a drag that leaves the window.
- [ ] The phone locked with a PIN: taps and typing refused, Back and Home taken. (where: by hand)
- [ ] A password field. (where: by hand)
- [ ] Over the home network (not the relay): latency again. (where: owner)
- [ ] Rotation: taps land right after the phone turns.
- [ ] Android 13+ sideloaded: the restricted-settings step.
- [ ] Advanced Protection (Android 17): the service refused.
- [ ] Linux.

## 105. Choose where received files land (C2)

On the A71 against a throwaway server, the phone in the background with files on:

- [x] A folder picked in the phone's Settings; a file from the computer saved there (checksum matching), the same again as `name (1)`, and a folder with a sub-folder arriving whole.
- [x] The picked folder deleted: the next file in Downloads/Pocketdesk, and the notification says why.
- [ ] The computer's chosen folder with a phone's share, a KDE device's file and another computer's; a folder that is gone; Settings' Choose folder… and Use Downloads.
- [ ] The phone's grant taken back in Android's settings, then a file. (where: by hand)
- [ ] A download broken and continued in a picked folder. (where: by hand)
- [ ] A picked folder on an SD card or a cloud provider.
- [ ] Linux.

## 106. Notifications that wait and go later (C3)

On the A71 against a throwaway server:

- [x] The computer stopped, a notification posted 2 s later, the computer back 65 s after: it arrives within 10 s, "Sent 1 min late", `late_secs` in its event.
- [x] A notification closed on the phone closes its toast at once.
- [x] An hour in forced deep idle (USB in): all 12 arrive within a second.
- [ ] Doze on battery without USB, where the phone suspends; a notification waiting through it and arriving marked late. (where: by hand)
- [x] Settings "When the computer is out of reach" chosen on screen (A71, by taps on the phone, 2026-10-19): Don't wait dropped a notification posted while the computer was away; Wait up to an hour kept one and sent it once the computer was back, its event saying 127 s late.
- [ ] "Wait up to 10 minutes" on screen, and one posted more than 10 minutes before the computer comes back.
- [ ] A computer from before C3: notifications arrive once each, as before. (where: other hardware) (a computer running a release from before C3.)
- [x] The computer's Notifications permission off: nothing waits on the phone. A notification posted then was refused (logged once) and did not arrive in the minute after the permission was turned on again (2026-10-19, A71, throwaway server, by script).
- [ ] Linux: the late line in a D-Bus notification.

## 107. The desktop app's headless harness (C4)

`desktop/src/ui_tests.rs` drives the app through AccessKit with `egui_kittest`, over throwaway state and config folders (`cargo test -p pocketdesk-desktop`):

- [x] Every page opens from the left, with its purpose (Pair in a test of its own).
- [x] Devices with nothing paired says how, and its button opens Pair; Sessions without a server sends the owner to Overview; Files with nothing to send to offers to pair.
- [x] Unpair… asks by name; Cancel keeps the phone, Unpair removes it from the trust store.
- [x] The Overview shows a paired phone with its battery, Ring, Send file… and Send link….
- [x] A change in Settings brings the save bar; Undo takes it away; Save writes `config.toml` and says "Saved.".
- [x] A one-off look: the Overview, Devices and Settings in both themes drawn on this box's GPU with the `snapshots` feature, then deleted (CI's are the ones kept).
- [x] The committed pictures drawn by CI's software Vulkan, and the comparison passing there (PR #131).
- [ ] Clicks through the app's own window that a harness cannot stand in for: the tray, drag and drop, the cast window.

## 108. The phone with large fonts, landscape and TalkBack (C5)

On the A71 (1080×2400 at 420 dpi):

- [x] Every clickable node on Home, the Add form, Settings, a session and its menu labelled and at least 48 dp each way (uiautomator's tree, which TalkBack reads; rows cut by a scroll edge aside).
- [x] Font scale 1.3: Home, Settings, a session and its menu fit.
- [x] Font scale 2.0: Home and Settings fit; the session menu's choices are radio buttons, one per line; no nodes overlap between the chrome's groups (uiautomator's bounds, at 1.0, 1.3 and 2.0).
- [x] On its side: Home fits, and a session's stream fills the screen with no trackpad dock.
- [x] The choice row at 1.0 and 2.0, with four options and with three, in a Compose instrumented test (`ChoiceRowTest`).
- [ ] A TalkBack walk by hand: Home, the Add form, Settings, the session menu (TalkBack's tutorial takes the screen when it is turned on from adb, and adb cannot read which node it reads).
- [ ] Split screen: Home and a session in half a screen. (where: by hand)
- [ ] Landscape with the keyboard open (§26). (where: by hand)
- [ ] Spanish and Tamil at 1.0, 1.3 and 2.0 (a long word such as "Individual" may already make a list at 1.0). (where: by hand)

## 109. The owed checks by where they run (C6)

- [x] `docs/owed-checks.md`, generated by `scripts/owed_checks.py`, held to the checklist by CI.
- [x] The batch run here on 2026-10-18 (each ticked in its own section): sound underruns (§25), the background clipboard's two switches (§65), a folder with no session (§67), a folder cancelled on the computer (§103). Found and fixed: a download cancelled on the computer kept its part on the phone.
- [ ] The rest of "here": what the S cycle (S1 to S5, 2026-10-23 and 24) did not reach, mostly long transfer and battery runs.

## 110. Recent photos on the computer (C7)

On the A71 against a throwaway server:

- [x] Three JPEGs pushed into DCIM/Camera: listed newest first with thumbnails; one fetched into the received files, checksums matching.
- [x] The grid with each photo's Save, Copy and Open (a picture only), in the desktop's headless harness.
- [ ] The Phone page by hand: Refresh, Save, Open and Copy. (where: by hand)
- [x] The phone's switch off: the computer keeps its reason, "Recent photos are turned off in Pocketdesk's Settings on the phone", with no photos (2026-10-19, A71, by script).
- [ ] Photo access denied: the same with its reason; and the page showing either (by eye).
- [x] A phone with a thousand camera photos: the newest 24 answered in 3 to 4 s (2026-10-19, A71, twice; the photos made for the test and removed after).
- [ ] HEIC photos: thumbnails shown, Copy saying it cannot. (where: other hardware) (a phone that saves HEIC.)
- [ ] Android 8 or 9: the legacy thumbnail path.

## 111. Health after updates (D1)

On the A71 (Android 16), no server needed:

- [x] An app update with nothing broken posts nothing. One with only what Home already showed posts nothing. One with something new (contact names without contacts access) posts one notification.
- [x] Screenshots on without photo access, and notification access off: both on Home's card. Fix opens Android's notification access page.
- [x] Access granted: the listener runs, and the screenshots switch turning itself off is said as such (found here: it had vanished without a word).
- [x] Not now hides the set, across a reopen. A new problem (control on, its service off) brings the card back.
- [x] After a force-stop and reopen, the listener is live again and nothing is called stopped.
- [x] Everything fixed: the card and the notification go.
- [ ] A real system update (the fingerprint path), and a listener that stays unbound after one.
- [ ] Advanced Protection on, with control on (needs the owner's Google account). (where: owner)
- [ ] A low-RAM phone on Android 10 or older: no "stopped" card.
- [ ] The card in Spanish and Tamil, and with TalkBack.

## 112. Quiet hours kept in step (D2)

On the A71 (Android 16) against a throwaway server, phone notifications shown nowhere (`phone_notifications = false`; arrivals read from its events):

- [x] With Do Not Disturb off, a notification reaches the computer. With it on, one it silences does not, and the computer sees the phone quiet, then not.
- [x] "Mute for 2 minutes" from `pocketdesk quiet`: the phone goes quiet, the computer sees it muted from here, and it ends by itself at its time.
- [x] The owner's own Do Not Disturb (set to alarms) during a mute stands, and an end then leaves it as the owner's.
- [x] Unmute ends a mute from here.
- [x] A mute asked while the owner's own Do Not Disturb is on leaves it the owner's: the phone stays quiet, not muted from here.
- [x] With "Let the computer mute this phone" off, nothing changes and the phone answers with a refusal.
- [x] Found here and fixed: the state never reached the computer (and paused the phone's notifications), Android 15's own mode outlived the mute, and a filter read too soon forgot every mute.
- [ ] The Overview's Mute and Unmute by hand, and the refusal's words. (where: by hand)
- [x] Android 14 and older (one filter for all, no app mode). (seen 2026-10-24 on the OPPO against a throwaway server (S4) on Android 14: `pocketdesk quiet` set `zen_mode` 1, the computer kept it as quiet by the computer with its end, `--off` put it back to 0; with the phone's switch off the mute was refused.)
- [ ] A Do Not Disturb that silences but still lists notifications. (where: by hand)
- [ ] A phone with a bedtime mode switching on during a mute. (where: by hand)

## 113. Clipboard history and Resend (D3)

On the A71 against a throwaway server with a stand-in clipboard (`scratchpad/d3_device.sh`, in the evening's batch):

- [x] An earlier copy sent from Home's card menu reaches the computer's clipboard, and the history keeps it as the phone's.
- [x] The computer's copy pushed to the phone is kept as this computer's.
- [x] `pocketdesk clipboard` lists both, newest first; `--send 2 --to` puts the older on the phone.
- [x] The history turned off in `config.toml` by hand is emptied at the next text and listed as none.

**Run 2026-10-19 (A71, throwaway server, the build of main with D3–D9):** all four above passed by script.
- [ ] The Clipboard page by hand: Copy, Send to…, Clear and its confirm, a cut entry's Send greyed. (where: by hand)
- [ ] A KDE Connect device's text, and another computer's, in the history.
- [ ] Linux (`wl-paste`).

## 114. Continue reading on phone (D6)

- [ ] Chrome (Windows): "Continue reading on phone" on a long article mid-way; the phone's notification opens it in Chrome scrolled to that paragraph. (where: by hand)
- [ ] Firefox: the same, the extension loaded from `about:debugging`.
- [ ] A page with a `#section` link: the fragment replaced, the page still scrolled there. (where: by hand)
- [ ] The browser's own page (`chrome://extensions`): sent at its top, no error. (where: by hand)

## 115. Conversations (D4)

- [ ] A real chat app on the A71: the thread on the Notifications page, oldest first, the owner's own lines set apart, a reply going through.
- [x] A second message with the same text: the thread grows.
- [x] A group chat: each sender named.
- [x] A one-time code arriving in a chat: no thread sent.
- [x] "Let the computer answer notifications" off: no messages sent.

**Run 2026-10-19 (A71, throwaway server, a throwaway chat app posting MessagingStyle threads):**
- The thread was kept with its notification, oldest first, the owner's own lines marked.
- A repeated message grew it.
- A group named each sender. Android titles a group's notification after the last sender, so the page heads it with that name.
- With a code in the thread, the phone sent the notification without its content, and the computer forgot the earlier note: the code reached the computer nowhere (notes, log, events).
- Answering off kept no note.
- Owed: a real chat app, the page by hand, a reply.

## 116. The tray you act from (D5)

- [ ] Windows, the A71 linked: its submenu titled with its battery, which changes in the menu when the battery does. (where: by hand)
- [ ] Ring, Mute for an hour and then Unmute, from the tray with the window hidden: the phone answers, the window stays hidden.
- [ ] Send clipboard: the computer's copy on the phone; a password manager's private copy not sent. (where: by hand) (the tray.)
- [ ] Send file…: the window opens on Files with the phone chosen. (where: by hand)
- [ ] The A71 away: its items greyed out. (where: by hand)
- [ ] A phone named with `&` (Windows) or `_` (Linux): the name as written.
- [ ] Linux: the same menu in a real panel.
- [ ] Send clipboard with an empty clipboard, and with over 256 KiB of text: a notification says it was not sent. (where: by hand) (the tray.)
- [ ] A tray pick with the server stopped, and with a server from before D5: nothing sent, and the notice says only that it asked.

## 117. The phone's folders as a drive (D8)

- [x] With the A71 sharing a folder: `curl -X PROPFIND -H "Depth: 1"` on the address lists the phone, its folders and their files.

**Run 2026-10-19 (A71, throwaway server):**
- A folder shared through Android's picker was listed through the drive: the phone, its folder, a sub-folder and the files with their sizes. The first listing took 1.7 s.
- A 3 MB file came in 1.0 s, its MD5 matching the phone's. Read again, it came in 0.1 s from the kept copy.
- A byte range answered 206, and a HEAD gave the size without fetching.
- A wrong token answered 404.
- Nothing landed in Downloads; the fetched files waited in `drive_cache/`.
- Owed: Explorer, which opens a window on the owner's screen, and everything below.
- [ ] File Explorer at the Windows address: browse, open a photo, copy one out. (where: by hand)
- [x] A file over 50 MB with curl: 60 MB in 10 s through the drive from the A71, its MD5 matching the phone's, nothing in Downloads (2026-10-19).
- [ ] A file over 50 MB in Explorer (refused by Windows' limit). (where: by hand)
- [ ] Explorer's repeated looks (`desktop.ini`, thumbnails) answered without a new listing each time (the phone's log).
- [ ] A phone from before D8: the file lands in Downloads, the drive answers 504 after the wait. (where: other hardware) (a phone on a release from before D8.)
- [x] The setting off: `drive.json` gone and the port closed within seconds, without a restart (2026-10-19, curl).
- [ ] The setting off: the address gone from the Phone page (by eye).
- [ ] Linux: Files or Dolphin at the `dav://` address.

## 118. Lock when the phone leaves (D9)

- [x] The A71 with the switch on, a throwaway server with `POCKETDESK_LOCK_FILE` set on this box: Wi-Fi off on the phone, a line in the file after the set time, and the `away_locked` event.
- [x] The same while typing on the computer: no line until the keyboard has been quiet that long.

**Run 2026-10-19 (A71, throwaway server, the seam checked before anything left; this box never locked):**
- The phone's link was dropped 35–45 s after its Wi-Fi went off.
- The computer was in use during the run. The first departure locked nothing within six minutes, the idle check holding it.
- The second wrote one line when the keyboard and mouse had been quiet for the set 2 minutes, 4 min 9 s after the drop.
- Once per departure: still to see with the computer untouched throughout.
- [x] The phone back, and gone again: a second line, 2 min after it left; staying gone 150 s more: no more (the OPPO, a throwaway server with the seam, 2026-10-24: home through `adb reverse`, away by the internet road alone).
- [x] The server takes the switch and the time without a restart: the switch turned off in the trust store while running, no line; the time set from 5 to 1 minute while running, a line at 2 minutes (the floor: the time is held to 2..60, as Settings offers) (the OPPO, a throwaway server with the seam, 2026-10-24: home through `adb reverse`, away by the internet road alone).
- [ ] The same set from the desktop app's Devices page and Settings. (where: by hand)
- [ ] A real lock, on a computer where locking is allowed (not this box), Windows; on Linux under GNOME, KDE and X11 (R3), and that it never locks on Sway or Hyprland.
- [x] A failed lock (the seam's file not writable): tried again a minute later, and `away_locked` with `failed`. (2026-10-25, the OPPO against a throwaway server whose lock is its own file, the runner's `ag1-failed-lock`, this computer never locked: the file read-only, the phone gone (home, then the internet road alone), the lock tried 1.3 min later and failed, `away_locked` with `failed`, tried again 60 s later; the file writable again, a line written. Twice.)
- [x] V1: a phone linked only by the internet road counts as gone: the OPPO linked through `adb reverse`, then (the reverse removed, the app restarted) only by the internet road; a line 121 s later, at the set 2 minutes (throwaway server with the seam, 2026-10-24).
- [ ] V1: a phone that walks off on mobile data, on a computer where locking is allowed: locked at the set time. (where: by hand)
- [x] A failed lock (the seam's path a folder): `away_locked` with `failed`, and tried again 65 s later, `failed` again (the OPPO, a throwaway server with the seam, 2026-10-24: home through `adb reverse`, away by the internet road alone).

## 119. Memory at rest (D7)

- [x] Windows: `scripts/memory_at_rest.ps1`, server and app at rest (2026-10-19: server 9.7 MB private, app 77–85 MB; the reliability spec's D7 section).
- [ ] Linux: the same by hand (`/proc/<pid>/status` VmRSS and RssAnon), the app in a Wayland session.
- [ ] The app with a phone linked and a session running, then at rest again: back to its number. (where: by hand)

## 120. Answers that say how they went (E1)

- [x] With the throwaway chat app: a reply from the computer (an answer request as the Notifications page writes it) is kept as "handed", and the chat app logged the reply (2026-10-19, A71, by script).
- [x] The notification gone (the reply cleared it) and answered again: not sent; the computer had already dropped it and said "the notification no longer offers that answer" (2026-10-19). The phone's own "gone from the phone", for an answer that overtakes the phone's word, is not reached this way.
- [ ] A desktop notification for such a failure.
- [x] Answering off on the phone (its Settings switch, the app running on): "answering is off on the phone" (2026-10-19).
- [ ] The phone away for more than a minute, its notification still kept: "the phone did not connect within a minute". (Run 2026-10-19 with the listener off: the phone never sent the notification, so the answer was refused at once as no longer offered, before any wait.)
- [ ] A phone from before E1: "sent, but no word from the phone" after a minute. Seen in passing 2026-10-19: an answer sent into a link the phone had just dropped (the app force-stopped) stays "sent", which the page shows as no word back. (where: other hardware) (a phone on a release from before E1.)
- [ ] From a toast (N3), the window hidden: a failure's desktop notification. (where: by hand)

## 121. The tray on its own (E2)

- [x] Windows: `scripts/memory_at_rest.ps1 -Tray`, the tray program at rest beside the server (2026-10-19: 2.1 MB private, 13 MB working set; the reliability spec's D7 section).
- [ ] Windows: starting `pocketdesk-app` starts `pocketdesk-tray` once (a second window launch starts no second tray); closing the window leaves the tray.
- [ ] Each item from the tray program: Ring, Mute and Unmute, Send clipboard; Open, Pair a phone… and Send file… with the window closed (it starts on that page, Files with the phone chosen) and with it open (it comes to the front on that page).
- [ ] With the window closed: a phone that must ask, an ask-first task, and a phone's "Show screen" each start the window (once each), with the prompt or the screen. (where: by hand)
- [ ] Quit from the tray program closes the window too; with no window open, an app started just after Quit stays open.
- [ ] An install without `pocketdesk-tray`: the app's own tray, as before.
- [ ] Linux: the tray program on a StatusNotifierItem host (or the WSL stand-in), and on a desktop with none (the window then quits on close).

## 122. The phone as a pen (E3)

- [x] Windows: the server's synthetic pen (its ignored test, `a_synthetic_pen_draws_a_stroke`) into a scratch window over every monitor that reads pointer messages (2026-10-19: type pen, enter, down at the expected pixel with pressure 313 rising to 938 of 1024, up, leave).
- [ ] Windows: a pen session from the A71 into that window: pressure following the touch, Down and Up once per stroke. (where: owner) (pen input on this computer.)
- [ ] A drawing app with pressure (Krita or Paint) on Windows: strokes thin and thick with the touch. (where: owner) (pen input on this computer.)
- [ ] A stylus (the A71 has none): hover moves the cursor without drawing, tilt and the side button reach the app.
- [ ] Pen pressure Soft, Even and Firm change the ink for one touch. (where: owner) (pen input on this computer.)
- [ ] A pinch while in Pen lifts the pen and zooms the view, and the stroke does not go on after it. (where: owner) (pen input on this computer.)
- [ ] Pen is not offered on a computer from before E3, and a session that meets one again goes back to Direct touch. (where: other hardware) (a computer on a release from before E3.)
- [ ] Control moving to another phone mid-stroke lifts the pen.
- [ ] Linux: the uinput tablet (Krita, or `libinput debug-events` showing a tablet tool), and the mouse fallback without uinput.

## 123. A gamepad on the phone (E4)

- [ ] The on-screen gamepad from the A71 into a scratch window that logs keys: each button's key down and up once, the D-pad's arrows, the left stick moving the pointer, the right stick scrolling.
- [ ] Closing it (× or Back) with a button held lets the button go. (where: owner) (game pad input on this computer.)
- [ ] A game (one that takes Enter, the arrows and Space) by hand. (where: owner)
- [ ] A real controller connected to the phone (none here): buttons, D-pad or hat, both sticks; Settings off leaves it to Android. (where: other hardware) (a game controller.)
- [ ] Two buttons at once, and a stick held while pressing a button. (where: owner) (game pad input on this computer.)
- [ ] Linux.

## 124. The desktop app's design pass (E5)

- [ ] Each page at the 640 px minimum width, in both themes: no row runs off the page (Devices, KDE Connect, Activity, Notifications, Phone files, Sessions). (where: by hand)
- [ ] The Phone page with a long folder: the photos below are in reach, and every photo row shows. (where: by hand)
- [ ] The cast window's bar in the light theme: its buttons readable. (where: by hand)
- [ ] KDE Connect's Unpair… asks by name; Cancel keeps it. (where: by hand)
- [ ] Devices: a paired computer has no Ring, Mute, cast or lock switches; the lock's note shows while it is on. (where: by hand)
- [ ] Notices: "Asked the phone…" on the Phone page goes in seconds; an error shows in red. (where: by hand)
- [ ] A screen reader (Narrator) names each photo's Save, Open and Copy, the pairing code and the phone's screen.
- [ ] The tray: a phone's title says Do Not Disturb or the mute's minutes left; Send the clipboard and Pair a phone.

## 125. The phone app's design pass (E6)

- [x] `ChoiceRowTest` on the A71: four choices and three at 1.0 and 2.0, and three that even crowded do not fit (2026-10-19, all five pass).
- [x] Home's ⋮ menu: Sound, Send from clipboard history, Wake, then Wake settings, Check connection, Rename, then Remove (2026-10-19).
- [x] Settings' Input group, Pen pressure as a row saving its choice, at font scale 1.0 and 2.0 (2026-10-19).
- [ ] The session menu with Pen offered: three segments at 1.0; at 2.0 crowded. (where: owner) (Pen is offered only where the computer takes pen input.)
- [ ] The gamepad in portrait and landscape on the A71 and a 360 dp phone; at font scale 2.0.
- [x] The history sheet from Home says a tap sends ("Tap one to send it to Test Desk") (2026-10-19, A71, throwaway server, by script).
- [x] The history sheet's entries 48 dp: 7 rows, each 144 px on the OPPO's 3x screen (the runner's `y3-history-sheet`, 2026-10-24).
- [x] No Clear when the history is empty. (2026-10-25, the runner's `al3-empty-history` on the OPPO: seven entries cleared through the sheet's Clear, confirmed; then the empty words, no Clear, and the clipboard icon above them, read from a screenshot and seen once by eye.)
- [ ] The Health card's Choose opens the folder picker. (2026-10-25: it shows only when a chosen downloads folder has lost its grant; the OPPO has none chosen, and choosing one is the folder picker, by hand. The card's Fix for Pocketdesk control did open Android's Accessibility settings.)
- [ ] TalkBack on the new rows (the mode chip's action, the gamepad's keys, the folders' Remove).
- [ ] Spanish and Tamil on the changed screens. (where: by hand)

## 126. The deck (F1)

Built 2026-10-20 on the owner's night of features; nothing here ran on a device yet.
- [x] Buttons of four kinds (keys, media, link, text) in `[[deck]]`, listed on the OPPO's Deck from Home and from the session menu by their labels; a media button named `play_pause` (not `playPause`) is left out. (Seen 2026-10-24 on the OPPO against a throwaway server (S4); none pressed, since input is off; the icons not read.)
- [ ] A task button, the deck set up in the desktop app's Settings, and the icons by eye. (where: by hand)
- [ ] Keys (`ctrl+shift+m`, `alt+tab`) into a scratch window here: pressed in order, all let go (input reaches this box: with the owner).
  - Checked 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server with a keys button of `ctrl+a` into a scratch window that logs each key: Ctrl down, A down, A up, Ctrl up, the text selected, nothing left held. `ctrl+shift+m` and `alt+tab` themselves not pressed.
- [x] Text typed into a scratch window; a task that asks first, the owner's Allow here; media Play/Pause and Next; a link opening in the browser.
  - Checked 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server: "hello" typed into the scratch window; the ask-first task held and run after Allow; Play/Pause and Next moved the computer's player; the link opened in the browser; with nothing playing, the media button's refusal said so.
- [ ] Each refusal on the button: Input, Tasks, Audio or Links off for the phone; "Deck" off; another phone driving.
  - 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server: Input off refused on the button. The other switches, "Deck" off and another phone driving not run.
- [x] A button edited after the phone listed it: "the computer's deck has changed".
  - Checked 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server.
- [ ] 2, 3 and 4 a row; a long label; Spanish and Tamil; TalkBack reads each button's label.
- [x] The Activity page (H1): a press reads "A deck button pressed (a media key)", or by its label with "Details in events" on, under "Tasks, deck and hooks".
  - Checked 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server, read through the app's accessibility tree.

## 127. How the computer is doing (F2)

Built 2026-10-20 on the owner's night of features; the computer's half read this box live in a test (memory and uptime), nothing on a device yet.
- [x] "How it is doing" on the OPPO against this computer: CPU 61 %, memory 17.3 of 29.9 GB, disks C:, D: and E:, the battery 59 % charging, uptime. (Seen 2026-10-24 on the OPPO against a throwaway server (S4); the CPU moving under load not watched.)
  - Checked 2026-10-20 by script on the A71 against the throwaway server (built from main): memory 25.9 of 29.9 GB, the four fixed disks (C:, D:, E:, G:), the battery 55 % charging and uptime 21 h 42 min, each matching Windows' own; the CPU shown (56 %), its movement under load not watched.
- [ ] The same against Linux: `df`'s disks once each (btrfs subvolumes, bind mounts), the battery.
- [x] "This computer's state" off in desktop Settings: the sheet says so, without a restart. (seen 2026-10-24 on the OPPO against a throwaway server (S4): "This computer does not share how it is doing. Its Pocketdesk app, Settings, This computer's state, turns it on.".)
- [ ] The sheet's asking stops when it closes; Spanish and Tamil; font scale 2.0. (where: by hand)

## 128. Take a photo with the phone (F3)

Built 2026-10-20 on the owner's night of features; nothing on a device yet.
- [x] "Take a photo with the phone" on the Phone page: the A71's notification "<computer> asks for a photo" with the app in the background.
  - Checked 2026-10-20 by script (the request written as the Phone page writes it): "Test Desk asks for a photo" within 2 s of the ask; a first ask sent before the phone linked was dropped by the computer ("the phone is not connected"), as designed.
- [ ] Its tap: the camera permission prompt the first time, then the camera; the photo in the received files and on the computer's clipboard.
  - 2026-10-20: the tap opened the camera (Aperture), the camera key took the photo, Confirm sent it; the computer refused both the file and the image with "will not connect back to 127.0.0.1", because `adb reverse` put the phone on loopback. The arrival is owed on the LAN road.
- [ ] "Also onto its clipboard" off: the file only; "Photos the computer asks for" off (the default): no notification, and the Phone page's button disabled while the phone is not linked.
- [ ] The camera left with Back: nothing sent; the activity killed while the camera runs (Don't keep activities): the photo still sent.
- [ ] Spanish and Tamil.

## 129. An air mouse (F4)

Built 2026-10-20 on the owner's night of features; nothing on a device yet (the pointer is this box's input: with the owner).
- [ ] Remote's "Air mouse" on the A71: held flat, turning moves the pointer across and tilting moves it up and down, a still hand moving nothing; the last choice kept.
- [ ] A tap clicks without moving the pointer; a press and turn moves without clicking; nothing moves after letting go.
- [ ] "Air mouse speed" at 0.5 and 3; a slow link (moves add up, no replay). (where: owner) (pointer input on this computer.)
- [ ] A phone without a gyroscope keeps the trackpad. (where: other hardware) (a phone without a gyroscope.)

## 130. Ringing calls on the computer (F5)

Built 2026-10-20 on the owner's night of features; needs a phone with a SIM.
- [ ] A call ringing on the phone: "Call from <name> (<number>)" on the computer within a second or two, in the toast and on the Notifications page; gone when it stops ringing (answered on the phone, declined, missed). (where: sim)
- [ ] Silence from the toast: the ring stops (or the refusal, said); the ringer's volume back after the call. (where: sim)
- [ ] Decline (with "Let the computer decline calls"): the call ends; with a call waiting, which one ends. (where: sim)
- [ ] The permission granted while notification access was already on: the next call shows (no restart). (where: sim)
- [ ] Two calls one after another; a call ending while its note is on its way. (where: sim)

## 131. The phone's camera on the computer (F6, part 1)

Built 2026-10-20 on the owner's night of features; nothing on a device yet.
- [ ] "Show camera" on the A71: the camera permission asked once, then the back camera in the computer's cast window, landscape, live; the notification's Stop ends it and the camera's indicator goes.
  - Checked 2026-10-20 by script (the permission granted with `pm grant`): camera 0 opened and streamed (CameraService), the computer took a cast session (mode 2, "the phone's screen is shown here"), "Showing the camera on Test Desk" in the shade; its Stop disconnected the camera, the computer shut the pipeline down and the notification went. The cast window itself not looked at (the throwaway server's relay has no viewer).
- [ ] "Show its screen or camera here" off for the phone: refused, with its reason.
- [ ] Stop pressed while the camera is still opening: the camera's indicator goes.
- [ ] Another app taking the camera: "The camera stopped", the cast ended.

## 132. The computer's state on the home screen (G1)

Built 2026-10-20 on the owner's night of features; nothing on a device yet.
- [x] Place "Pocketdesk: how it is doing" on the A71's home screen: "Asking the computer…", then the last computer's name, `CPU · Mem · Disk · Bat` and "Updated <time>".
  - Checked 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server: "Test Desk", CPU, memory, disk and battery, "Updated 9:43 AM".
- [x] A tap with the server stopped: "Not answering · from <time>", the numbers kept; a tap with it back: updated.
  - Checked 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server.
- [x] `share_state` off on the computer: the widget says it does not share.
  - Checked 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server: "This computer does not share how it is doing" at the next tap, with no restart; on again, the numbers back.
- [ ] Connect to another computer, then go Home: the widget follows it, asked once.
- [ ] "State widget refreshes by itself" off: no ask in the logcat at the next period; on mobile data with it on: none either.

## 133. Take a photo from the tray and the Overview (G2)

Built 2026-10-20 on the owner's night of features; nothing by hand yet.
- [ ] With the A71 linked and "Photos the computer asks for" on: the tray's Take a photo under it, then the phone's notification, the camera, and the photo in the received files; the same from the Overview's button, with its notice.
  - 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server: the Overview's Take a photo, once the phone linked (5 s after the switch went on), brought "Test Desk asks for a photo" within 4 s, and its notice. The tray's menu (in the hidden-icons overflow) and the photo's arrival (refused on `adb reverse`'s loopback, §128) not run. Found first: a phone in a remote alone, with no control link, was offered Take a photo and Send file, and the ask dropped while the page said asked; fixed in `fix/files-reach-needs-link`.
- [ ] The phone away, or Files off for it: Take a photo greyed in the tray and on the Overview, its hover saying why.
  - 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server: greyed on the Overview with the phone away. Its hover, Files off and the tray not run.

## 134. The deck on the home screen (G3)

Built 2026-10-20 on the owner's night of features; nothing on a device yet. Presses that type or press keys reach the computer: with the owner's OK.
- [x] Place "Pocketdesk: deck": its first six buttons, as the deck sheet shows them; ↻ asks again.
  - Checked 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server: the six buttons in the config's order; ↻ asked again and cleared an old line.
- [x] With the app swiped away: a media or link button pressed from the widget runs on the computer, the line saying "<label>: Done" (the bridge started by the tap).
  - Checked 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server, the process killed with `am kill`: "Open example: Done", the link opened. (`am force-stop` cancels the widget's buttons until it is drawn again: a test artifact.)
- [ ] The server stopped: "No answer from the computer yet" after 8 s, never a stuck "Asking…". (where: by hand) (a widget on the home screen.)
  - 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server: on `adb reverse` a stopped server closes the tunnel during the handshake, which read "Security credentials no longer match. Pair again" (fixed in `fix/handshake-eof`: it reads as nothing answering). The 8 s wait owed on the LAN road.
- [ ] Connect to another computer, then press a button still drawn for the first: nothing is sent, the widget redrawn for the new one.

## 135. A laser pointer for the air mouse (G4)

Built 2026-10-20 on the owner's night of features; nothing on a device yet. The dot is drawn on this box's screen: with the owner there.
- [ ] Remote on the A71 with the air mouse on: "Laser pointer" under the pad; on, a press shows a red dot in the middle of the pointer's screen, turning moves it, the pointer stays put.
  - 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server: the switch under the pad; a press showed the dot window (26 px) at the centre of the pointer's screen, the pointer where it was. Turning not run (it needs the phone in a hand).
- [x] Held still for 5 s: the dot stays; let go: it hides. Pressed again quickly: the dot is not hidden mid-gesture.
  - Checked 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server, the window's visibility polled every 50 ms: shown at the press, still after 4 s held still, hidden at the release; two presses back to back kept it shown through both.
- [ ] Two screens: the dot crosses where they meet and stops at an edge, never between them; larger on a 4K screen.
- [x] Desktop Settings "Laser pointer from phones" off, after a restart: the switch is not offered.
  - Checked 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server (`laser_pointer = false`, the server restarted, Retry): no switch, the pad's plain hint.
- [ ] A slide show (PowerPoint or a browser's): a tap with the laser on still advances or clicks at the pointer.

## 136. The computer's music on the home screen (G5)

Built 2026-10-20 on the owner's night of features; nothing on a device yet.
- [ ] Place "Pocketdesk: playing" with music playing on the computer: its title and artist, Previous, Pause and Next as the player offers them, "Read at <time>". (where: by hand)
- [ ] Pause from the widget: the music stops, and the button turns to Play; Next: the next track shown. (where: by hand)
  - 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server: Pause stopped the player and the button turned to ▶ Play. Next not run (one track).
- [ ] Nothing playing: "Nothing is playing", no buttons; Audio off for the phone on the computer: the refusal, no old track.
  - 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server: Audio off gave the refusal with no old track. Nothing playing not run.
- [ ] The server stopped: "No answer from the computer yet" after 8 s, never a stuck "Asking…". (where: by hand) (a widget on the home screen.)

## 137. The computer's music from Quick Settings (G6)

Built 2026-10-20 on the owner's night of features; nothing on a device yet.
- [ ] Add "Computer's music" to the A71's Quick Settings; with music on the computer, a tap pauses it, the shade still open showing "Asking the computer…", then the track, the tile off; a tap again plays, the tile on. (where: by hand)
  - 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server, read through `dumpsys activity service SystemUIService`: the track as the subtitle, and a tap turned the tile from on (2) to off (1).
- [ ] On the lock screen: a tap asks to unlock first. (where: by hand)
- [x] Audio off for the phone on the computer: the refusal as the subtitle.
  - Checked 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server.
- [ ] Opened after an hour: "Tap to play or pause", not the old track. (where: by hand)

## 138. Wake the computer from Quick Settings (G7)

Built 2026-10-20 on the owner's night of features; nothing on a device yet. Waking is a magic packet, never a power action on this box; a computer really asleep is owed elsewhere.
- [ ] Add "Wake computer" to the A71's Quick Settings: its subtitle names the last computer; with its MAC not known yet, "Connect once while it is awake". (where: by hand)
  - 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server: the subtitle "Test Desk". The unknown MAC not run.
- [ ] A tap with the computer awake: "Sending the wake…", "Sent, waiting for it to answer…", then "It is awake"; a tap again at once is ignored. (where: by hand)
  - 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server, with the shade open and read through `dumpsys`: "It is awake". The steps between and a second tap not seen.
  - 2026-10-20 by script: a tap sent the wake ("Sent the wake to 2 of 2 targets", twice). Its words are not seen: Android 16 draws the tile compact, with no subtitle; at its large size by hand.
- [ ] On the lock screen: a tap asks to unlock first. With no computer saved: a tap opens the app. (where: by hand)
- [ ] A computer asleep (not this box): woken, "It is awake" within a minute. (where: second computer)

## 139. A talk timer in Remote (H2)

Built 2026-10-20 on the owner's night of features; nothing on a device yet.
- [x] Remote on the A71: "Timer", 5 minutes: the count down, a buzz at one minute (twice) and three at the end, then "+0:05" in red; a tap stops it.
  - Checked 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server: the count down, the buzzes asked for at one minute left and at the end (alarm usage, `dumpsys vibrator_manager`), "+0:25" in the error red, a tap stopping it. Felt: no, the A71 has vibration off (`vibrateOn = false`, ringer silent), so Android drops every app's buzz.
- [ ] The screen off, the phone in a pocket: the buzzes still come on time (the session's foreground service). (where: by hand)
- [x] The link dropped and back mid-talk: the timer still counting; rotated: the same.
  - Checked 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server: the server restarted under a 10-minute timer, Retry, and a turn to landscape and back; it counted on. Found: a remote does not reconnect by itself after a drop (it waits for Retry), unlike a screen or sound session.
- [ ] Do Not Disturb on: the buzzes still come; Settings "Talk timer buzzes" off: none. (where: by hand)
- [x] Landscape and font scale 2.0: all seven lengths reachable; the header not crowded.
  - Checked 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server: the list scrolls to 60 minutes; the header holds Trackpad, Keyboard, Exit and Timer. (Changing the font scale restarted the remote.)

## 140. Send the clipboard to the phone with a key (H3)

Built 2026-10-20 on the owner's night of features; nothing by hand yet. It registers a global key on this box: with the owner there.
- [ ] Settings "Send the clipboard with a key" = `win+shift+c`, Save: "Registered." under it; with the A71 linked, copy a text on the computer and press it: the text on the phone. (where: owner) (a key pressed on this computer.)
  - 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server: another program on this box holds Win+Shift+C (`RegisterHotKey` fails with 1409); `ctrl+alt+shift+p` registered within 5 s. A press while the A71 was only in a remote found no phone, as it should. The text arriving not run.
- [ ] A combination another app holds: "Another app holds this combination"; freed, registered within 30 s. (where: owner)
  - 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server: the tray program's status said taken for Win+Shift+C. The words in Settings not seen: the app keeps the field it loaded, and a key typed into it from outside did not land.
- [x] No phone in reach: a press says "the last press found no phone".
  - Checked 2026-10-20, with the owner's OK, by script on the A71 against the throwaway server: "Registered. The last press found no phone the clipboard reaches."
- [ ] A password manager's copy: not sent (the server's notification says why). (where: owner) (a key pressed on this computer.)
- [ ] The tray's menu (H4): the phone it sends to shows "Send the clipboard (Win+Shift+C)"; with the key cleared, the plain label again.
- [ ] Linux (R4): the key and the typing key through the GlobalShortcuts portal on KDE Plasma 6 and GNOME 48 (the desktop's question once, then a press sending), and Settings saying "no global shortcuts portal" on Sway.

## 141. The home-screen widgets, a design pass (I2)

Built 2026-10-20 on the owner's word; checked on the A71 as it was built (against the throwaway server, stopped for the last checks):
- [x] The four widgets in dark mode with the system's colours, and in light mode: text, tiles and the warning readable in both; the warning's colour following a switch from light to dark.
- [x] The state widget at two cells: four bars with their percents and "Not answering · from <time>" in the warning colour; the bars' track seen in both modes.
- [x] The music widget refused: its warning icon and the reason in its middle; before its first draw it reads "Asking the computer…".
- [x] The deck's buttons with their kinds' icons, centred with their labels; before its first draw it reads as asking.
- [x] The picker's previews on Android 16 for all four, at four cells wide (two tall for the state, three for the music and deck).
- [ ] The music widget with a track: title, artist, the buttons at 56 dp and "Read at <time>" in its header; nothing playing. (where: by hand)
- [ ] The state widget made one cell tall: the line, no "%" left alone; on its side at two cells: the line where bars do not fit. (where: by hand)
- [ ] Android 11 or older: the pine theme in both modes, no previews, the state widget's layout by its height.
- [ ] Spanish and Tamil (the bar labels at 68 dp); font scale 2.0. (where: by hand)

## 142. The desktop app, upgrades and UI tweaks (I3)

Built 2026-10-20 on the owner's word; tested through the harness and units, nothing by hand yet.
- [ ] Messages: half-type a reply to one conversation, open another, then New text: each its own draft; Enter sends, Shift+Enter a new line. (where: by hand)
- [ ] Settings: change a key by hand in `config.toml` with the app open, then change another in the form and Save: the hand edit stays. Ctrl+S saves.
- [ ] The notice at the foot of the window, the page not moving; a long one readable; Restart's spinner. (where: by hand)
- [ ] The page list: a count on Sessions in a session, on Files while a file moves, a warning dot on Settings while it has changes; readable selected and hovered, light and dark. (where: by hand)
- [ ] Files: Send again on a failed send (and disabled with the phone away); Open on a received picture and PDF, none on a program; the rows wrapping at the minimum width. (where: by hand)
- [ ] Overview: a phone's battery bar, in the warning colour at 15 % or under not charging; with pairing off, the warning and Open Settings. (where: by hand)
- [ ] Diagnostics: the checks running as it opens, the chips in both themes. (where: by hand)
- [ ] Pair: "For 15 minutes" makes a new code; a phone paired with it shows its time left on Devices and goes when it runs out. (where: by hand)
- [ ] Phone files: the spinner while a folder is asked for. (where: by hand)
- [ ] Keys: Esc on Unpair, Restart and Clear; Ctrl+Page Up and Down; Enter in the Files and Notifications boxes.

## 143. The Logs page (J1)

Built 2026-10-20; tested through the harness and units, nothing by hand yet.
- [ ] A debug-level log of several megabytes open on the page: no hitch while it grows, the newest lines at the foot. (where: by hand)
- [ ] The filter and "Warnings and errors only", each alone and together, and the count beside them. (where: by hand)
- [ ] Copy pastes the lines shown; Open log folder opens the folder at the file (Windows, then Linux).
- [ ] Just after midnight, today's log still empty: the page shows the controls and "0 of the last 0 lines". (where: by hand)

## 144. Find a setting on the desktop (J2)

Built 2026-10-20; tested through the harness and units, nothing by hand yet.
- [ ] Ctrl+F with Settings scrolled down: the box in view with the cursor in it. Esc empties it, and a Restart question open at the same time stays open.
- [ ] Searches for "browser", "file manager", "computers" and "kde": each extra shown alone in its section. (where: by hand)
- [ ] A setting with a problem, then a search that does not find it: its section still there, whole. (where: by hand)
- [ ] The page at the minimum width, light and dark. (where: by hand)

## 145. The Sessions page in blocks, with the recent ones (J3)

Built 2026-10-20; tested through the harness and units, nothing by hand yet.
- [ ] A live screen session: its block with the phone, "on this network" and "screen" chips (the address on hover), how long, End. (where: by hand)
- [ ] Two phones: "driving" on one, Give control on the other. A cast with its window closed: Show screen. Music on the phone: Previous, Play/Pause, Next inside its block.
- [ ] The page at the minimum width: the chips wrap whole, not as tall slivers. (where: by hand)
- [ ] After a session ends: it heads "Recent sessions" with its length, its road and "capture to arrival"; a refused phone reads "A device claiming to be …", its retries one line. (where: by hand)

## 146. Search on Activity and Clipboard (J4)

Built 2026-10-20; tested through the harness and units, nothing by hand yet.
- [ ] Activity with a few hundred events: Search by a phone's name, a file's name and a link's host; with a kind chip; nothing found, then Clear search. (where: by hand)
- [ ] Clipboard: Search by a text and by "From …"; with Clear's question open, Esc in the box empties it and leaves the question. (where: by hand)
- [ ] Ctrl+F on each page, scrolled down; both boxes at the minimum width, light and dark.

## 147. Dropped files ask where they go (J5)

Built 2026-10-20; tested through the harness and units, nothing by hand yet.
- [ ] One phone linked: a dropped file goes to it at once, one notice. (where: by hand)
- [ ] Two (a phone and another computer, or two phones): "Send 3 files to:" with a button each; each button sends all three, one notice; Cancel and Esc send nothing.
- [ ] A second drop while it asks joins the files; a folder reads as a folder. (where: by hand)
- [ ] Leaving Files, or hiding the window, while it asks: "Not sent". The phone leaving while it asks: one button, or "Not sent" with none left. (where: by hand)

## 148. A sound session notices a computer gone quiet (J6)

Built and checked 2026-10-20 on the OPPO against the throwaway server.
- [x] A sound session held a minute over the LAN.
- [x] The server stopped: "the computer stopped answering" in 14 s, then reconnecting.
- [x] Back by itself 13 s after the server returned, with the phone's screen off.
- [ ] Nothing playing, the screen off, an hour: never ends on its own. (where: by hand)
- [ ] Over a relayed internet path: the same 14 s; a Wi-Fi roam over 12 s reconnects. (where: away)

## 149. Files keep their dates (K1)

Built and checked 2026-10-20 on the OPPO against a throwaway server built from the branch.
- [x] A file dated June here, sent to the phone: dated June there, by `stat` and in MediaStore.
- [x] The same file shared from the phone: dated June here.
- [ ] A file downloaded from the remote file browser, and a folder each way. (2026-10-25, to the phone: the runner's `ad3-dates`, a file and a folder's file dated 2020 kept their dates on the OPPO. A folder to the computer is owed: its picker is by hand.)
- [ ] A folder picked under "Files from computers go to": the file keeps its arrival date; Settings says so. (where: by hand) (the phone's folder picker.)
- [x] "Keep files' dates" on the computer: a file dated 2020 sent to the phone kept 2020; with `keep_file_dates = false` it was dated as it arrived. (Seen 2026-10-24 on the OPPO against a throwaway server (S4).)
- [x] "Keep files' dates" off on the phone: a share to the computer dated as it arrives. (2026-10-24, the runner's `ac4-dates`, the fixture's file dated 2020: with the phone's switch on, kept with the computer's on and dated as it arrived with the computer's off; with the phone's off, dated as it arrived either way.)
- [ ] Android 10, and a KDE Connect device's share.

## 150. Not from this app (K2b)

Built 2026-10-20; tested through units and the harness, nothing by hand yet.
- [ ] A phone notification's toast on Windows: "Not from this app" last among its buttons; pressed, a notification says the app is muted, and the app's next notification shows no toast but is on Activity, "muted here". (where: by hand)
- [ ] An answerable notification from a muted app: still on the Notifications page. (where: by hand)
- [ ] Settings → Notifications and texts → Muted apps: Unmute, and the app's toasts again.
- [ ] Linux: GNOME and KDE show the action; a server without actions shows none.

## 151. The phone's volume keys for the computer (K4)

Built 2026-10-20; tested by JVM tests, nothing on a phone yet. Each press sets this computer's volume, so run it with the owner.
- [ ] Remote open: the volume keys and the panel's slider move the computer's volume, one step a press; a held key stops when let go. (where: owner) (it sets this computer's volume.)
- [ ] The Sound sheet moves the volume, then a key press: one step from there, no jump. (where: owner) (it sets this computer's volume.)
- [ ] The screen off: the keys still the computer's. A call: the keys are the call's. (where: owner) (it sets this computer's volume.)
- [ ] A sound session: the keys stay the phone's. The setting off: the keys stay the phone's. (where: owner)
- [ ] `dumpsys media_session`: the session remote, and which session takes the keys on the OPPO and the A71. (where: owner) (a remote needs the Input permission.)

## 152. Pause the phones on the computer (K2a)

Built 2026-10-20; tested through units and the harness, nothing by hand yet.
- [ ] The tray's Pause → For an hour: the menu says "Paused, 60 min left" with Resume, and the Overview says what is held.
- [x] While paused: a phone's chat notification showed no toast (one did before the pause) and was kept to answer. (Seen 2026-10-24 on the OPPO against a throwaway server (S4), read from Windows' notification store.)
- [ ] While paused: a text on Messages without a toast. (where: sim)
- [ ] While paused: a link not opened, and a phone ringing this computer silent. (where: owner)
- [x] A session and a file send still work while paused. (seen 2026-10-24 on the OPPO against a throwaway server (S4): paused, a session opened and a file sent to the phone in it arrived ("sent").)
- [ ] Resume from the tray and from the Overview; `pocketdesk pause --for 1` ends by itself in a minute.
- [ ] Linux: the pause in the StatusNotifierItem menu.

## 153. Notifications with their app's icon, and pictures (K3)

Built 2026-10-20; tested through units and the harness, nothing on a device yet.
- [ ] A phone notification's toast on Windows shows the app's icon as its logo (on screen, not only in the XML).
- [ ] With "Pictures in notifications" on in the phone's Settings: a notification with a picture (BigPicture, or a contact's photo) shows it in the toast; one the lock screen hides shows none. (where: by hand)
- [ ] The Notifications page shows each answerable notification's app icon. (where: by hand)
- [ ] Closing the notification on the phone removes its picture from `notification_icons/pictures/`. (where: by hand) (a notification with a picture.)
- [ ] Desktop Settings "Apps' icons and pictures" off: plain toasts. (where: by hand)
- [ ] Linux: GNOME and KDE show the icon; a server with body images shows the picture.

## 154. The phone's music in the computer's media controls (K7)

Built 2026-10-20; the media controls read back on this Windows box by a live test, nothing with a phone yet.
- [ ] Settings "The phone's music in this computer's media controls" on; a phone in a session plays music: the volume flyout shows its song as Pocketdesk, with its buttons. (where: by hand)
- [ ] Play/Pause, Next and Previous from the flyout reach the phone; the flyout shows the new state at once. (where: by hand)
- [ ] The keyboard's media keys (input on this box: with the owner). (where: owner)
- [ ] The session ends: the song leaves the flyout within a few seconds. Paused five minutes: it leaves. (where: by hand)
- [ ] The setting off: it leaves at once. (where: by hand)

## 155. The phone's sound with its screen (K5)

Built 2026-10-20; tested through units, a loopback cast and the WAV seam, nothing on a device yet.
- [ ] Phone Settings "The phone's sound with its screen" asks for the microphone permission; desktop Devices "Play its sound here" on. (where: by hand)
- [ ] "Show screen" with a throwaway app playing a 1 kHz tone: a throwaway server with `POCKETDESK_PHONE_AUDIO_FILE` writes it; its pitch and level match. (where: by hand) (a tone app on the phone.)
- [ ] By ear: a video on the phone plays on the computer's speakers, behind the picture by how much.
- [ ] With another app in front on the phone (the use case): the sound still comes. (where: by hand) (by ear.)
- [ ] "Play its sound here" off mid-cast: the cast ends. Without it: the phone does not capture. (where: by hand)
- [ ] Linux: the sound through `pw-cat`.

## 156. Cleared on the computer, cleared on the phone (L1)

Built 2026-10-20; tested through units, the harness and the server's mapping, nothing on a device yet.
- [x] The throwaway chat app posts a notification; a clear as the Notifications page writes it (`notification_clears/`): gone from the phone's `dumpsys notification`. (Seen 2026-10-24 on the OPPO against a throwaway server (S4). A clear arriving after the listener restarted is refused as not sent: the phone keeps what it sent in memory.)
- [ ] The toast's ✕ on its banner, and inside the Action Center: cleared on the phone (by hand). (where: by hand)
- [x] An ongoing notification (the fixture's progress) not cleared: the phone logged `NotClearable`. (Seen 2026-10-24 on the OPPO against a throwaway server (S4).)
- [ ] A ringing call not cleared. (where: sim)
- [x] Phone Settings "Let the computer clear my notifications" off: not cleared, the phone logged `Off`. (Seen 2026-10-24 on the OPPO against a throwaway server (S4).)
- [x] Desktop "Clearing here clears it on the phone" off: not cleared. On the OPPO with the fixture app's notification and a throwaway server: with `clear_on_phone = false`, a clear written as the Notifications page writes it was taken and dropped, and the notification stayed on the phone; turned on again while the server ran, the next clear removed it (2026-10-24).
- [ ] Linux (GNOME): closing a notification clears it; pressing its button does not.
- [x] 2026-10-21, on the OPPO (CPH2359) against a throwaway server: a shell notification sent to the computer, then cleared there through `notification_clears/` as the Notifications page writes it, was gone from the phone within 8 s.

## 157. Drop files and paste into the phone's window (L2)

Built 2026-10-21; tested through units, nothing on a device yet.
- [ ] With "Files from the computer in the background" on: a 3 MB file dropped on the phone's window; its MD5 on the phone matches. Without it: the window says why. (where: by hand)
- [ ] With control on: Ctrl+V into a scratch field, read back with `uiautomator dump`; a long paste says it was cut.

## 158. A phone charged to a level (L3)

Built 2026-10-21; tested through units, nothing on a device yet.
- [ ] Desktop Settings "Tell me when a phone has charged to" 80 %; with Share battery on, `adb shell dumpsys battery set ac 1`, `set level 79`, then `set level 80`: one notification, "charged to 80 %"; `set level 81`: none. (where: by hand) (the phone reads its battery from the hardware, which `dumpsys battery set` does not move.)
- [ ] `dumpsys battery unplug`, then `set ac 1` again at 80: one more. (where: by hand) (a real charge.)
- [ ] `adb shell dumpsys battery reset` after. (where: by hand)
- [x] 2026-10-21, on the OPPO in a session: with `charged_alert = 80`, a charge seen at 70 % then 85 % raised "charged to 85 %" once (the level faked with `dumpsys battery set`). In the background the report stayed stale until the BatteryManager fix (see §162).

## 159. Call a number from the computer (L4)

Built 2026-10-21; tested through units and the harness, nothing on a device yet.
- [x] Phone Settings "Calls from the computer" on; in a session, `pocketdesk call` with +1 555 0100: the dialer opened in front with the number, no call placed (no SIM). (Seen 2026-10-24 on the OPPO against a throwaway server (S4); the Messages page's Call by hand.)
- [ ] With the app behind another, or outside a session with a background link: the notification, its tap opening the dialer.
- [x] A number with `*` or `#`: `pocketdesk call … *#06#` refused on the computer, "not a phone number: digits, with a + only at the start". (Seen 2026-10-24 on the OPPO against a throwaway server (S4).)
- [ ] A real call (a SIM): the owner presses Call.
- [x] 2026-10-21, on the OPPO with only a background link: `pocketdesk call` offered "Call 5550102?" in the phone's notification, no call made. One of three test numbers sent during a link's reconnect never showed.
- Seen on the OPPO (ColorOS 14): with the screen off, ColorOS froze the app's background threads even with Android's battery optimization lifted, so numbers and commands waited until the screen woke, then arrived at once. ColorOS's own "Allow background activity" is what the Home card's dontkillmyapp link covers.

## 160. Screenshot and record the phone's window (L5)

Built 2026-10-21; tested through units and a real ffmpeg recording, nothing on a device yet.
- [ ] Screenshot: a PNG in Pictures\Pocketdesk at the cast's size, and on the clipboard. (where: by hand)
- [ ] Record for about 30 s, then Stop: an MKV in Videos\Pocketdesk; `ffprobe` gives h264 at the cast's size and a duration within 10 % of the wall clock; it plays. (where: by hand)
- [ ] Recording while the phone turns: the file so far kept, the bar says why it stopped. (where: by hand)
- [ ] The cast ends during a recording: the bar names the file and stops counting. (where: by hand)

## 161. Commands to the phone's automation apps (L6)

Built 2026-10-21; tested through units, nothing on a device yet.
- [ ] A throwaway receiver in Tasker's package logs `dev.pocketdesk.android.event.COMMAND` with NAME, TEXT and ADDRESS after `pocketdesk phone-command`.
- [ ] With "Only to" set to another installed app: nothing reaches the receiver. (where: by hand) (a second automation app.)
- [ ] With no automation app installed: nothing is sent, and Settings says so. (where: by hand)
- [ ] Tasker itself.
- [x] 2026-10-21, on the OPPO with no automation app installed: `pocketdesk phone-command` reached the phone, which sent it nowhere ("no automation app").

## 162. The phone's state on the computer (L9)

Built 2026-10-21; tested through units, nothing on a device yet.
- [ ] The Overview's line matches the phone's Settings: free and total storage, Wi-Fi with its bars. (where: by hand)
- [x] The ringer turned on the phone shows within a tick in the background: vibrate reached the computer's `phone_status.json` in 2 to 7 s, four times (the OPPO, a throwaway server, no session, 2026-10-25; the runner's `ap1-ringer`, the ringer turned through the fixture app and put back).
- [x] The same within 15 s in a session: silent reached the computer in 2.1 to 5.1 s, four times (the OPPO, a throwaway server, the phone's input off, 2026-10-25, with AP3). Found there first: silent took 13.6 to 16.7 s over four runs, as the session's reporter read the phone's state on a 15 s tick and sent after it; AP3 checks at once when the ringer changes.
- [ ] Behind Tailscale on the phone: Wi-Fi with its bars, not nothing. (where: away)
- [ ] Mobile data with its bars, on a phone with a SIM (two SIMs: the data SIM's).
- [x] "Share the phone's state" off: nothing sent for 330 s, past the 5-minute resend; on again, the state arrived 8 s after the app started (the OPPO, a throwaway server, no session, 2026-10-24).
- [ ] The phone away: the line is gone.
- [x] 2026-10-21, on the OPPO (CPH2359) against a throwaway server: the computer kept 90.3 GB free of 128 GB, Wi-Fi with 4 bars and the ringer on silent, as the phone's own settings said.
- Found there: in the background the battery broadcast stayed stale on ColorOS (72 % kept while the phone was at 79 %); the level is now read from BatteryManager (`BatteryRule.fresh`), and the background report then followed the phone's real level.

## 163. Features on both sides (L7)

Built 2026-10-21; tested through units and the desktop harness, nothing on a device yet.
- [x] A switch turned off in the phone's Settings ("Links from the computer") reached the computer's `phone_switches.json` in 1 s, and on again in 24 s, with no session (the OPPO, a throwaway server, no session, 2026-10-24).
- [ ] The same within 15 s in a session, and on the computer's Devices page. (where: by hand)
- [ ] Notification access revoked on the phone: the features the listener runs say "Android allows: no", with the advice.
- [ ] Settings "Phone notifications" off on the computer: the notifications rows name it. (where: by hand)
- [x] A phone unpaired: its report gone from `phone_switches.json` within seconds. `pocketdesk unpair` took the OPPO out of `phone_switches.json` and `phone_status.json` in 2 to 5 s, four times (a throwaway server, its trust store put back after, 2026-10-25; the runner's `ap2-unpaired-reports`).
- [x] 2026-10-21, on the OPPO (CPH2359) against a throwaway server: the switches arrived with the battery's tick, matching the phone (control on with its Accessibility service off, so "Android allows: no"; photos without photo access). Found there first: the bridge let only listed packets out, and the report was not on the list (fixed).

## 164. Notifications to every computer of mine (L8)

Built 2026-10-21; tested through units, nothing on a device yet.
- [ ] Two throwaway servers, both paired, the second picked: a notification reaches both, and its cancel closes it on both. (where: second computer)
- [ ] The own server stopped: the second still gets each notification at once. (where: second computer)
- [ ] The second server stopped: the own one still gets every notification on time. (where: second computer)
- [ ] The second server shows no answer buttons, and no texts.
- [ ] The same chat posting, being cleared and posting again: the second is cleared on both. (where: second computer)
- [ ] The computer in use swapped (a session with the second): the picks still mean the same computers. (where: second computer)
- [x] 2026-10-21, on the OPPO with two throwaway servers (the second on other ports, reached as `localhost`): a notification reached both, the copy a second after the own computer's; with the own server stopped, the second still got the next at once; the copied cancel reached the second server. Found there first: copied cancels went as a plain packet, which the bridge refuses for notifications (fixed).

## 165. The other computers answer and clear (L8 part 2)

Built 2026-10-21; tested through units, nothing on a device yet.
- [ ] Two throwaway servers, both paired, the second picked, answering on: the second's toast offers the buttons, and pressing one runs it on the phone. (where: second computer)
- [ ] A toast closed on the second computer is cleared on the phone.
- [ ] The second computer's Overview does not show the phone as linked by that link, and its Send file says it cannot reach the phone.
- [ ] One computer saved at two addresses and picked: no link to itself, no doubled toasts. (where: second computer)
- [ ] The second server stopped: its link is tried again a minute later, and copies fall back to one-offs. (where: second computer)

## 166. Linked with the screen off (M1)

Built 2026-10-21. Checked on the OPPO by adb: the service started from the background, Pocketdesk unfrozen for 5 minutes with the screen off, and frozen with the switch off, the pause logged once the screen woke.
- [ ] The Health card on screen: "Turn on" with the switch off, and this phone's own setting after a pause with it on; Not now.
- [ ] After a reboot, the "Keeps the link" notification is back without opening the app.
- [ ] On battery without USB, on Wi-Fi, for an hour (N7's script): a ring from a throwaway server arriving with the screen off, CPU use and battery drain. (where: by hand)
- [ ] A Samsung, a Xiaomi and a Pixel: the service keeps the app linked with the screen off. (where: other hardware)
- [ ] Renaming the computer changes the notification's words. (where: by hand)

## 167. Both sides' switches, part 2 (M2)

Built 2026-10-21; tested through units and the desktop harness, nothing on a device yet.
- [x] The OPPO paired to a throwaway server, with Links off for this phone in its trust store and "Clipboard to phones" off in its Settings: "What this computer allows" said "The computer: off for this phone" on the Links row and "off in its Settings" on the clipboard's (2026-10-24).
- [ ] The same beside the desktop app's Devices page, by eye. (where: by hand)
- [ ] The sheet for a computer that is not the phone's own: every row "with its own computer only", the notifications too unless picked under "Other computers of mine". (where: second computer)
- [ ] A computer from before M2: the sheet says it cannot tell. (where: other hardware) (a computer on a release from before M2.)
- [ ] Paired as "Someone else's": a file sent from the phone lands; the remote browser and Download this folder are refused; a session waits for Allow; a clipboard push is refused. (where: by hand) (the Pair page.)
- [ ] Paired from the phone's "Show PIN on desktop" while the Pair page says "Someone else's": the same narrow permissions. (where: by hand) (the Pair page.)
- [ ] A phone paired as mine, paired again as someone else's: what was off stays off.
- [ ] `pocketdesk pair --someone-else` and `--grant 15m`: the same narrow permissions.

## 168. Image copies without a session (M3)

Built 2026-10-21; tested through units, nothing on a device yet.
- [x] A PNG in `POCKETDESK_CLIPBOARD_IMAGE_FILE` on a throwaway server, its "Images to phones' clipboards" on; the OPPO with "Computer's clipboard in the background" and "Computer's copied images in the background" on, on Wi-Fi: the image reaches the phone's clipboard, read back by its SHA-256 through a throwaway app. (2026-10-25, the runner's `ad1-image-copy`, by the internet road: told and fetched, three runs. By adb reverse the server rightly refused to dial the phone back at 127.0.0.1.)
- [ ] Either switch off, or the phone on mobile data: nothing fetched; back on Wi-Fi, it is fetched then. (2026-10-25, the images switch off: the image told to the OPPO and not fetched. Mobile data needs a SIM.)
- [ ] A screenshot copied on Windows (Win+Shift+S) with a phone linked: it goes once, even while the Snipping Tool holds the clipboard. (where: owner) (keys pressed on this computer.)
- [ ] A copy marked private on Windows (a password manager): neither pushed nor answered to a session's poll. (where: owner)
- [ ] The tile with an image copied on the phone: the seam file holds it; away from home with no link, it opens one and sends.
- [ ] GNOME (no `wl-paste --watch`): an image on the clipboard is read at most every 30 s.

## 169. Photos keep where they were taken (M5)

Built 2026-10-21; tested through units, nothing on a device yet.
- [ ] Photo access revoked first, then the switch turned on: Android asks for photo access, and the switch stays on. (where: by hand)
- [ ] A JPEG with GPS tags pushed to `DCIM/Camera` by adb, shared from the gallery to a throwaway server with the switch on: the tags arrive (an EXIF reader); with it off, they are gone.
- [ ] The same photo fetched through recent photos (C7), and in a folder send: the tags kept. (where: by hand) (photos with GPS tags.)
- [ ] Android 14's "selected photos only": the row says access is needed, and photos go as before. (where: by hand)
- [ ] A share from the Files app (`com.android.providers.media.documents`), and one from Google Photos. (where: by hand)

## 170. No toast while the phone is in hand (M6)

Built 2026-10-21; tested through units, nothing on a device yet.
- [x] The throwaway chat app's notification with the OPPO unlocked and its screen on, the switch on: no toast on the computer; the Activity page says "not shown: the phone was in use". 2026-10-21, on the OPPO against a throwaway server, with a shell notification standing in for the chat app: the computer's event carried `phone_in_use`, so no toast. The Activity page's words were not looked at.
- [x] The same with the screen off, the phone locked, or the switch off: a toast. 2026-10-21: with the phone asleep and locked, the next one came without `phone_in_use` (shown), 12 s after it was posted, the screen off.
- [ ] With Pocketdesk's Remote open on the phone: a toast. (where: owner) (a remote needs the Input permission.)
- [x] A notification that comes as the screen is off and wakes it: a toast. (seen 2026-10-24 on the OPPO against a throwaway server (S5): posted with the screen off, its toast in Windows' notification store.)

## 171. Type on the phone from the computer's keyboard (M4)

Built 2026-10-21; tested through units, nothing on a device yet.
- [x] The OPPO with "Typing from the computer's keyboard" and the Accessibility service on, a scratch field in a throwaway app focused: text, a Backspace right after typing, Enter, and Tamil arrive, read back by `uiautomator dump`; the notification shows with Stop. 2026-10-21, against a throwaway server, a batch written to `typing_out/` as the window writes it, into the Google app's search field: `Héllo தமிழ்`, a Backspace (it took the Tamil vowel sign) and `!` read back as `Héllo தமிழ!`; the typing notice showed while typing. Enter was not sent.
- [ ] Back, Home and Recents from the window, in turn with text typed before them. (where: by hand) (Pocketdesk control's Accessibility service, turned on by hand.)
- [ ] A password field: nothing typed, and the notification says so. (where: by hand) (Pocketdesk control's Accessibility service.)
- [x] The phone locked: nothing typed. 2026-10-21: a batch sent while the OPPO slept locked typed nothing; the field was unchanged after unlocking.
- [ ] Stop: nothing typed for 10 minutes. (where: by hand) (Pocketdesk control's Accessibility service.)
- [ ] The switch off, or the app's notifications off: nothing typed. (where: by hand) (Pocketdesk control's Accessibility service.)
- [ ] Japanese through Windows' input method: the committed text arrives. (where: owner)
- [ ] The phone unlinked while the window is open: the window says why and sends nothing. (where: by hand)

## 172. The phone's sound without its screen (M7)

Built 2026-10-21; tested through units and a loopback session, nothing on a device yet.
- [ ] The OPPO with the throwaway tone app against a throwaway server with the WAV seam (`POCKETDESK_PHONE_AUDIO_FILE`): the tone's frequency and level; no cast window (the relay's log says nothing); the Sessions page says "its sound here". (where: by hand) (a tone app on the phone.)
- [ ] "Play its sound here" off for the phone: refused, the phone saying why; turned off mid-session: it ends. (where: by hand) (the casting consent.)
- [ ] Stop from the phone's notification, and End on the Sessions page. (where: by hand)
- [ ] The microphone not granted on Android 13+: its request shows.
- [ ] Started while the screen is shown: refused before Android's consent, saying why. (where: by hand)
- [ ] An hour with the screen off: the battery. (where: by hand)
- [ ] By ear, and a real music app (the owner).

## 173. tel: links and Call back (M8)

Built 2026-10-21; tested through units, the harness and the registry read back here, nothing on a device yet.
- [x] `pocketdesk tel tel:+1-555-0100` to the OPPO through a throwaway server: the dialer opens with the number (`dumpsys activity activities`); the desktop notification says so. 2026-10-21, against a throwaway server with two paired phones, only the OPPO linked: it was chosen ("Calling +15550100: Press Call on OPPO Reno8 5G.") and the dial reached it, which declined with its "Calls from the computer" off (L4's rule). The dialer opening is owed with that switch on. (seen 2026-10-24 on the OPPO against a throwaway server (S5): the dialer in front with the number, nothing placed.)
- [ ] Two phones that may dial, neither linked: the Overview asks; a choice dials; Cancel, and two minutes, drop it.
- [ ] Settings' switch on Windows: Default apps offers Pocketdesk for TEL; a link clicked in a browser opens the dialer with no console window. (where: owner) (it registers a handler on this computer.)
- [ ] Off again: the keys are gone and Windows asks for another app. (where: owner)
- [ ] Linux: the desktop file and `xdg-open tel:5550100`; removed, `mimeapps.list` no longer names it.
- [ ] Call back from a ringing call's note, and from one whose call ended (a SIM).

## 174. Home Assistant buttons for phone commands (M9)

Built 2026-10-21; tested through units, nothing against a broker or a device yet.
- [ ] A local broker as in 1e: the buttons announced under the phone's device; a press published with `mosquitto_pub` reaches the OPPO's throwaway receiver in logcat. (where: by hand) (an MQTT broker.)
- [ ] A retained or empty message on the command topic: nothing sent. (where: by hand) (an MQTT broker.)
- [ ] The phone's "Send commands to its automation apps" off: refused, in the server's log. (where: by hand) (an MQTT broker.)
- [ ] A command removed in Settings: its button goes from the broker; another command keeps its id. (where: by hand) (an MQTT broker.)
- [ ] Home Assistant itself, and Tasker (the owner).

## 175. Copies sent by themselves (M10 part 2)

Built 2026-10-21; checked on the OPPO by adb: a copy noticed, the reading window started from the background, the clipboard read, two copies 6 s apart both read. Owed:
- [x] The copy reaching the computer's clipboard on a throwaway server with the clipboard seam (`POCKETDESK_CLIPBOARD_FILE`). 2026-10-21, on the OPPO by adb: `endtoend42` copied by keys in the Google app's search field reached the throwaway's clipboard seam by itself ("A copy sent by itself").
- [x] A password manager's copy (marked sensitive): read, not sent. 2026-10-21, on the OPPO by `scripts/device_checks.py` (`m10-sensitive`): the fixture's copy marked `IS_SENSITIVE` did not reach the throwaway's clipboard seam, and a plain copy right after it did. A real password manager is still owed.
- [ ] The switch off: nothing watched, nothing read (`dumpsys accessibility` shows no event types). (where: by hand) (Pocketdesk control's Accessibility service.)
- [ ] The keyboard and selection in the app copied from, after the read (focus moves away and back). (where: by hand)
- [ ] An hour with the switch on: the battery. (where: by hand)
- [ ] Another maker's Android 13+, and Android 12 (the row hidden).

## 176. The phone's music without a session, with its cover (N2)

Built 2026-10-21; checked on the OPPO against a throwaway server, with the fixture app's silent player and no session (also as `scripts/device_checks.py`'s `n2-media`): the report with `background: true` and a 256 px cover kept; Pause from the computer pausing the player; a player paused more than five minutes not sent after the app was reinstalled; A, then B, then A again with A's cover back; the server restarted while it played, the phone dialing again and the cover resent; the switch off clearing it on the computer. Owed:
- [ ] A real player (Spotify, YouTube Music) with the screen off, an hour: its cover, presses, and the battery by N7's script.
- [ ] Windows' media controls with the tray program running: the phone's song and cover in the volume flyout, the media keys (with the owner).
- [ ] The Overview's music line and buttons, and the tray's menu, by hand.
- [ ] Next and Previous from the computer. (where: by hand) (a real player on the phone.)
- [ ] Media turned off and on for the phone on the Devices page mid-track: the cover comes back at the next track. (where: by hand)
- [ ] Two sessions in one app (a browser's two tabs): the cover of the one reported. (where: by hand)
- [ ] Another maker's Android, and Android 10.

## 177. Open on the phone (N3)

Built 2026-10-21; checked on the OPPO against a throwaway server, by `scripts/device_checks.py` (`n3-open`, `n3-locked`): a fixture notification opened from the computer showing its field on the phone's screen, with Pocketdesk control on and the app in the background; refused while locked ("the phone is locked"). Owed:
- [ ] Without Pocketdesk control: "Tap to open" on the phone, its tap opening the app, gone after ten minutes or when the notification goes. (where: by hand)
- [ ] The button by hand on the Notifications page, in a Windows toast, and as a Linux D-Bus action.
- [ ] The switch off on the phone: "opening is off on the phone". (where: by hand)
- [ ] A real chat or mail app's notification, and one whose app blocks a background start (another maker's limits). (where: by hand)
- [ ] Android 15 and 16 (`ALLOW_ALWAYS`). (where: other hardware) (Android 15 or 16.)

## 178. The phone's ongoing updates on the computer (N4)

Built 2026-10-21; checked on the OPPO against a throwaway server, by `scripts/device_checks.py` (`n4-live`): the fixture's progress notification at 10, 50 and 52 % reaching the computer's card at 52 % (held back by the gap, then sent), then 100 %, and its end clearing the card; one event for the card. Checked here by an ignored live test: the toast shown at 10 % and updated in place with new words. Owed:
- [ ] A real download or upload (Chrome, Drive), a timer (Clock) and a delivery or ride app, by eye on the toast: no pop-up after the first, the words changing.
- [ ] Android 16's Live Updates (`ProgressStyle`) on a phone that has them. (where: other hardware) (Android 16.)
- [ ] Closing the toast mid-download: it stays closed; the card stays on the page. (where: by hand)
- [ ] The phone in hand at the first update: the toast comes once it is put down. (where: by hand)
- [ ] A Linux desktop (the `value` hint, the bar, a close).
- [ ] The battery with a long download. (where: by hand)

## 179. The computer's music in the phone's media controls (N5)

Built 2026-10-21; checked on the OPPO against a throwaway server whose music comes from a file (`POCKETDESK_MEDIA_FILE`), by `scripts/device_checks.py` (`n5-media`): the song in Pocketdesk's media session, not sent back as the phone's music with N2 on, a media key's Next reaching the file's presses, the session gone when nothing plays; `n2-media` passing beside it. Owed:
- [ ] The lock screen and the shade's media controls by eye: title, artist, cover, and Previous, Play/Pause, Next as the computer's player offers them.
- [ ] A real player on this computer (Spotify, a browser), with the owner: a press from the phone plays, pauses and skips it; never Pocketdesk's own entry (the phone's music) sent back.
- [ ] A headset's button and a watch's controls. (where: other hardware)
- [ ] The link dropped: the session gone within 150 s; back by itself when the link returns. (where: by hand)
- [ ] Audio turned off for the phone on the Devices page: nothing more comes, and the session ends. (where: by hand)
- [ ] A Linux computer (`playerctl`, `mpris:artUrl`).

## 180. Files into the phone's folders from the computer's drive (N6)

Built 2026-10-22; checked on the OPPO against a throwaway server with `phone_drive = true` (2026-10-22): two folders shared through Settings by adb, one marked (the picker opened at it, after a fix; another folder picked left it unmarked); `scripts/device_checks.py`'s `n6-put` PASS (a folder made through the drive and in the phone's next listing, a PUT into the unmarked folder 403); a PUT's file reached the phone as a fetch, which the computer refused to dial back over `adb reverse` (loopback, as for F3). Owed:
- [ ] A file's write into the marked folder on the LAN road (or the internet road), its SHA-256 read back through the drive. (where: owner) (the LAN road needs the throwaway's port open.)
- [ ] Unmarking: the write grant given back. (where: by hand)
- [ ] File Explorer with the drive mapped: copy a file and a folder in (Windows' empty file first, then its content), a large file past 45 s (answered 202, then on the phone), a name taken (the next free one). (where: by hand)
- [ ] A file larger than the phone's free space: refused, said in Explorer. (where: other hardware) (a full phone.)
- [ ] A Linux file manager (Nautilus, Dolphin) over `dav://`.
- [ ] Desktop Settings "The phone's folders take files" off: the drive read-only again. (where: by hand)
- [ ] Explorer's rename, delete and "New folder" (MOVE and DELETE are refused): what Explorer says. (where: by hand)
- [ ] A folder that is also where files from computers go: unmarking it leaves downloads there working. (where: by hand)
- [ ] A marked folder on an SD card with too little room: the phone checks only its own storage, so the copy fails part way and says so.

## 181. Toasts only on the computer in use (N7)

Built 2026-10-22. Owed:
- [ ] Two throwaway servers, both picked on the OPPO (L8), the switch on: a notification while one is in use pops up there only, and the other's Notifications page and Activity have it ("not shown: another computer was in use").
- [ ] Neither in use: both pop up. One's server stopped: the other pops up within 30 s of its last answer. (where: second computer)
- [ ] "Tell phones when this computer is in use" off on one: it always pops up. (where: second computer)
- [ ] The battery with the links kept, by N7's background-cost script. (where: by hand)
- [ ] A Linux computer: it does not say, and always pops up.

## 182. Leftovers: typing from the tray and a key, several links, the phone nearly full (N9)

Built 2026-10-22. Owed:
- [ ] The tray's "Type on the phone" for a linked phone: the typing window opens (both trays).
- [ ] "Type on the phone with a key": registered, a press opening the window; another app holding it said in Settings. (where: owner) (a key pressed on this computer.)
- [ ] A share with three links sent to "Open on computer": three open on the computer, and the toast says three. (where: owner) (it opens links in this computer's browser.)
- [ ] A phone with under a gigabyte free: one notification, none again until it has had room. (where: other hardware) (a full phone.)

## 183. Linux catch-up: the phone's music in MPRIS, the laser dot, the typing and cast windows (N10)

Built 2026-10-22. Run on a throwaway Azure VM (Ubuntu 24.04, headless Sway with waybar as the tray host, the release workflow's Linux build) with the OPPO (CPH2359) over the internet:
- [x] The phone's song in the desktop's media controls: `playerctl -l` lists `pocketdesk_phone`, with its title, artist, app and cover (`file://` in the state folder).
- [x] Pause and Play from `playerctl` reach the phone's player within 2 to 3 s, Paused shown at once and kept.
- [x] The player leaves the bus within 3 s of the phone's player stopping.
- [x] The typing window opens from the tray's request (`app_open` page `type`), and `wtype` into it types `Linux typing é` into the fixture's field.
- [x] "Show screen" opens the phone's window on the VM, live, with its Screenshot, Record and control bars.
- [x] Remote offers "Laser pointer" there; held, a red dot is drawn at the screen's middle through the layer shell, and gone once let go.
- [x] After the fixes, with no tray host: the tray program runs without an icon and still lists the phone's song in MPRIS; the window starts with no panic.
Owed:
- [ ] The laser dot moved by turning the phone (the phone lay still), and across two outputs.
- [ ] GNOME's and KDE's media controls by eye, and the media keys.
- [ ] A press on the cast window's picture (the VM has no pointer). (where: linux)
- [ ] Orca with the window (AccessKit's Linux backend, fixed here: it had panicked at start).

## 184. Automation apps start the computer's tasks and press its deck (O1)

Built 2026-10-22. Checked on the OPPO (CPH2359) against a throwaway server through `adb reverse`, by `adb shell am broadcast`:
- [x] From a cold app, `RUN_TASK` ran the task within a second; `TASK_FINISHED` told done, code 0.
- [x] `PRESS_DECK` ran a task-kind button; `DECK_PRESSED` told done.
- [x] Refusals told: no such task or button (`unknown`), an unsaved address (`not_saved`), an empty name (`bad_name`), the 11th in a minute (`too_many`), another computer than the phone's own with no link (`not_linked`).
- [x] With the switch off, nothing ran and nothing was told.
- [x] 2026-10-23, by `scripts/device_checks.py` (R1) on the OPPO, `o1-task` and `o1-deck`: RUN_TASK ran an `echo` task on the computer (code 0) and an unknown name started nothing; PRESS_DECK pressed a task button, its task ran.
Owed:
- [ ] Tasker itself: a Send Intent to the receiver, and Intent Received for the events.
- [ ] A task that asks first, allowed at the desk. (where: by hand)
- [ ] Over the internet road, cold. (where: by hand) (Tasker.)

## 185. A screenshot of the phone and its screen locked from the computer (O2)

Built 2026-10-22. Checked on the OPPO (CPH2359) against a throwaway server through `adb reverse`, the requests written as the desktop app writes them:
- [x] A screenshot asked: taken (ColorOS saved it) and picked up by "Send new screenshots".
- [x] A lock asked: the keyguard shown, the phone asleep.
- [x] Not done: a screenshot on the lock screen, a screenshot with the screen off, a lock with the computer's permission off.
Owed:
- [ ] The screenshot's arrival on the LAN road (over `adb reverse` the computer will not dial back to 127.0.0.1). (where: owner) (the LAN road needs the throwaway's port open.)
- [ ] The Overview's and the tray's buttons by hand, and their words when the phone is away or the permission off.
- [ ] Android 9 to 12, and a phone with a PIN.

## 186. A paired computer found again at a new address (O3)

Built 2026-10-22. Owed (it moves this computer to a second address, so with the owner):
- [ ] The computer on the phone's hotspot (or a new DHCP lease): its card moves to the new address with a snackbar, connects, and keeps its name, wake settings and pairing.
- [ ] Another computer announcing the first one's id at a new address (its own certificate): no move.
- [ ] A card saved by name (`mypc.local`) is left alone. (where: by hand)

## 187. Notifications shared by their kind as well as their app (O5)

Built 2026-10-22. Checked on the OPPO (CPH2359) with the fixture app's two channels against a throwaway server:
- [x] Both kinds listed under the app by their titles ("Message A", "Updating messages"), and both sent while on.
- [x] "Updating messages" switched off in Settings: the next one in that kind stayed on the phone, the next message went.
- [x] 2026-10-23, by `scripts/device_checks.py` (R1) on the OPPO, `o5-kinds`: a fixture kind kept on the phone stayed there (posted, never on the computer), another kind arrived.
Owed:
- [ ] A real chat app's kinds listed under it, by their titles, and one kept on the phone while its messages go.
- [ ] TalkBack reads each kind's switch with its kind.

## 188. Copies kept on the computer by program or by look (O4)

Built 2026-10-22. Checked on a throwaway server with the clipboard stand-in and the OPPO linked:
- [x] A copy matching a "Never send copies that look like" pattern held, said once in the log; an ordinary copy not.
Owed:
- [ ] A copy made in KeePassXC (or another listed program) on Windows: not on the phone, not in the history, not sent by the tray.
- [x] A phone asking in a session for a copy held by its look gets nothing: with `clipboard_never_like = ['^s5-secret-']` a matching copy stayed on the computer and a plain one crossed. (Seen 2026-10-24 on the OPPO against a throwaway server (S5).)
- [ ] A copy, and an image, from a listed program held the same. (where: by hand)
- [ ] The two Settings rows by hand, "Add the usual password managers" and "Add keys and tokens". (where: by hand)

## 189. Find a setting on the phone (O8)

Built 2026-10-22. Checked on the OPPO:
- [x] "ring" showed the Ring and ringer rows under their groups' headings, and no other heading (it had found "during" before the review).
- [x] "wake" showed the Wake group; "zzz" said no setting matches, with no choice rows left over.
- [x] Back with the box full emptied it, the whole list back.

Owed:
- [x] Clear by tap; Back again leaves Settings. (seen 2026-10-24 on the OPPO against a throwaway server (S5): "ring" found its rows, "Clear the search" emptied the box, one Back left the search and the next left Settings.)
- [ ] TalkBack reads the box and its Clear; Spanish and Tamil search their own words.

## 190. Received images onto the clipboard (O7)

Built 2026-10-22. Owed:
- [ ] A photo from the phone, with "Images sent here onto the clipboard" on: pasted into Paint upright (a portrait photo turned by its EXIF), and the file still in Downloads. (where: by hand)
- [ ] A PNG from the computer, with "Images from the computer onto my clipboard" on: pasted into a chat app, and not sent back by the tile or a session's clipboard sync. (where: by hand)
- [ ] Either switch off, or a folder of images, or a GIF: the clipboard unchanged. (where: by hand)
- [ ] Linux: a JPEG onto the Wayland clipboard (`wl-paste --list-types` says `image/png`). (where: linux)

## 191. Choose the computer's player (O6)

Built 2026-10-22. Checked on the OPPO against a throwaway server with the media seam holding two players:
- [x] The Sound sheet's card offered "Player" with the computer's choice, Chrome and "Spotify (playing)"; choosing Spotify showed its song, and Next reached it (`next Spotify.exe`).
- [x] Back to the computer's choice: the card showed Chrome's tab again, and Previous went unnamed.

Owed:
- [ ] Two real players on Windows (a browser tab and Spotify): the one chosen takes Play, Next and Previous from the Sound sheet, the widget and the lock screen. (where: by hand)
- [ ] The chosen player closed: the card reads "The computer's choice", and the choice comes back when the player opens again. (where: by hand)
- [ ] Linux: two MPRIS players under `playerctl`. (where: linux)

## 192. Diagnostics' firewall row on Linux (O9)

Built 2026-10-22. Owed (where: linux):
- [ ] No ufw: "no firewall found". (where: linux)
- [ ] ufw on with the default deny: the row is red, naming 1716/tcp and 4716/udp with `sudo ufw allow …`; after allowing them, green. (where: linux)
- [ ] The app run without root: ufw's rules "need root to read", with `sudo ufw status`. (where: linux)
- [ ] firewalld with KDE Connect's service: 1716/tcp in, 4716/udp named. (where: linux)

## 193. The computer's announcement follows its network (P1)

Built 2026-10-22. Checked on the OPPO against a throwaway server (named apart from this computer's own, so its goodbye touched no other record), its addresses moved by the seam:
- [x] Announced at this computer's address: the saved card read "On this network".
- [x] Moved to a documentation address: "announced again" logged after two readings, and the card lost "On this network" (without the goodbye it had kept it: Android holds an old resolution).
- [x] Moved back: announced again, and the label returned.

Owed:
- [ ] A real move: the computer onto another Wi-Fi, or a VPN brought up, found at the new address within a minute. (where: away)
- [ ] A wake from sleep with a new lease. (where: owner)
- [ ] Linux. (where: linux)

## 194. Open an app on the phone from the computer (P2)

Built 2026-10-22. Checked on the OPPO (CPH2359) against a throwaway server through `adb reverse`, the requests written as the desktop app writes them:
- [x] The apps asked for: 77 listed on the computer (Settings among them, Pocketdesk left out).
- [x] An open with the control service off: "Tap to open: Settings" offered, and its tap opening Settings.
- [x] An open with "Let the computer open my apps" off: refused (`OFF`).
- [x] 2026-10-23, by `scripts/device_checks.py` (Q1) on the OPPO, `p2-apps`: 77 apps listed, Settings offered with Tap to open.
Owed:
- [ ] An open with the control service on: the app in front at once, also with the screen locked (refused) and from the background. (where: by hand) (Pocketdesk control's Accessibility service.)
- [ ] The Overview's "Open an app on the phone…" by hand: the search, "No answer from the phone" with the phone locked, Open greyed on an old list. (where: by hand)
- [ ] Android 13 and older, and a work profile's apps.

## 195. Quiet hours, and quiet while presenting (P3)

Built 2026-10-22. Checked on the OPPO (CPH2359) against a throwaway server through `adb reverse`, the fixture app's notifications, the config changed while it ran:
- [x] "While presenting or in full screen" on with the seam's file (`POCKETDESK_PRESENTING_FILE`): logged with `presenting`; without the file, not.
- [x] Quiet hours from two minutes ago to ten minutes on: logged with `quiet_hours`; with a span later today, not.
- [x] 2026-10-23, by `scripts/device_checks.py` (Q1) on the OPPO, `p3-presenting`, `p3-hours`: held with `presenting` and with `quiet_hours`, each with a control shown.
Owed:
- [ ] Windows' own state: a PowerPoint slideshow, a game in full screen, and a video in a browser's full screen. (where: owner)
- [ ] A notification under quiet hours with the phone locked: no pop-up, on Activity and, when answerable, on the Notifications page. (where: by hand)
- [ ] The Settings row by hand (the days, a bad time's warning) and the Overview's line, also on Linux.

## 196. Notifications held by their words (P4)

Built 2026-10-22. Checked on the OPPO (CPH2359) against a throwaway server through `adb reverse`, the fixture app's notifications, the config changed while it ran:
- [x] `big sale` held "Today only: BIG SALE" (logged with `held_by_words`); `Pocketdesk fixture: standup` held "Standup at 10"; "Lunch?" was not held.
- [x] 2026-10-23, by `scripts/device_checks.py` (Q1) on the OPPO, `p4-words`: held by its words, case aside, with a control shown.
Owed:
- [ ] The Settings box by hand: its warning for a line ending with a colon, and Activity naming the line with "Details in events" on. (where: by hand)
- [ ] An update held by its words closing the toast its first showed. (where: by hand)
- [ ] A call's note with words in it (a SIM): still shown.

## 197. A phone's ringing alarm on the computer (P5)

Built 2026-10-23. Checked on the OPPO (CPH2359) against a throwaway server through `adb reverse`, paused so nothing rang here, the alarm set with `SET_ALARM`:
- [x] With "Alarms on the computer" on and Clock not shared, the ringing alarm (ColorOS: no category, no-clear, a foreground service) reached the computer logged with `alarm`.
- [x] Seen first: with the switch off it never left the phone (dropped as ongoing).
- [x] 2026-10-23, by `scripts/device_checks.py` (Q1) on the OPPO, `p5-alarm`: the fixture's `CATEGORY_ALARM` notification reached the computer marked `alarm`, the server paused.
Owed:
- [ ] The Windows alarm toast by ear and eye: its sound, Snooze and Stop pressed from here, Dismiss, its close when the phone stops it, and its end after ten minutes.
- [ ] Linux's urgent notification.
- [ ] A finished timer, and a Pixel's clock (`CATEGORY_ALARM`, local only). (where: other hardware) (a Pixel.)

## 198. Files to several phones or computers at once (P6)

Built 2026-10-23. Checked on the OPPO (CPH2359) against a throwaway server through `adb reverse`, by the CLI:
- [x] `pocketdesk send <phone>,nosuchphone,<the phone's id> file --json`: the OPPO's send queued once (the repeat skipped), the unknown name refused, `"error": "1 of 2 did not go"`, exit 2.
- [x] 2026-10-23, by `scripts/device_checks.py` (Q1) on the OPPO, `p6-several`: the phone once, the repeat skipped, the unknown refused, exit 2.
Owed:
- [ ] Two real targets (the OPPO and a second computer, or two phones): both offered, each its own row in the sent list.
- [ ] The Files page by hand: "Also to…", "Every one", "And 2 more…", a drop's "All of them", the notice when one has left. (where: by hand)

## 199. Files copied in Explorer sent with the clipboard key (P7)

Built 2026-10-23. Checked on the OPPO (CPH2359) against a throwaway server through `adb reverse`, both clipboard seams set (`POCKETDESK_CLIPBOARD_FILE`, `POCKETDESK_CLIPBOARD_FILES_FILE`), the request written as the tray writes it:
- [x] Two copied files with the phone linked over `adb reverse` (which files cannot use): refused at once, "phone not connected", with the notification, nothing queued.
- [x] 2026-10-23, by `scripts/device_checks.py` (Q1) on the OPPO, `p7-files`: two copied files read through the seam and queued.
Owed:
- [ ] On the LAN: two files copied in Explorer, the tray's Send the clipboard and the key each sending them, rows in the sent list.
- [ ] Files off and text copied too: the text sent instead. (where: owner) (the clipboard key on this computer.)
- [ ] The Files page's "Paste copied files", one target and several; more than 100 copied. (where: by hand)
- [ ] Linux: files copied in Nautilus or Dolphin (`text/uri-list`).

## 200. Links by site (P8)

Built 2026-10-23. Unit-tested: the rules' reading and matching (the longest site, a name under it, an address whole, a backslash), and the links plugin with a stand-in opener and runner (a site shown, one handed to a program, one opened, a program that fails said).
Owed:
- [ ] A phone's link through each rule on a real server: `show`, `browser`, a program (Windows and Linux), a program that is not there.
- [ ] The Settings box by hand: its warning for a bad line, a .bat refused. (where: by hand)

## 201. Permission profiles (P9)

Built 2026-10-23. Unit-tested: the store (saved, replaced, removed, a broken file kept), the PIN file's `profile` line (with its space, CRLF, a bad name still someone else's), the pairing preset (a profile's permissions for a new phone, never widening one paired, none for a computer).
Owed:
- [ ] Devices page by hand: Save as profile, Replaced, Apply a profile (a narrowed one ending the phone's session once). (where: by hand)
- [ ] Pair page: a profile chosen, its words, a new code; a phone paired with it gets those permissions; the profile removed before the scan pairs as someone else's.

## 202. A ring that vibrates and flashes (P11)

Built 2026-10-23. JVM-tested: the torch's blink and the vibration's pattern. Checked on the OPPO (CPH2359) with `pocketdesk ring` from a throwaway server through `adb reverse`:
- [x] The ring vibrates (an alarm vibration of 30,009 ms) and blinks the torch every 0.4 s, both ending with the ring's 30 seconds.
- [x] 2026-10-23, by `scripts/device_checks.py` (Q1) on the OPPO, `p11-ring`: a 30 s alarm vibration and 38 torch blinks, off at the end.
Owed:
- [ ] Stop and a dismiss ending both at once. (where: by hand)
- [ ] Each switch off: no shaking, no torch; the camera open in an app: the torch left alone, the ring going on.

## 203. The phone's contacts on the computer (P10)

Built 2026-10-23. Unit-tested: the list's reading (names cleaned, numbers a call takes, each once, five a contact), the store, the phone's rule (its switch, its own computer, Android's permission, the minute), the desktop's search. Checked on the OPPO (CPH2359) against a throwaway server through `adb reverse`, the ask written as the desktop app writes it:
- [x] With the switch on and contacts allowed, the phone's list reached the computer (counts only were looked at; the file was deleted after).
- [x] 2026-10-23, by `scripts/device_checks.py` (Q1) on the OPPO, `p10-contacts`: the list reached the computer (its count only), then deleted.
Owed:
- [ ] The Messages page by hand: Ask, the search, Text filling the number, Copy, Call (and with Dial off), "No answer" with the switch off. (where: by hand)
- [ ] Texts turned off on Devices and the phone unpaired: the list forgotten.

## 204. The desktop's O and P pages after their design pass (Q2)

Built 2026-10-23. Unit-tested: what a profile turns off, the phone's switch against a screenshot, unread lines holding Save, a count's words. A harness test applies a profile through its question.
Owed:
- [ ] Devices by hand: Apply a profile's question (what it turns off, the session ending), Remove a profile…, Save as profile without a name, a broken profiles file said; the Pair page's choice wrapping with a long name. (where: by hand)
- [ ] The Overview: the line under a phone's buttons with its "Let the computer take screenshots and lock this phone" off; quiet hours' group and Change quiet hours. (where: by hand)
- [ ] V6: with only "Pocketdesk control" off on the phone, the line under its buttons names Accessibility and not notification access. (where: by hand)
- [ ] Files: a link's notice with "Also to…" ticked, "And 2 more…"'s hover, Paste copied files while reading and with several phones to choose from. (where: by hand)
- [ ] Messages and the apps box: "Asking…", Copied., Ask again, the box coming into view; Narrator or Orca reading a row's buttons.
- [ ] Settings on Linux: the presenting row naming Do Not Disturb (Q4), no "Never send copies from these programs" row; a bad link rule, words line or quiet hours keeping Save off.

## 205. The phone's O and P rows after their design pass (Q3)

Built 2026-10-23. Unit-tested: the switches report's list on both sides and what Settings hold back (alarms by phone notifications or "Phones' alarms ring here"). Checked on the OPPO (CPH2359) against a throwaway server built from the branch:
- [x] All 24 features reported, alarms, contacts, images, tasks and apps among them with the phone's own values.
- [x] "Let the computer open my apps" on: its summary says it opens at once with Pocketdesk control on, else as "Tap to open", and no Accessibility link is offered.
- [x] "Vibrate while it rings" and "Flash the torch while it rings" live with their "in a session too" summaries.
Owed:
- [ ] Settings' search with "This phone's notifications" off: "alarms" and "timer" find Alarms on the computer greyed, and the camera's "Also onto its clipboard".
- [ ] "Let the computer take screenshots and lock this phone" warning while Send new screenshots carries nothing, and gone once it does; hidden on Android 8.
- [ ] My contacts on the computer: the permission refused (the line and Android's page), then granted there; without notification access, its warning and link. (where: by hand)
- [ ] The Devices page and the phone's "What this computer allows" naming the five new features. (where: by hand)
- [ ] A kind of notification's row, Spanish and Tamil on screen, font scale 2.0, TalkBack across the moved and changed rows.

## 206. Linux catch-up for the P cycle (Q4)

Built 2026-10-23. Unit-tested: GNOME's `show-banners` and KDE's `Inhibited` answers read. The Linux code type-checked for `x86_64-unknown-linux-gnu`; CI builds it.
Owed:
- [ ] GNOME: Do Not Disturb on, "While presenting or in full screen" on: a phone's notification logged with `presenting`, no pop-up; off again, shown (within 5 s).
- [ ] KDE Plasma: its Do Not Disturb, and while it presents, the same.
- [ ] A phone's alarm on KDE: the urgent notification with the alarm sound; on GNOME, silent but urgent.
- [ ] X11 (no `WAYLAND_DISPLAY`): files copied in Nautilus or Dolphin sent with the clipboard key or "Paste copied files", through `xclip`; with `xclip` missing, nothing read and nothing hung.

## 207. Start beside KDE Connect (T1)

Built 2026-10-24. Unit-tested: the ports tried, `listening.json` believed only for the running server, the bind moving past a held port, the Diagnostics finding, the phone's candidates for a card's new port. Checked on the OPPO against a throwaway server (control port 1726 held by another listener): the server took 1727, said why and recorded it, a second start for the same folder stopped at once, and the phone connected on 1727; with 1726 free again it took 1726.
Owed:
- [x] Linux with a listener holding 1716: the server started on 1717, said why in its log, wrote `listening.json` (1717 for 1716), and a second start for the same folder stopped at once (Azure VM, Ubuntu 24.04, 2026-10-24).
- [ ] Linux with KDE Connect (or GSConnect) itself running: the KDE bridge off, Diagnostics' "Control port moved", and the pairing code naming 1717. (where: linux)
- [ ] A phone with a card saved at the computer's LAN address finding it on the moved port by itself (the toast), and its tailnet card following. (where: by hand)
- [ ] Windows: `pocketdesk install` opening TCP 1716-1725, and a phone reaching a server moved to 1717 through the firewall. (where: by hand)
- [ ] The desktop app's Overview label and Diagnostics with the port moved. (where: by hand)

## 208. The clipboard on X11 (T2)

Built 2026-10-24. Unit-tested: Wayland or X11 from `WAYLAND_DISPLAY` and `DISPLAY` (neither: Wayland, as before), `xclip`'s command lines and the best text target. The X11 watcher and the reads were type-checked and linted for `x86_64-unknown-linux-gnu` here; CI builds the server.
Owed:
- [x] X11 under Xvfb, the phone by the internet road: text copied with `xclip` reached the phone in the background (with its "Computer's clipboard in the background" on), read once per copy (no `xclip` but the copy's own over 10 idle seconds: the XFixes watcher); a text the phone sent from its history landed in the X11 clipboard, offered as `UTF8_STRING` (Azure VM, Ubuntu 24.04, 2026-10-24).
- [ ] X11 on a real desktop (Mint, or Xfce): the same in a session, and an image both ways. (where: linux)
- [ ] X11 with KeePassXC: a password it copies stays on the computer. (where: linux)
- [ ] X11 without `xclip`: nothing read, nothing hung, the server says so once. (where: linux)
- [ ] Wayland as before: Sway and GNOME, the watcher still `wl-paste --watch`. (where: linux)

## 209. New camera photos to the computer (T3)

Built 2026-10-24. Unit-tested: the camera's folder, taken since the switch, an empty file waited for, Wi-Fi only. Checked on the OPPO against a throwaway server: a photo and a video pushed into `DCIM/Camera` handed to the transfer service and offered to the computer, a photo in `Pictures` not, one taken with Wi-Fi off waiting and going when Wi-Fi came back.
- [x] Seen in `DCIM/Camera` and handed over; another folder not; Wi-Fi only holding one back (OPPO, 2026-10-24).
Owed:
- [ ] On a LAN (not `adb reverse`): photos and a video landing in the computer's received files, matching their bytes. (where: by hand)
- [ ] The phone's own camera: a burst of 20, a 4K video, and photos taken while the computer was off going when it is back. (where: by hand)
- [ ] Photo access taken back: the switch off and Home's notice; video access taken back: "Videos too" off. (where: by hand)
- [ ] The computer's Devices page listing "Its new camera photos here"; Settings in Spanish and Tamil. (where: by hand)

## 210. The whole photo gallery (T4)

Built 2026-10-24. Unit-tested: pages and `next`, the kept pages joined and bounded, the ids a fetch may name, the bridge's `before` and `clipboard`, the Phone page's video, HEIC Copy, More and Save the chosen (headless). Checked on the OPPO against a throwaway server with 30 photos and a video pushed into `DCIM/Camera`: the newest page of 24 with more waiting, newest first, the video marked, every thumbnail kept, the next page joined with no repeats, and a Copy taken by the phone.
- [x] Pages, the video among them, thumbnails, a Copy asked (OPPO, 2026-10-24).
Owed:
- [ ] On a LAN: Save the chosen landing, a video opening in the player, a HEIC photo's PNG on the computer's clipboard. (where: by hand)
- [ ] The desktop app's Phone page by hand: More photos, the ticks and their limit of 20, a page of screenshots only. (where: by hand)
- [ ] "Its videos too" asking for video access; Settings in Spanish and Tamil. (where: by hand)

## 211. Files to a KDE Connect device (T6)

Built 2026-10-24. Unit-tested: the send with real TLS on loopback, each side with its own KDE identity (the file byte for byte, a device with another certificate refused), the CLI finding a device by name or id, the desktop listing it as a target and a drop not going to it. No KDE Connect device was reachable here.
Owed:
- [ ] KDE Connect on Android, paired: a file from the Files page and from `pocketdesk send <device>` lands in its Downloads, its name and date kept; two files one after the other. (where: by hand)
- [ ] KDE Connect on a Linux desktop (or GSConnect), the same. (where: linux)
- [ ] The device not connected: the send fails in the Sent list with why; a folder refused with why. (where: by hand)
- [ ] Windows: `pocketdesk install` opening TCP 1739-1764, and a device coming for a file through the firewall. (where: by hand)
## 212. The phone's gamepad as a game controller on Linux (T7)

Built 2026-10-24. Unit-tested: the packet as the pad's events (changes only, sticks, triggers, the hat, X and Y as xpad's). The Linux code was type-checked and linted for `x86_64-unknown-linux-gnu` in a scratch crate; CI builds it.
Owed:
- [x] Linux with `/dev/uinput` access, the phone in a session by the internet road: "Pocketdesk Gamepad" (USB 045e:028e, an event and a `js` node) made at the first press; `evtest` saw A as `BTN_SOUTH`, X as 307 (`BTN_X`), Y as 308 (`BTN_Y`), the D-pad's up as `ABS_HAT0Y` −1, each let go (Azure VM, Ubuntu 24.04, 2026-10-24).
- [ ] Both sticks and the other buttons in `evtest`. (where: linux)
- [ ] A game or Steam's controller test seeing an Xbox 360 pad, A/B/X/Y in their places. (where: linux)
- [x] A held while the app was stopped: let go once the server saw the session end (about 30 s later) (Azure VM, Ubuntu 24.04, 2026-10-24).
- [ ] The same with a stick pushed. (where: linux)
- [ ] Without uinput access, or with the setting off: keys as before, said once in the log. (where: linux)

## 213. Lock when the phone leaves on Sway and Hyprland (T8)

Built 2026-10-24. Unit-tested: the idle seconds from when the compositor said idle. The Linux code was type-checked and linted for `x86_64-unknown-linux-gnu` against the locked wayland-protocols; CI builds it.
Owed:
- [x] Sway (headless): with the phone's "Lock this computer when it leaves" on and the keyboard untouched, the phone gone (Wi-Fi off, its link ended) locked the computer once, at the set 2 minutes, the idle time from `ext-idle-notify-v1` (version 1 there) (Azure VM, Ubuntu 24.04, 2026-10-24). Found: a phone that left while the computer had data for it stayed linked for over 15 minutes (fixed in U1's own PR, `TCP_USER_TIMEOUT`).
- [ ] Sway: touched meanwhile, it does not lock. (where: linux)
- [ ] Hyprland, the same; and a video playing (an idle inhibitor) not keeping it from locking where the compositor offers version 2. (where: linux)
- [ ] A compositor without `ext-idle-notify-v1`: never locks, as before. (where: linux)
- [ ] V5: the desktop app's Settings shows "Lock after the phone leaves" on Linux, and a change there is taken without a restart. (where: linux)

## Recording results

Update `AGENTS.md` Current Status (mark verified items / list failures) and
record real device results — items marked
"device behavior unverified" are not done until this file says they are.
