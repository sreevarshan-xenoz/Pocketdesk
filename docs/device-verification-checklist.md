# Device Verification Checklist

The ordered battery to run against a real phone (arm64, Android 8.0+/API 26, debug APK with the bridge
`.so`) and a real desktop. Work top-to-bottom: later sections assume earlier ones pass. Record failures as
`FAIL: <symptom>` next to the box; each item maps to the commit/feature noted so fixes land in the right
place.

Several sections are now ticked from real runs (OPPO CPH2359 over LAN and tailnet, 2026-09-22 onward).
What is *not* ticked is not verified, however confident the code looks — and §22 is the standing register
of exactly that, including the one fix that merged without its confirming observation.

## 0. Prerequisites

- [ ] Desktop: `cargo run -p pocketdesk-server` (or installed server) on the
      Hyprland machine; FFmpeg + PipeWire + xdg-desktop-portal present.
- [ ] Phone: `adb install -r android/app/build/outputs/apk/debug/app-debug.apk`
      (build it first: `cargo ndk` for the bridge `.so`, then `assembleDebug`).
- [ ] Both on the same Wi-Fi LAN; phone can reach desktop port 1716 (TCP)
      and the QUIC streaming port (default 50000-range per config).
- [ ] Grant the app POST_NOTIFICATIONS when prompted (connect screen).
- [ ] Desktop: user in `input` group (uinput + `/dev/input/event*` read for
      privacy mode) — `groups $USER` should show `input`.
- [ ] Note which items are Hyprland-specific (window picker, workspace HUD,
      monitor geometry) — retest on a plain session if that's a target.

## 1. Connect + pairing gate (Tier 1 #5, Tier-2 #11b)

- [ ] Enter desktop IP + streaming port + control port → "Connect" opens a session.
- [ ] First connection: Pairing sheet auto-opens (pairing_required default true).
- [ ] "Show PIN on desktop" → desktop pops a notify-send PIN → phone confirms "Paired ✓", button flips to "Paired ✓".
- [ ] Second connect to same host: no pairing prompt (persisted both sides).
- [ ] Manual flow: `pocketdesk pair` on desktop → type 6-digit PIN on phone → paired.
- [ ] Wrong/expired PIN → clear error, retry works.
- [ ] With an *unpaired* second phone: control features (clipboard, monitors) must stay locked out.
- [ ] QUIC gate (v2 + video): before pairing completes, video must not flow — desktop log shows
      "Streaming session rejected: device not paired" (or "Unpaired device … on v2 QUIC") and the
      phone's status chip goes Down; after pairing, video works with no app restart beyond Retry.
- [ ] QUIC gate over WAN (iroh): unpaired dial is rejected the same way (desktop log + no frames).

## 2. Session shell + video (Tier 1 #5, R2#2, R2#5)

- [ ] Video appears within ~1 s; with "Show connection details" on, the HUD shows fps/kbps/rtt/delay/lost, plus the caption naming what
      the rates and the link figure were measured over (§2d).
- [ ] FGS notification is present ("Streaming to <ip>"), survives home/swipe-away attempts; its Disconnect action ends the session.
- [ ] Screen-off for 1 min, wake → stream still live (wake lock + keepalive; plan #13).
- [ ] Drop phone far from AP / saturate link → frames stall, then recover; the HUD's `lost` rises but no permanent freeze (gap-driven keyframe request, R2#5).
      "Drops" means **frames the phone never received** — the holes in the server's per-frame sequence
      numbers, counted where only the client can count them. So a saturated link must move it (the
      desktop's transport task trims its backlog before a stall clears), while a frame that arrives late
      and intact must not: the number can come back down when an out-of-order frame lands, and that is
      correct, not a bug. A decoder that cannot keep up is a different failure and shows as fps below the
      desktop's `src` rate, not here.
- [ ] Status chip: kill the server → "Down: <reason>" chip + Retry rebuilds after restarting the server.
- [ ] R4 B1 screencopy backend (desktop-observable): starting a stream on
      Hyprland must NOT raise the portal screen-share grant dialog; server log shows
      "Capture started via backend Screencopy". If instead it shows "Capture backend
      Screencopy unavailable (…); trying next" followed by "Capture started via backend
      Portal", the reason is in the line — that's the fallback working, but note it as a
      finding. `POCKETDESK_SCREENCOPY=0` forces the portal path.
- [ ] R4 B3 capture_backend config override: set `capture_backend = "portal"` in
      config.toml, restart, start a stream → log shows "Capture started via backend Portal"
      and NO screencopy attempt line (pipeline pinned). With `capture_backend = "screencopy"`
      on a NON-wlroots/portal-only Wayland, the server must fail the session with a
      "Capture backend Screencopy unavailable" error rather than silently falling back to the
      portal (explicit intent surfaces). Omitting the key (or `"auto"`) restores the
      screencopy→portal fallback above. An unknown value (e.g. `"v4l2"`) makes the server
      refuse to start with a TOML parse error.
- [ ] R4 B1 idle pacing + freshness: keep the desktop static → `src` fps drops to ≤10
      (damage-driven VFR); move a window → back to target fps within a frame or two, no tearing
      or upside-down frames (YInvert handling).
- [ ] R4 B1 multi-monitor: pick monitor N in the picker → log line "Screencopy capture: output at
      (x,y)" shows that monitor's wl_output origin and frames are its size.
- [ ] R4 C1 codec negotiation, default off: connect normally → server log shows
      "Client codec caps negotiated" with `codec=H.264` even on a HEVC-capable phone
      (allow_hevc defaults false); video unaffected.
- [ ] R4 C1 forced HEVC round-trip: `allow_hevc = true` in config.toml, restart server,
      reconnect → log shows `codec=H.265 (HEVC)`; phone decodes (sniffed `video/hevc`)
      with video within ~1 s and input still mapped. A pre-C1 phone build (no caps
      stream) must still get H.264.
- [ ] R4 C1 HEVC + window crop: pick a window during a HEVC session → encoder rebuild at
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
- [ ] R4 C2 mid-session stall, same-codec: force the supervisor on an H.264 session (kill its
      sidecar child) → video recovers on software x264 with NO phone toast (identical bitstream —
      the server log line is the only signal; that is expected).
- [ ] R4 C2 bottom rung exhausted: with software already live, keep killing encoders until the
      log shows "Encoder bottom rung exhausted; ending session" → stream goes Down cleanly
      (status chip + Retry works), never a silent infinite stall.
- [ ] R4 C2 crop stability: during a software-fallback session, pick/clear a window crop →
      encoder rebuild must NOT re-log the hardware fallback (sticky `encoder_preferred` — a
      rebuild must not re-probe the dead hardware).
- [ ] R4 C3 NVENC power pin (**NVIDIA hardware only — skip on non-NVIDIA boxes**): start an NVENC session (server log "Video encoder sidecar" on `h264_nvenc`/`hevc_nvenc`) → server log "pinned NVENC GPU 0 graphics clock to N MHz"; `nvidia-smi -q -d CLOCK -i 0` shows the graphics clock held at its max. End the session → log "restored NVENC GPU 0 to default clock management" and `nvidia-smi -q -d CLOCK` shows clocks back under adaptive management. On an unprivileged desktop the pin self-declines (log "clock lock refused ... running unpinned") and no reset is issued.

## 2c. In-process VAAPI encode (R4 C4) — desktop/box-observable, no device needed

Note: there is **no `hardware_encoder` key in config.toml yet** — the streaming server constructs
`StreamingConfig` with `HardwareEncoder::Auto` (`streamer.rs`), which `VideoEncoder::new` resolves via
`resolve_encoder(Auto, probe_encoders())`. So on a live session the exact rung is whatever Auto picks;
the deterministic C4 proof on the host is the unit test below, and the desktop-observable checks are the
backend log line + the `POCKETDESK_VAAPI_DEVICE` override (honoured by both the in-process and sidecar
paths regardless of the config knob). Forcing a *specific* encoder end-to-end is a known future knob,
not part of C4.

- [ ] R4 C4 host proof (no phone): `cargo test -p pocketdesk-core --lib encoder_vaapi` → 
      `test_vaapi_encode_roundtrip` encodes 40 real frames through `h264_vaapi` on this box's Intel
      iHD node and asserts Annex-B packets + ≥1 keyframe (self-skips green where no VA node exists).
- [ ] R4 C4 device-node selection: on the hybrid Intel+NVIDIA box the probe must land on the
      VA-capable node, NOT the hardcoded NVIDIA `renderD128` — `POCKETDESK_VAAPI_DEVICE=/dev/dri/renderD129`
      starts a session → server log "Video encoder: in-process VAAPI (h264_vaapi) on /dev/dri/renderD129"
      and **no `ffmpeg` sidecar child** is spawned for the vaapi path (`pstree` of the server). Point the
      override at a VA-less node (e.g. `/dev/dri/renderD128`) → it degrades to sidecar-then-software (C2),
      never hangs.
- [ ] R4 C4 quality (needs a phone): a VAAPI-encoded session decodes with correct color (no NV12 chroma
      shift) and the GOP (fps×2) keyframe lets first-frame render land within ~2 s.

## 2a. Desktop-side session visibility + kick (R4 D2)

- [ ] Consent notice: first phone connect → desktop shows a "Pocketdesk: a device is
      streaming this desktop" notification with device id + LAN/WAN + peer (headless: the same
      line appears in the server log instead).
- [ ] `pocketdesk status` during the session → tailscale output plus a "Streaming sessions
      (live):" line (short device id, transport, peer, up-seconds). With no session: "none live".
- [ ] `pocketdesk kick <id-prefix>` (≥6 chars, or full id, or peer IP) → phone shows the Down
      status chip within ~2 s and the status line disappears; other sessions are untouched.
- [ ] `pocketdesk kick all` ends every live session at once.
- [ ] kick with no running server → "no running Pocketdesk server to kick through", nothing
      written; a bogus target logs "kick: no live session matched" and the live session survives.
- [ ] Stale request safety: with no daemon running, hand-write `$XDG_STATE_HOME/pocketdesk/kick`
      (`all\n<unix-secs older than 60s>`), start the daemon, connect a phone → the session must
      NOT be killed (the expired request is dropped with a warn on first tick).
- [ ] Phone disconnect (normal Exit) → status line goes away within ~2 s; no leftover session in
      `live_sessions.json` (registry deregisters on pipeline exit).

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
- [ ] The stall check this replaces: freeze the desktop's output (sleep the display, or `SIGSTOP` the
      server) → HUD `fps` and `kbps` must reach 0 within ~3 s and stay there, with the caption still
      naming a real span. Under the old lifetime average this was the bug: a dead link read healthy for
      minutes. Resume → the rates recover within ~3 s, not within a session's worth of averaging.
- [ ] Basis reset: end the session and reconnect to the same (or a different) desktop → the caption must
      come back up from a small span and `rtt median of 1`, never showing a figure or a sample count
      carried over from the session that just died.
- [ ] Portrait legibility: the caption is one line of 10 sp monospace; in portrait on a narrow phone it
      must not wrap into a single-character column the way the six metric chips did before they became a
      `FlowRow`, and must not be clipped by the HUD panel's padding.
- [ ] Optional cross-check: while the caption reads `rates over 3.0 s`, `pocketdesk sessions` for the same
      session should report a mean kbps within the same order of magnitude — the session record averages
      over the whole session, the HUD does not, and a large disagreement is a measurement bug in one of
      them.

## 3. Input paths (R2#4, Tier 1 #2)

- [x] Direct-touch: tap lands at the touched point. **Verified 2026-09-22 on OPPO CPH2359 → Hyprland:** taps map pixel-exact through the letterbox (phone (540,1230) → cursor (768,432) = desktop center). Under Wayland the injector uses uinput (enigo/XTEST is inert on Hyprland): motion rides the "Pocketdesk Virtual Abs Pointer" device (ABS_X/ABS_Y + BTN_LEFT, created eagerly at startup so the first tap is not lost); keyboard/relative ride "Pocketdesk Virtual Input".
- [ ] ...and a tap is actually a **click** (roadmap 2061, re-verify 2026-09-24). The 2026-09-22 note above
      claimed "click (BTN_LEFT) lands", but the code that ran sent absolute motion plus a left *release* and
      never a press — so that half was either an inference or a window reacting to something else. A tap
      must now put `BTN_LEFT` down (value 1) and up (value 0) at the touched point: check with `evtest` on
      the abs-pointer device rather than by whether a window happened to react.
- [ ] Drag (roadmap 2061, shipped 2026-09-24, device-unverified): start a slow drag-select across desktop text — `evtest` should show the pointer warp to the touched point, `BTN_LEFT 1`, moving `ABS_X/ABS_Y` while held, `BTN_LEFT 0` on lift, and the selection must follow the finger (before this change a drag was motion-with-button-up: nothing began, and the release landed on a button that was never pressed). Then the leak check: begin a one-finger drag and add a second finger mid-drag (pinch) — the button must still come up, and the desktop must not be left dragging. Note there is no server-side safety net for a lost release packet yet (roadmap 2272): a session that dies mid-drag leaves `BTN_LEFT` held on the uinput device until the next release.
- [ ] Trackpad mode: pointer moves without jumping to touch point; tap = click; two-finger = scroll.
- [ ] Mode toggle switches behavior live, no session restart.
- [ ] Keyboard: type letters/digits/modifiers via on-screen shortcuts + remote input — check against `evtest`: Super, Alt+Tab, Ctrl+Alt+Del, PrtSc, Esc, Super+1..9 work (ShortcutBar). (Esc tap verified 2026-09-22; this box binds workspace cycling to Super+Tab, so the bar's Alt+Tab is config-dependent, not a pipeline failure.)
- [ ] The same keyboard list **run twice, once per injection backend** (roadmap 2057-2060). Under
      Wayland the server prefers uinput and emits the evdev code the phone sent, so nothing is
      translated; the enigo/X11 rung is the one that has to name every key, and it is the rung a
      machine without `/dev/uinput` access lands on. Read which one is live from the server log
      ("using uinput" vs "using enigo"), then re-run the list with the other by starting the server
      as a user who cannot open `/dev/uinput`. Symbols matter most here: `- = [ ] ; ' \` , . /` and
      the dialpad `*`/`+` were refused outright on that rung before the table was completed.
- [ ] holdKey/repeat (e.g. hold an arrow key) behaves.
- [ ] R4 D1 view-only: tap "View-only: off" on the session bar → label flips
      ("View-only: on", amber), desktop log shows "View-only mode changed" (enabled=true), and
      EVERY remote input goes dead on the desktop (taps, shortcuts, workspace chips) while video,
      StatsHud, clipboard sync and monitor/window pickers keep working. Toggle back → input restored
      without reconnect. Re-arm check: while view-only is ON, trigger a stream Retry / monitor switch
      (fresh server pipeline starts interactive) — the phone must re-send the latch once the stream
      is Up again; verify input stays blocked across that rebuild.

## 4. Zoom + display mapping (Tier 1 #6, R3#7)

- [ ] Pinch-zoom in direct-touch mode pans/clamps correctly; taps still land on the visible desktop point while zoomed.
- [ ] Trackpad mode: two-finger still scrolls remotely (no zoom capture).
- [ ] Letterbox: on a phone/desktop aspect mismatch, taps map through the letterbox offset (no systematic tap drift).

## 5. Window + monitor streaming (R3#7, Tier-2 #10)

- [ ] Window picker lists Hyprland windows with title/class/workspace/size + "focused" marker.
- [ ] Picking a window: video re-encodes at window size (encoder rebuild), crop tracks the window, taps map to desktop coords via the screen box.
- [ ] Moving the window mid-session: crop follows only after re-pick (known limitation — crop is static rect).
- [ ] "Whole desktop" clears the crop.
- [ ] R4 B2 (Hyprland): pick a window → server log shows "Streaming compositor-cropped window frames"; partly-occluded window streams correctly (occluder NOT in the video), and only the window's pixels are encoded.
- [ ] R4 B2: close the streamed window mid-session → stream keeps running on full-output frames with software crop (warn log "did not stage"; no session drop).
- [ ] R4 B2 (non-Hyprland wlroots, no export global): window pick still works via software crop exactly as pre-B2.
- [ ] Monitor picker lists monitors via xcap; selecting index N streams that monitor's region; reconnect persists the choice per host.
- [ ] Mid-session monitor switch tears down + rebuilds cleanly (no zombie session — bridge "already active" path).

## 6. Workspace HUD + battery (R3#8, Tier-2 #11a)

- [ ] Workspace chips appear under StatsHud; open/close/focus a window on desktop → HUD updates within ~4 s.
- [ ] Tap a chip ≤9 → Super+N injected → workspace switches.
- [ ] R4 E1: a row of window chips (title/class) shows under the workspace chips on a Hyprland session; tap one → video re-encodes cropped to that window (same as the picker), the tapped chip goes solid/highlighted.
- [ ] R4 E1: re-tap the highlighted window chip → crop clears back to whole-desktop (monitor) view; a *different* window chip tap switches the crop directly.
- [ ] Roadmap 2066 (needs a second output or a hotplug-capable desktop): with the HUD open, unplug/replug a monitor → server log shows `Hyprland output changed, refreshing compositor state` within a second of the event (not up to 4 s) → HUD chips and window-crop geometry re-layout immediately. Never observed here: this box has one built-in output and Hyprland's socket1 write dispatchers are broken upstream, so no fake hotplug exists.
- [ ] Battery chip shows desktop percentage + charging state; matches `upower`.

## 7. Clipboard + files (Tier 1 #4)

- [ ] Copy on phone → desktop clipboard updated within ~4 s; copy on desktop → phone receives.
- [ ] No echo loop (toggle sync off/on, paste both ways repeatedly).
- [ ] History sheet: 20 entries survive app restart; pick restores local + pushes.
- [ ] Share text to the app from any Android app → lands in desktop clipboard, toast "Sent to clipboard."
- [x] Share a file (EXTRA_STREAM) → appears in the desktop's Downloads folder. The desktop connects to the phone and presents the one-time key from the request; the phone serves the bytes only then. Verified on 2026-10-13 against the Windows host over LAN: 300,000 and 1,500,000 bytes with matching MD5, a second share of the same name saved as `name (1).ext`, the Mark of the Web on the received file, no `.part` left behind. Before this the phone and the server both listened and neither connected, so no file share had ever completed. It needs the TCP control channel on a LAN address; not over `adb reverse` (the desktop would dial itself), and not over the v2 channel or WAN.
- [ ] Reboot phone → history + saved hosts survive.

## 8. Notifications + find-my-device (R3#11c, Tier-2 #11)

- [ ] `notify-send -a Test "Hi" "body"` on desktop → phone notification appears (channel "Desktop notifications").
- [ ] Inline reply from the shade → desktop `replies.log` records it, text on desktop clipboard, confirmation popup; originating notification disappears.
- [ ] "Ring PC" → desktop plays sound 30 s (pw-play/paplay path; silent no-op is FAIL here since desktop has audio).
- [ ] Desktop pushes findmydevice ring → phone alarm loops at max volume 30 s (phone in alarm-volume >0; check DoNotDisturb doesn't block USAGE_ALARM).

## 8a. Control-channel versioning (R4 D4)

- [ ] Reply still works end-to-end after the `kdeconnect.notification-reply` →
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

- [ ] "Privacy: on" → physical keyboard/mouse on desktop stop responding (`evtest`: grabs held), while phone input still flows.
- [ ] Compositor shield (R4 E4, Hyprland/wlroots): the same "Privacy: on" also paints a full-perimeter red frame on every monitor within ~1 s; the desktop centre stays visible and interactive from the phone (the ring is `KeyboardInteractivity::None`, click-through). Desktop log shows "privacy: layer-shell privacy shield engaged".
- [ ] Leave session with privacy on → release happens (phone app exit); also verify 10-min TTL auto-release by force-killing the phone app mid-grab.
- [ ] Shield tears down with the grab: "Privacy: off" → the red frame disappears immediately; force-killing the phone app mid-grab → frame gone after the TTL auto-release.
- [ ] Non-wlroots desktop (GNOME/KDE/X11, or `POCKETDESK_SCREENCOPY=0` headless): no shield is attempted — grab still works, no crash, no "shield engaged" log line.
- [ ] "Lock PC" / session-notification "Lock desktop" → screen locks (loginctl path).
- [ ] Without `input` group: clean error, no crash.

## 10. Audio control (Tier-3 #16)

- [ ] Audio sheet opens with volume %, mute state, sink list (matches `wpctl status`).
- [ ] Slider → desktop volume changes on release; mute switch works; selecting a sink routes default output (verify with `wpctl inspect` + real playback).

## 10a. Phone mic share (R4 E2)

- [ ] "Mic: off" tap → RECORD_AUDIO permission dialog; denying it toasts "Microphone permission denied" and the toggle stays off.
- [ ] Granting it → button reads "Mic: on" (amber) and the desktop gains a source: `pactl list short sources | grep pocketdesk_mic` (or pavucontrol → Recording).
- [ ] Speaking moves the pavucontrol input meter for "Pocketdesk Mic"; `pw-record --target pocketdesk_mic /tmp/mic.wav` captures audible speech (play it back).
- [ ] "Mic: on" tap → source disappears immediately; `pgrep -f pocketdesk_mic` empty (no leaked pw-loopback children after repeated toggling).
- [ ] View-only latched → mic keeps flowing (it is session media, not injected input); input is still dropped.
- [ ] Exit the session while mic is on → source disappears (release-on-dispose); force-killing the phone app → source gone within the QUIC idle timeout (~45 s).
- [ ] Screen off / PiP while mic is on (Android 14+): audio keeps flowing — the FGS carries the microphone type (logcat: no SecurityException, no silent AudioRecord).
- [ ] WAN (relayed or direct) session: the same checks pass over the iroh path.

## 11. WAN over iroh (R1)

- [ ] On same LAN: identity cached (logcat), status shows LAN.
- [ ] Phone on LTE (Wi-Fi off): dial from cached identity succeeds → badge shows "WAN · punching…" then settles to "WAN · direct" or "WAN · relayed — trying direct…", video + input + keyframe-on-gap all work through relays (R4 A1).
- [ ] Desktop reconnects (restart server) → identity re-announced within 30 s, phone re-dials.
- [ ] Hole punching through a real NAT (relay-free if possible): check `directAddrs` paths get used.
- [ ] After both a LAN and a WAN session: `pocketdesk sessions` shows one line per session with distinct `outcome=` (`lan_direct` vs `wan_punched`/`wan_relayed`), plausible `rtt_ms`/`kbps`, and `dev=` = the paired deviceId.
- [ ] Tail latency (roadmap 2141-2146): the same lines carry `enc_n`/`enc_p50`/`enc_p90`/`enc_p95`/`enc_p99`/`enc_max` and the matching `rtt_*` set. `enc_n` counts frames the encoder actually produced a packet for, so it must be non-zero and of the same order as fps × session duration — a large shortfall is its own finding (frames dropped upstream, or a path that never samples). `enc_p50` must sit near the typical frame time for the encoder rung in use (the log line names the backend at session start), and a deliberate stall — crop to a window, or unplug/replug Wi-Fi mid-session — has to move `enc_max` or `rtt_max` above `enc_p50`/`rtt_p50`. **A line with no `enc_*` at all means the sampling was never wired to the live pipeline: that is a bug, not a healthy session.**
- [ ] Device-reported tails (roadmap 2143-2146, the phone's half of the chain): the same lines must carry `dec_*`, `rnd_*` and `e2e_*` plus `phone_rtt`/`phone_lost`. `dec_n` counts frames the decoder actually rendered, so it has to track `enc_n` within the loss the link imposed (a `dec_n` far below `enc_n` is dropped-or-stalled frames, not a telemetry bug), `dec_p50` must be single-digit ms on a hardware rung and grow when the desktop is cropped to a bigger window, and `e2e_p50` must sit near the HUD's e2e figure for the same session. `rnd_p50` is the rendered-frame interval, so it should be near 1000/fps ms and must jump when the bitrate clamp or a preset slows the link. On a session that never delivered video (rejected, or killed before the first frame) `dec_*`/`rnd_*` are legitimately absent — that is the "nobody measured" case, and `key=0` in their place would be a bug. **The server side of this path is already proven without a phone: `cargo test -p pocketdesk-server --test session_record -- --ignored --nocapture` drives a real capture→encode→send→receive session over loopback and asserts the record carries all of these tails; what remains device-only is that `H264Decoder.kt` feeds `dec_*`/`rnd_*` with real MediaCodec timings rather than the numbers the test injects.**
- [ ] Transport exposure (roadmap 2151-2158): every line from a session that lasted longer than the 5 s sampler must carry `lost_pk`/`lost_b`/`dgrams`/`tx_b`/`path_chg`/`relayed_s`, and `cong_ev`/`cwnd_pk`/`mtu`/`black` **on either transport**: quinn exposes them on the connection, and the iroh arm reads them off the *selected* path (`Connection::paths()`), so a WAN session riding a punched direct path reports the same set. A WAN line missing them is no longer the expected shape — the earlier "iroh drops the per-path values" claim was read off its connection-level aggregate, which really does discard them — and the only legitimate reason for a gap now is a snapshot taken with no selected path, which is why the field is `Option` and never a `0` pretending to be a measurement. `mtu` must be ≥ 1200 (quinn's `INITIAL_MTU`) and may climb as PLPMTUD probes pay off; `cwnd_pk` must be non-zero on any session that sent video. Walk between Wi-Fi bands, or kick the phone onto cellular mid-session, and `path_chg` has to count it; start a relayed WAN session and `relayed_s` must be non-zero with `outcome=wan_punched` if punching eventually won.
- [ ] Relayed → direct upgrade while streaming: badge flips to "WAN · direct" on its own (iroh keeps punching) and the bridge logs "upgraded from relay to direct path" (logcat). Relayed session must stay usable the whole time, not stall on the transition.
- [ ] Relayed session (R4 A3): within ~2 s of the badge reading "relayed", the desktop log shows "Encoder bitrate target changed" (relay_cap engaged) at effective ≤ 2 Mbit/s followed by "Encoder rebuilt for new bitrate", and picture is visibly softer than LAN/direct.
- [ ] Full-quality override (R4 A3): while relaying, the badge area offers "Full quality: off" — tap → log shows "Relay quality override changed" then a fresh "Encoder bitrate target changed" back to configured (`pocketdesk sessions` kbps over the next session line). Tap again to re-clamp; toggle survives a stream retry (re-arms).
- [ ] Clamp release on punch-through: start relayed (override off), wait for the direct upgrade → within ~2 s the log shows "Encoder bitrate target changed" back to configured without touching anything.
- [ ] LAN session (R4 A3): the relay clamp never engages (`stats().relayed` is always false on quinn → relay_cap is unbounded); the arbiter task still runs but emits no bitrate-change line until a preset is chosen below or the link starts losing packets.
- [ ] Loss response (roadmap 2053): degrade the link — `tc qdisc add dev <iface> root netem loss 5%` on the desktop, or walk the phone toward the far edge of the Wi-Fi mid-session — and within a few 2 s ticks the desktop log shows "Encoder bitrate target changed" carrying a `loss_cap=` field with effective below configured, then "Encoder rebuilt for new bitrate" and a visibly softer picture. The cut is 20 % per congested tick down to a 1 Mbit/s floor (a configured rate below the floor is never raised to it). Restore the link → the same line reappears **without** `loss_cap=` as the rate climbs back ~10 % per tick to configured, then stops being a term at all.
- [ ] Loss response must not fire on a quiet desktop: hold the screen still (no window movement, no video) for a minute and the log must show no bitrate-change line. Below 100 datagrams in a tick the controller treats the sample as no information, which is the only thing distinguishing "the link is lossy" from "there was nothing to lose" — a bitrate cut on an idle LAN session is a bug, not a measurement.

## 11b. HUD link-profile presets (R4 E5)

- [ ] Session bar shows "Quality: auto" by default. Tap it once → "Quality: max" (blue), then "balanced", "economy", back to "auto" — the button cycles all four.
- [ ] On LAN, pick "economy": desktop log shows "Link-profile preset changed" then "Encoder bitrate target changed" with effective ≤ 1.5 Mbit/s and "Encoder rebuilt for new bitrate"; the HUD bitrate/`pocketdesk sessions` kbps drop and the picture softens. Native resolution is NOT reduced (only bitrate) — verify the HUD reports the same width×height.
- [ ] Pick "balanced": effective ≤ 5 Mbit/s. A configured rate below a band (low-res monitor) is never raised above it (the band is a ceiling, not a floor).
- [ ] Preset survives a stream retry / monitor switch: choose "economy", trigger Retry — the new pipeline re-arms the preset (stays ≤ 1.5 Mbit/s without re-tapping). Server latches are per-session, so a full reconnect to a fresh `StreamingServer` resets to Auto (phone must re-pick) — expected.
- [ ] View-only interaction: while a preset is active, toggling View-only still drops injected input but the bitrate preset keeps applying (both are control-plane).
- [ ] A preset change mid-action is seamless: no freeze longer than one keyframe; the post-rebuild IDR reseeds the decoder cleanly (no green/garbled persist).
- [ ] Auto is a no-op on (re)connect: with "Quality: auto" and a LAN session, tapping Retry sends no preset packet and logs no "Encoder bitrate target changed" (only the relay floor would fire, and not on quinn).


## 12. Roaming (Tier-3 #13 remainder)

- [ ] Mid-stream, toggle Wi-Fi off → phone falls to LTE → within a few seconds status chip shows connecting and a fresh WAN/LAN session comes up (reconnect-on-new-radio, not live migration).
- [ ] Wi-Fi↔LTE ping-pong with 3 s cooldown: flapping radio must not thrash a healthy stream.
- [ ] In PiP: network change must NOT tear the float session (pip guard).

## 13. PiP + multitask (Tier-3 #14, DeX)

- [ ] "PiP" button → chrome hidden, video-only float with correct aspect (matches decoded frame size after first SPS); input goes to system.
- [ ] Expand back → chrome returns, session uninterrupted.
- [ ] DeX/docked or split-screen: session is resizeable, letterboxing recomputes, taps still map correctly.

## 14. Blackout / pocket mode (Tier-3 #15 remainder)

- [ ] "Blackout" → black screen, "double-tap to unlock" hint, screen dimmed; stream continues (watch desktop from another device or check fps in logcat).
- [ ] Screenshot during blackout → blocked (FLAG_SECURE); Recents thumbnail blank/secure.
- [ ] Double-tap or system back unlocks; back never exits the session while blacked out.
- [ ] Entering PiP while blacked-out auto-clears the overlay.
- [ ] Pocket test: with overlay on, phone in pocket should not send spurious input (touch consumed by overlay).

## 15. Language / i18n (Tier-3 #17 remainder)

- [ ] Connect screen shows "Language" button on Android 13+ (absent on 12).
- [ ] Switch system per-app language to Español / தமிழ் → all screens, sheets, FGS notification text localize (telemetry units, key labels, window titles stay as-is by policy).
- [ ] Mid-session language change (if the OS applies live) → no crash; format strings (`%1$s`) render correctly in both locales.

## 16. Long-run / hygiene

- [ ] 1 h continuous session: memory stable (no decoder leak across resolution changes), fps steady, no watchdog kills.
- [ ] Force-stop app mid-stream: desktop auto-releases privacy within TTL, encoder sessions stop (no server-side zombie capture), siren/WoL relays unaffected.
- [ ] Airplane-mode mid-session then back: clean Down chip + retry, no ANR.

## 17. Foldable / tablet dual-pane (R4 E6)

- [ ] On a foldable folded to half-open (vertical hinge): session auto-splits — desktop video on one pane, trackpad + shortcut dock on the other; tapping/dragging the dock moves the desktop cursor and the finger never covers the video.
- [ ] Rotate to a horizontal hinge → split flips to top/bottom (stream top, dock bottom) and vice-versa.
- [ ] Tablet / unfolded (≥600 dp, no hinge): dual-pane auto-engages with a left|right split.
- [ ] "Pane: auto/dual/single" button cycles the override; forcing Single on a foldable restores the old full-screen layout; forcing Dual on a phone splits it.
- [ ] Toggling panes mid-session does NOT drop the stream (the decode session survives the fold — check fps continuity / no re-Keyframe gap).
- [ ] Direct-touch taps on the video pane still hit the right desktop point (letterbox/aspect-fit recompute for the half-width pane).
- [ ] Shortcut bar appears once — in the dock while dual, under the stream while single.
- [ ] Blackout still covers BOTH panes (whole window), not just the stream.
- [ ] es/ta: "Pane:" and trackpad hint localize.

## 18. Login autostart + tailnet access (server-side)

- [ ] Reboot the desktop → after reaching the Hyprland session, `systemctl --user status pocketdesk` is active without any manual start (WantedBy=graphical-session.target).
- [ ] Phone reconnects to the auto-started server WITHOUT re-pairing (QUIC identity persisted in `~/.local/state/pocketdesk/certs/`; a regenerated cert would trip the TOFU pin).
- [ ] Log out / session end → service stops with the session (PartOf), never orphaned.
- [ ] `install.sh` run → unit lands in `~/.config/systemd/user/`, ExecStart points at the real prefix path, prompt enables it; `--status` reports the user unit; `--uninstall` removes it and any legacy `/etc/systemd/system` unit.
- [x] Phone on the tailnet (not on the LAN): connect to the laptop's 100.x Tailscale IP → pairing + video + taps work end to end. **(verified 2026-09-22: oppo-reno8-5g <phone tailnet IP> → <laptop tailnet IP>)**
- [x] Re-verified 2026-09-22 with the new home screen over tailnet (both ends on the same Wi-Fi, Tailscale direct path): auto-connect + card tap → session Up (26 fps, 1.4 Mbit/s, rtt ~55 ms, drops 0), server log shows the peer as <phone tailnet IP>, and a direct-touch tap moved the desktop cursor (1142,37 → 768,390). Note: Alt+Tab and PrtSc shortcut buttons are no-ops on THIS desktop (no Hyprland binding for either) — not transport failures.
- [ ] Phone on cellular (Wi-Fi off): same connect via tailnet IP works (Tailscale traverses NAT). Then iroh WAN fallback (`## 11`) with the two machines on genuinely different networks — still pending (phone cellular was OUT_OF_SERVICE at test time).

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
- [ ] Multi-host scenarios (second card ordering, remove-then-readd) and es/ta strings for the new keys.

## 21. UI polish pass C1–C9 (design system, immersive session, error UX, settings)

First device run (OPPO CPH2359, tailnet) found the icon layer broken on-device —
the runtime-Canvas glyph set drew as a corner dot — so `ui/Icons.kt` now wraps
`res/drawable/ic_ll_*.xml` VectorDrawables tinted through `painterResource`.
`StatsHud` is a `FlowRow`, session chrome is inset-aware and self-describing, and
any touch on the stream summons faded chrome (`PointerEventPass.Initial`, so the
tap still reaches the desktop). The app also gained an adaptive launcher icon.
Items below are re-scored against that build.

- [ ] Theme follows the system light/dark; toggling the mode in Settings restyles every screen immediately (no restart), and the window background matches on cold start (no white flash in dark mode or vice-versa). *Partly verified: the Dark radio restyles Settings immediately; cold-start window colour not yet checked.*
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
- [ ] Siren: a desktop-pushed `kdeconnect.findmydevice {ring:true}` rings the alarm AND shows the full-screen dialog; Silence stops both; the 30 s auto-stop still works if the dialog is ignored.
- [ ] Clipboard sheet: Clear asks for confirmation first (Cancel keeps history); empty state has the clipboard icon.
- [ ] Haptics: tick on the action disc, mode toggle, picker picks and shortcut chips; heavier buzz on Exit; the Settings haptics switch mutes all of them live.
- [x] Session notification: real Pocketdesk status-bar icon (not the stock eye), Disconnect + Lock-desktop actions carry icons, channel shows its description in system settings. *`dumpsys notification` shows the custom `ic_stat_pocketdesk` resource and both actions; the shade renders the monitor glyph plus power/lock action icons, and the session channel carries its description.*
- [x] Shortcut bar: System | Workspaces groups with divider and localized captions; chips are ≥32 dp touch targets; key caps stay literal in es/ta. *Now a dark floating pill, legible over video; scrolls horizontally past PrtSc.*
- [x] es/ta: every Phase-C string shipped (locales in sync at 161 each).
- [x] Launcher icon: the adaptive icon renders on ColorOS (phone-into-monitor mark on the brand pine plate) instead of the default Android blob. Needed filled paths in a scaled group — stroke-only adaptive foregrounds are not rasterized there.

## 22. Owed verification register

**This is the gate for Phase 0 of [roadmap-execution-plan.md](roadmap-execution-plan.md) item 5 — nothing
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

- [ ] **PASS condition:** zero `Incoming v1 TCP connection from` lines (`server/src/service.rs:330`) after
      the single session-establishing connection, while every sheet and picker above is used.
- [ ] **PASS condition:** zero `Broken pipe (os error 32)` WARN lines from any plugin.
- [ ] **PASS condition:** zero `Kicked 1 stale session(s) due to reconnect storm` from the server.
- [ ] Battery/clipboard/monitors/window rows still show *current* values — the shared connection must not
      trade churn for staleness.
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
  see roadmap-3000 **2791-2800**, and §10's "audio" results are about *control* (routing/volume), not playout.
- The file browser — the server plugin answers, `listRemoteFiles` has zero call sites (**2874**).
- Notification per-app channels, grouping, icons, privacy modes (**2898-2905**).
- Every K (virtual display) and L (macro/automation) behaviour — neither subsystem exists.

## 23. Windows host (W1: core remote desktop)

Built on `feat/windows-host`; plan `docs/superpowers/plans/2026-10-09-windows-host.md`. Box-verified
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
      file appears under `%LOCALAPPDATA%\pocketdesk\logs`.
- [ ] A toast "Pocketdesk pairing PIN: …" appears for `pair` (the `a_toast_is_shown` test only proves
      Windows accepted it).
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
- [ ] Window crop: picking a window streams just that window; closing it returns to the monitor.
- [ ] A UAC prompt or Win+L mid-session: the stream recovers after it is dismissed (WGC keeps running; on
      the DXGI fallback, access-lost rebuilds the duplication).
- [ ] `pocketdesk stop` from a second terminal stops the server cleanly (the log ends with
      `Shutdown requested (pocketdesk stop)`) — run this yourself; it is deliberately not automated.
- [ ] `uninstall` removes the task and the firewall rules (`wf.msc`), including the "Pocketdesk" ones an
      older version created.

## 24. Desktop app and QR pairing

Built on `feat/pocketdesk`; spec `docs/superpowers/specs/2026-10-12-pocketdesk-desktop-app-design.md`.

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
- [ ] Settings → Save on a real `config.toml` keeps comments; the server picks the change up after Restart.
- [ ] One code, one phone: after a phone pairs, the Pair tab shows "Paired ✓" and no code, `pairing.pin` is gone, and "Pair another phone" shows a new code. Five wrong PINs typed on a phone make the tab say the code no longer works, and no new code appears until it is clicked. Leaving the tab, closing the window to the tray, and Quit each delete `pairing.pin`.
- [ ] Leaving the scanner (after a scan, and with Back) turns the camera off: the privacy indicator goes out at once, not when the app closes.
- [ ] Open a `pocketdesk://pair` link, finish or dismiss it, then switch dark mode or reopen the app from Recents: the pairing is not offered again. Switching theme while the confirmation is showing keeps it.

## 25. Desktop sound on the phone

Built on `feat/phone-audio`; spec `docs/superpowers/specs/2026-10-12-phone-audio-playback-design.md`. First device run 2026-10-12 (Galaxy A71, USB + WAN direct, Windows desktop): the player starts with the session, a 48 kHz stereo low-latency AudioTrack reaches `state:started` and keeps serving frames, and the playout thread averages about 4.5 % of a core. Not yet checked by ear: how it sounds, latency, the switch, focus with calls or other apps, PiP.

- [ ] Play a video on the desktop during a session: the phone plays its sound, and the lag behind the picture is small (≤ ~150 ms by ear).
- [ ] Quick Settings → "Desktop sound on this phone" off: silent at once; on again: sound returns within a second. The setting survives an app restart.
- [ ] Pause the desktop's audio for 10 s, then resume: it comes back cleanly, without a crackle burst.
- [ ] Connecting to a silent desktop does not pause music already playing on the phone; the first desktop sound takes over. "Silent" means no app holds an audio stream open: Windows loopback sends packets of silence while any app has one (seen on the A71 with a browser open: focus was taken 1.3 s after connect), and focus follows packets, not loudness.
- [ ] A phone call silences it, and it resumes after the call. Another app starting music takes over for good: switching "Desktop sound on this phone" off and on brings it back.
- [ ] In PiP the sound continues; Exit stops it.
- [ ] With sound on, the HUD's fps and delay match a session with it off, and taps on a still desktop feel immediate (the bridge lock fixes).
- [ ] `adb logcat -s DesktopAudio`: "desktop sound started", and on exit "stopped (N underruns)" with N small.

## 26. In-session keyboard

Built on `feat/in-session-keyboard`; spec `docs/superpowers/specs/2026-10-12-in-session-keyboard-design.md`.

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
- [ ] Landscape: the stream stays visible above the keyboard, and zoom survives opening and closing it.
- [ ] Linux host: ASCII typing over uinput. Non-ASCII and capitals in pasted text wait for roadmap F4 (2666).
- [ ] Tamil and Spanish keyboards: composing text arrives whole and correct.

## 27. Trackpad gestures

Built on `feat/trackpad-gestures`; roadmap F3 (`docs/superpowers/specs/2026-10-12-feature-roadmap.md`).

Checked on 2026-10-12 with the Galaxy A71 against Windows:
- a tap, then a touch-and-drag within 300 ms, selected a Notepad line (read back through UI Automation);
- the Trackpad speed slider renders and its value survives an app restart.

- [ ] Two-finger tap opens a context menu; three-finger tap middle-clicks (e.g. opens a link in a new tab). adb cannot make multi-finger touches.
- [ ] Two fingers resting without moving do not right-click, and a two-finger tap does not scroll the page first.
- [ ] Two-finger scroll is natural (content follows the fingers, like the phone itself) at about one wheel notch per 24 dp, in both axes. Unverified: adb cannot make two-finger touches.
- [ ] Tap, then drag a window by its title bar: it moves without maximising (no double click), and a tap left alone clicks about 0.3 s later.
- [ ] A drag survives a second finger landing mid-drag and releases on lift.
- [ ] Speed 0.5× and 3× feel right; slow movements still move the cursor.
- [ ] The foldable dock's trackpad behaves the same.

## 28. Permissions and session approval

Built on `feat/approvals-permissions`; spec `docs/superpowers/specs/2026-10-13-approvals-permissions-design.md`.

Checked on 2026-10-13 with the Galaxy A71 against the Windows host, over WAN. The answers were written to the answer file directly, as the app does; nobody clicked the prompt.
- with "ask first" on, the session is held, the request appears in `pending_approvals.json`, and the phone shows "waiting for the picture" with the approval hint;
- Allow once: the server logs `AllowOnce`, the list empties, and video plays;
- Deny: the server logs `Deny` and `session refused`, and the phone shows the refusal card about 2 s later.

- [ ] The desktop app's prompt: it comes to the front (also from the tray) and its three buttons work; Always allow turns the switch off.
- [ ] No answer: the phone shows the refusal about 30 s after connecting.
- [ ] Clipboard off: the phone's clipboard actions are dropped (`Denied … clipboard` in the log); turn it on again and they work at once.
- [ ] Running commands: denied by default; allowed after the switch is turned on.
- [ ] Control off: the session is view-only and the phone cannot lift it. Sound off: no desktop sound. Microphone off: the mic does not reach the desktop.
- [ ] Turning control, sound or the microphone off during a session ends that session; Retry starts one with the change. Removing a connected phone ends its session.
- [ ] LAN session: the prompt shows the phone's address.

## 29. Computers on this network

Built on `feat/lan-discovery`; spec `docs/superpowers/specs/2026-10-13-lan-discovery-design.md`.

Checked on 2026-10-13 with the Galaxy A71 and the Windows host on the same Wi-Fi:
- the host was found in about 0.2 s at its LAN address (not its link-local or Tailscale address), and the saved card, saved as 127.0.0.1 and matched by its paired id, showed "On this network";
- a second computer announced from the PC (a test announcer) appeared under "On this network" with its name, and disappeared live when it withdrew;
- with `lan_discovery = false` the server logs that it is not announced;
- a server killed without a goodbye left a cached record, which the phone did not list ("announced but does not answer").

- the confirmation dialog shows on tapping a found computer; Connect opened a session at the found address with the PIN sheet (an address not paired before), and nothing was saved;
- an announcement that lists an unreachable address first is listed at its reachable one.

- [ ] Pair from a found computer: after the PIN it appears under Saved.
- [ ] Android 17: the local-network permission prompt at first launch; LAN sessions and discovery work after Allow, and time out after Deny.
- [ ] Two computers on the network, both listed.
- [ ] Android 13 or older (the resolveService path).
- [ ] The Linux host is found (Avahi may also be announcing on the same host).
- [ ] "Visible on this network" in the desktop app, then a server restart: the computer no longer appears.

## 30. Sending files to the desktop

Built on `feat/file-transfer`; spec `docs/superpowers/specs/2026-10-13-file-transfer-design.md`.

Checked on 2026-10-13 with the Galaxy A71 against the Windows host over LAN. Three shares 1.5 s apart (2, 4 and 6 MB) queued on the phone and were sent one after another with matching MD5s. All three appear in `received.json`, and the final notification read "Sent 3 files to …".

- [ ] Share several photos at once from the gallery (`SEND_MULTIPLE`): "Sending 2 of 3" with a moving progress bar.
- [ ] The desktop shows a notification per file; the app's Files tab lists them, and Show in folder selects each in Explorer.
- [ ] A file from a provider without a size (e.g. some cloud apps) still arrives (copied first).
- [ ] A share with the desktop off or unreachable ends in "Not sent: 1 of 1" with the reason.
- [ ] A non-English file name arrives with its name.

## 31. Files both ways and the remote browser

Built on `feat/file-transfer-b`; spec `docs/superpowers/specs/2026-10-13-file-transfer-b-design.md`.

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
- [ ] Send a file while a large share from the phone is running; the offer still arrives.
- [ ] A phone that has no room declines an offer; the desktop's Sent list says "the phone declined it: …".
- [ ] Drag a file onto the desktop app's window; it is sent to the chosen phone (Windows, and X11 Linux).
- [ ] Tap "Saved … to Downloads"; the file opens.
- [ ] Download into a phone without enough space: "Not enough space for …", nothing left in Downloads.
- [ ] A file over 1 GB both ways, and a download stopped halfway (Wi-Fi off): no partial file is published.
- [ ] Android 8 or 9: the file lands in the app's own Downloads/Pocketdesk.
- [ ] The Linux host: browse `/home/<user>`, a download, and a send from the app's path field.

## 32. Switching screens

Built on `fix/monitor-switch`. Checked on 2026-10-13 with the Galaxy A71 against the Windows host (two monitors, 2560×1080 and 2560×1440):
- Quick Settings → Monitor switched both ways six times over LAN, with no crash.
- Three switches made while a WAN connect was still in flight all landed on the last pick. The old build crashed here with "Invalid to call at Released state".
- The chrome's chips 1 and 2 switched the stream each way with one tap.

- [x] Switch screens from the chips and from Quick Settings → Monitor; the picture follows, and the app stays up.
- [x] Switch while the stream is still connecting (WAN); no crash, and the last pick wins.
- [ ] Unplug the monitor being streamed; the session falls back to the primary screen.
- [ ] Three or more monitors: a chip each, in the desktop's order.
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
- [ ] Place the widget on the home screen: it names the last computer and connects on a tap; with none saved it opens the app.
- [ ] A Wake shortcut appears for a computer the phone can wake (§42), and sends the packet.
- [ ] Pin a shortcut to the home screen; it survives the computer being reconnected, and stops working once the computer is removed.
- [ ] Android 13 or older: the tile opens the session (the older collapse path).

## 34. Desktop diagnostics (F10)

Built on `feat/diagnostics`. On the Windows box, an ignored test (`cargo test -p pocketdesk-desktop the_checks_run -- --ignored --nocapture`) printed the report: right after a start the port checks wait; once running all six were ok.

- [x] Windows: the six checks against a running server; the "starting" window.
- [ ] Open the Diagnostics tab in the app and click Run checks; Copy debug report pastes the same text.
- [ ] A network Windows files as Public, with only `install`'s rules: the firewall check says no rule lets phones in.
- [ ] A Block rule for pocketdesk: the firewall check reports it.
- [ ] Linux: the checks with the systemd user service (no firewall line).

## 35. Reaching the desktop from outside (N1)

Built on `feat/wan-insight`. On the Windows box, the server recorded UDP working, a NAT that allows direct connections, and no public IPv6, and Diagnostics showed both lines.

- [x] Diagnostics shows "From outside" and "IPv6" from the server's own measurement.
- [ ] **Cellular:** the phone on mobile data, the desktop at home. A session connects, and the HUD says direct or relayed. (Owed: the test phone has no mobile service.)
- [ ] A strict-NAT network (or a hotspot behind carrier NAT): "From outside" warns, and the session rides the relay.
- [ ] A network with public IPv6: the IPv6 line is ok.

## 36. Your own relay (N2)

Built on `feat/own-relay`. On the Windows box, a phone WAN session ran through `pocketdesk relay --allow-paired` with `relay_urls` pointing at it, and the record said `wan_relayed`.

- [x] The desktop announces its own relay, and a phone session rides it.
- [x] Diagnostics says "ready through your relay", and "From outside" says "not measured".
- [x] The phone's own home relay is the user's relay too (logcat: `home is now relay http://…:3340/`), and its log names no n0 relay.
- [ ] **From outside:** the phone on mobile data, the relay on the desktop with port 3340 forwarded. A session connects through it.
- [ ] HTTPS mode on a cloud VM with a real name: the certificate is issued, and a session connects with `--allow` ids from `relay-ids`.
- [ ] A phone that is not paired (or an id not listed) is refused by the relay.

## 37. Found by id on the Mainline DHT (N3)

Built on `feat/dht-lookup`. On the Windows box (n0's relays, `dht_publish` at its default), the server published its record within 4 s of start and read it back a minute later. The A71's cached identity was then made stale on purpose (run-as on its prefs): a closed stream port so LAN fails, an n0 relay the desktop is not on, and no direct addresses. Input was off for the phone throughout.

- [x] Diagnostics' "Found by id" says "published on the Mainline DHT and read back (relay address only)".
- [x] The phone caches the identity with `"dht": true` after a LAN session.
- [x] Control: with `"dht": false`, the stale identity's WAN dial times out (`DHT lookup: false`).
- [x] With `"dht": true`, the same dial finds the record on the DHT in about 4 s, connects through the relay it names, and upgrades to a direct path (logcat: `discovered endpoint info`, `WAN session upgraded from relay to direct path`).
- [x] After the review fixes, again: the server's DHT joined in the background on a random port, published within 4 s and read back a minute later; the stale-identity dial found the record in about 3 s and went direct.
- [ ] **From outside:** the phone on mobile data after the desktop's home relay or public IP changed. A session connects, found by the DHT.
- [ ] `dht_publish = "addresses"` on a network with public IPv6: the record holds the IPv6 address, and a phone with relays unreachable connects directly.
- [ ] A network that blocks outgoing UDP: "Found by id" warns that it was not read back.
- [ ] The Linux host: the same checks.

## 38. Image clipboard (N4)

Built on `feat/image-clipboard`. On the Windows box, a 320x200 image put on the clipboard as a bitmap only (no PNG format, so the DIB path ran) reached the A71 during a LAN session as a 1196-byte PNG whose SHA-256 matched its id. It was sent once and never sent back. Input was off for the phone throughout.

- [x] Desktop → phone: a bitmap copied on Windows lands in the phone's clipboard cache as PNG, once.
- [x] After the review fixes: the same image copied again after some text in between moves again, and the text reaches the phone; only the newest image stays in the cache.
- [x] The Windows clipboard holds a PNG as both "PNG" and `CF_DIB`, and reads it back byte for byte (live ignored test).
- [ ] Desktop → phone: paste it into a chat app on the phone. (The A71 has no app that pastes images.)
- [ ] Phone → desktop: copy a photo on the phone and paste it into Paint. (The A71 has no app that copies an image: no browser.)
- [ ] With the phone's Clipboard permission off, neither direction moves.
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
- [ ] A messaging app that hides its content on the lock screen shows only "New notification".
- [ ] With no computer reachable, the phone's battery and the queue stay calm over an hour.
- [ ] The Linux host (`notify-send`).

## 40. Closed on the phone, closed on the computer (N5c, part 1)

Built on `feat/notification-dismissal`. On the A71 with sharing on for Shell: a notification was posted, its toast found in the Windows Action Center by its tag, and the notification swiped away on the phone. The phone sent one cancel, and the toast left the Action Center. Afterwards sharing was turned off and access revoked.

- [x] A notification swiped away on the phone leaves the Windows Action Center.
- [x] A live ignored test shows a tagged toast and takes it back.
- [x] Only a notification the computer was shown is mentioned (one cancel for a group of 35, of which one had been sent).
- [ ] The Linux host: a notification closed on the phone closes in the notification server (D-Bus `CloseNotification`).
- [ ] A chat app that clears its notification when read on the phone.

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
- [ ] A computer saved by its tailnet address counts as awake once its server answers.
- [ ] Turn "Wake from the phone" off on the desktop: after the next connection the phone forgets the MAC, and Wake disappears.
- [ ] The Linux host (ethtool's Wake-on, and the adapter chosen).

## 43. Several phones at once (N7)

Built on `feat/multi-phone`. Only one phone (the A71) is at hand.

- [ ] Two phones in sessions: the first to send input drives, and the other's taps do nothing while its session says "Another phone is driving".
- [ ] After 10 s without input from the driver (or its session ending), the other phone can take control.
- [ ] Sessions tab: the driver is marked "Driving"; Give control hands it over within a second.
- [ ] Settings "Several phones": every phone drives; the hand-back time.
- [ ] Two quick Ends in the Sessions tab both end their sessions.

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

R3 is tested in code (the table in `docs/superpowers/specs/2026-10-14-phase-r-reliability.md`). On a device:
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
- [ ] The phone's battery over an hour with the app in the background, and while a session is open.
- [ ] Network traffic while idle (both ends).

## 46. Phase R2: long sessions and recovery

On the A71 against this Windows box (LAN, release build, input off), 2026-10-14. The sampler logs, once a minute, the server's CPU, memory, threads and handles, and the phone app's memory, battery and temperature (`dumpsys meminfo`, `dumpsys battery`).

- [x] 35 minutes streaming: server 10–24 % of one core, private memory 84 → 96 MB, handles 674 → 705; the phone flat.
- [x] Wi-Fi off for 30 s: the stream came back by itself 12 s after the Wi-Fi did. **The control connection did not** (fixed; re-run: one reconnect, then steady).
- [x] The desktop's server stopped for 20 s: **the phone waited for Retry** (fixed; re-run: back 4 s after the server listened again, with no tap).
- [x] 30 minutes after the fixes: private memory 85 → 89 MB, handles flat at about 680, CPU 10–13 %.
- [x] Session start: one control connection that stays, plus two one-off queries.
- [x] An hour (2026-10-14): steady for 30 minutes (82 → 87 MB), then a reconnect left the old session running on the desktop, at up to 177 MB and double CPU. **Fixed:** the phone closes a stopped stream on the wire, and the desktop ends a session unheard from for 30 s. Re-run: a monitor switch leaves one live session.
- [ ] An hour with one session from start to end, to confirm the memory levels off.
- [ ] The computer sleeping and waking (the user's to run), and the phone switching between Wi-Fi and mobile data (needs a SIM).
- [ ] The same on the Linux host.
- [ ] A file transfer cut by a drop (today it fails and is sent again whole).

## 47. Check connection (T1)

On the A71 against this Windows box, 2026-10-14:
- [x] From Home, with the server running, every check passes: Wi-Fi, same network, answers, the security key matches (now with the handshake signature checked), the video port answers (the UDP probe gets a real Version Negotiation reply). No battery warning without a background feature on.
- [x] With the server stopped, the computer "doesn't answer" (Windows' firewall turns a stopped server into a timeout, not a refusal), and the key and video port say "not checked".
- [x] From the session's error card, "Check connection" opens the same check.
- [ ] A changed security key (pin a wrong one), and "Scan code again" from the finding: the computer and its key removed, the scanner opened, and the new code trusted.
- [ ] A VPN that captures home traffic, and Tailscale off for a tailnet address.
- [ ] Guest Wi-Fi with AP isolation, and another subnet.
- [ ] A UDP-only block (the computer's firewall dropping the stream port).
- [ ] Android 17's local network permission denied.
- [ ] Copy report on a device: no address or id in the text.
- [ ] The Linux host (a stopped server there should be "nothing is listening").

## 48. Staying connected in the background (T2)

On the A71, 2026-10-14:
- [x] With phone notifications on and battery optimization on, Home shows "Android may stop Pocketdesk in the background", with "This phone's own setting" (Samsung's dontkillmyapp.com page).
- [x] "Allow" opens Android's battery optimization page, and Back returns to the app.
- [x] Settings, Background shows the status; "Remind me on Home" off hides the card, and on brings it back.
- [ ] After allowing it in the battery settings, the card is gone on return.
- [ ] Battery use set to Restricted: the card says so, and "Allow" opens the app's page.
- [ ] With the app allowed, phone notifications and texts keep arriving overnight with the screen off (the reason for all this).
- [ ] Other makers (Xiaomi, OPPO): the guide link and their own battery managers.

## 49. Protocol audit (what Pocketdesk sends, and to whom)

`docs/protocol.md` is the map. On the A71 and this box, 2026-10-14:
- [x] A WAN dial (stream port closed to force it) resolves n0's relay through the network's DNS: no `android context was not initialized` panic and no `failed to read system DNS configuration, using fallback` in logcat (the build before logged both).

Owed:
- [ ] Linux: after `pocketdesk start`, `stat -c %a ~/.local/state/pocketdesk` is `700`, and the log says "Made … owner-only" once; a `POCKETDESK_STATE_DIR` the user cannot chmod logs a warning and the server still starts.
- [ ] Wake through a relay computer with Power off for the phone: the phone says the relay did not answer and asks about Power, not "Wake sent". With a target off the relay's networks: "The relay computer refused: not on this computer's network".
- [ ] The desktop app's Devices tab reads "Lock, sleep, privacy mode and waking others".
- [ ] Android backup: `adb shell bmgr backupnow dev.pocketdesk.android` backs up nothing, and a device-to-device move (Smart Switch) leaves the app unpaired on the new phone.
- [ ] Windows: after `pocketdesk install`, the firewall has TCP 1716 and UDP 4716 rules only, and a phone on a fresh machine still lists the computer by mDNS (no UDP 5353 rule is made).
- [ ] A Wi-Fi with Tailscale up: a phone browsing mDNS sees the LAN address and no 100.x or fd7a: address.
- [ ] Transfers both ways still end "saved" over many runs (the clean close after the status byte), and a desktop share to a full disk fails at once instead of after 10 s.

## 50. Resumable transfers (T6a)

On the A71 against Windows, 2026-10-15:
- [x] An offer of 150 MB with the phone's Wi-Fi off 2 s in: the retry continued ("The phone had 16580608 of 157286400 bytes"), one file in Downloads/Pocketdesk, MD5 matches.
- [x] A share of 120 MB the same way: the retry replaced the stale attempt and continued from 12.6 MB, MD5 matches, nothing left in `<state>/partials`.

Owed:
- [ ] A file from the remote browser broken and continued.
- [ ] "Retry failed files" off: a broken file fails at once, and no part is kept.
- [ ] "Unfinished files: not kept" on the desktop: a retry starts over.
- [ ] A file that changes between attempts starts over (the prefix hash).
- [ ] The app killed mid-download, then the offer fetched again within its 10 minutes.
- [ ] Android 8–9 (the legacy download file), and the Linux host.

## 51. Folders (T6b)

On the A71 against Windows, 2026-10-15:
- [x] Phone → computer: "Send a folder to this computer", a folder of three files in three levels: the same tree in Downloads, MD5s matching, `.hidden` left out.
- [x] Computer → phone: a folder queued for the phone during a session: the same tree in Download/Pocketdesk, MD5s matching, the hidden file left out, the Files tab's row "sent".
- [x] Sending the same folder again lands in "Name (1)".
- [x] After the review fixes: a request as the desktop app writes it (hyphenated id), 20 files in three levels: every file in its folder on the phone, MD5s matching; the dot-file and a zero-width-space name left out, and the Files tab's row says "1 left out".
- [x] The same folder from the phone: one received row for the folder (20 files, 8 MB), MD5s matching.

Owed:
- [ ] A folder from the phone shows one "folder arriving" toast, not one per file.
- [ ] The phone shows "Not all of … was sent" when it leaves names out, and "Stopped sending …" when the computer goes away mid-folder.
- [ ] A folder broken by Wi-Fi mid-way, continued.
- [ ] A folder over the limits (10 000 files, 32 levels) says what was left out.
- [ ] The desktop app's "Choose folder…" and dropping a folder on the window.
- [ ] Names Windows refuses on the phone (`a:b`) left out and counted.
- [ ] Android 8–9, and the Linux host.

## 52. Download a folder from the remote browser

On the A71 against Windows, 2026-10-15 (`2026-10-15-download-folder-design.md`):
- [x] Home shows no "Download this folder"; a folder below it does.
- [x] An 8-file folder in three levels, with a dot-file and a file Windows marks hidden: all 8 at their places in Download/Pocketdesk, MD5s matching, the hidden two left out; the desktop's Files tab row "sent".

Owed:
- [ ] The toast: "Asked the computer for …", and a refusal's reason (Files off, a third folder while two are on the way).
- [ ] Closing the sheet before the answer, and the toast still showing.
- [ ] A folder of thousands of files: the answer arrives, and other queries keep working while the offers come.
- [ ] The Linux host.

## 53. The local API (T7)

On this box with the A71, 2026-10-15 (`docs/local-api.md`):
- [x] `status --json` with the server stopped (running false), and with the phone in a LAN session.
- [x] `devices --json`: three paired phones, the connected one marked, Input off as set.
- [x] `send <prefix> <file> --wait 120 --json`: "sent", exit 0, MD5 matching on the phone; with no phone connected, refused at once; with no server, exit 3; a usage error as JSON, exit 2.
- Seen on the way: the desktop's "sent" can come a moment before the phone's download is published (the MediaStore entry is still pending for a few milliseconds). Not new to this change.

Owed:
- [ ] The Linux host: `status`, `devices`, `send --wait`.
- [ ] `send` of a folder with `--wait`, and a phone that leaves mid-send (exit 1).
- [ ] A broken config.toml with `--json` (exit 1, JSON).

## 54. Automation apps on the phone (T7b)

On the A71 against Windows, 2026-10-15 (`docs/phone-automation.md`):
- [x] A text share naming a computer that is not saved reached no computer: no clipboard packet at the desktop, its clipboard unchanged.

Owed:
- [ ] Tasker itself: the clipboard recipe, a file with "Grant read permission", Connect and Wake from a task.
- [ ] A share naming one of two saved computers goes to that one (the A71 has one saved).
- [ ] From the background, with and without "Display over other apps".

Seen on the way: sessions ending every ~10 s, and slow transfers. First taken for this computer's display being asleep; it was the link (§55).

## 55. A slow or lossy link (fix/slow-link-watchdog)

On the A71 against Windows, 2026-10-15, over Wi-Fi that lost about a third of its packets (strong signal, but large packets dropping; ~36 KB/s on the wire, pings 40-60 ms):
- [x] Before: every session ended after 10-15 s ("video feed silent"). The desktop wrote ~2.5 MB of frames per session into the connection while ~0.5 MB reached the wire; the phone waited on one starved frame stream. Downloads during that churn ran at ~26 KB/s, and one file was published ~16 s after the desktop said it was saved (the same starved link).
- [x] After (the phone counts video bytes, and the desktop keeps at most 8 frames on the wire): one session held 135 s until ended by hand, at 36 % loss.
- [x] After the review (the window sized to a round trip, any delivery making room): a session held 90 s with no drop on the same link, pings averaging 77 ms.
- [x] Loopback (`server/tests/session_record.rs`, a real capture and encode): with and without the bound, two runs each, frames encoded (168-196), rate (1.6-2.4 Mbit/s) and e2e p50 (16-21 ms) alike. Its e2e p99 of ~3 s is the same without the bound: older than this change, from the session's start.

Owed:
- [ ] A healthy LAN: frame rate and e2e unchanged (the in-flight bound must not throttle it).
- [ ] WAN, direct and relayed: frame rate at a 150-300 ms round trip.
- [ ] The picture's delay on the lossy link (how stale it runs), and a keyframe after a long stall.

## 56. Links both ways (T8a)

On the A71 against Windows, 2026-10-15, LAN, input off for the phone:
- [x] `pocketdesk link <phone> https://example.org/...` printed `queued` (exit 0); the phone showed "A link from <address>" on its own channel, "Links from the computer"; tapping it opened the link in the phone's browser.
- [x] `pocketdesk link` with `javascript:alert(1)`: exit 2, "not a web link".
- [x] "Open on computer" with "A page https://example.org/..." opened the link in this computer's browser ("Opening a link from …" in the log); a share with only a `file:` link sent nothing.
- [x] With the phone's Links permission off: a share from the phone was not opened, and a queued link was not sent ("links are off for this phone"). (Since then `pocketdesk link` and the app refuse such a link up front.)
- [x] After the review fixes: "Read (https://example.org/…)." opened on the computer (from its own thread); "See http://localhost:8080/admin." was refused on the phone, and `pocketdesk link <phone> http://127.1/` with "a link to the machine's own address".

Owed:
- [ ] Chrome's and Firefox's share sheets ("Open on computer" listed, the title and link they share).
- [ ] The desktop app's Send link, and its refusal for a phone with Links off.
- [x] `open_links = false`: links shown as notifications, none opened (2026-10-15, six links from "Open on computer").
- [x] Six links within a minute: the sixth refused ("more than 5 a minute").
- [ ] The phone's "Links from the computer" off: a link dropped.
- [x] Linux (2026-10-15, the release dry run's build on an Azure VM under a headless Sway, the A71 over the internet): "Open on computer" reached `xdg-open` with the link (the sentence's full stop left out), and no zombie process was left; `pocketdesk link` from the Linux CLI gave the phone its notification.

## 57. The phone's battery on the computer (T8b)

On the A71 against Windows, 2026-10-15, LAN, input off for the phone:
- [x] At the session's start `phone_status.json` held the phone's battery, 100 % and charging over USB, matching `dumpsys battery`.
- [x] `dumpsys battery unplug` and `set level 12`: one notification on the computer, "Phone <id>: battery low (12 %)"; `set level 10` gave none more; `pocketdesk devices --json` showed 10 %, not charging.

Owed:
- [ ] The Devices tab's text, and its age after the session ends.
- [x] After the review fixes: the report written at the session's start, and no control connection in the next 70 s beyond the two the session opened (the old loop dialed one per 15 s without a link).
- [ ] Catching up after a Wi-Fi drop.
- [ ] Unpairing the phone: its report gone from the file within 5 s.
- [ ] The phone's "Share battery with the computer" off: no report; the desktop's "Phones' batteries" off: no notification.
- [x] Linux: a faked 11 % was recorded and warned about in the log, but **no notification was shown**: every Linux notification failed (`notify-send --expire` is not an option). Fixed (`--expire-time`); owed: see it shown on Linux.
- [ ] A real discharge to 15 %, and Android's own low flag.

## 58. The computer's media from the phone (T8c)

On the A71 against Windows, 2026-10-15, LAN, input off for the phone. A silent track played through WinRT's `MediaPlayer` from a PowerShell process stood in for a player (it reports to the system media controls with no title):
- [x] The Sound sheet's "Now playing on the computer" showed "powershell" with Pause, and the volume and output list beside it read correctly (no reply taken by another call).
- [x] Pause on the phone paused it (the computer's session read `is_playing: false`) and the card offered Play; Play resumed it.
- [x] With the phone's Audio permission off, the card said "Audio is turned off for this phone on the computer".

Owed:
- [ ] A real player with a title: a browser video, Spotify; Next and Previous.
- [ ] Nothing playing: "Nothing is playing".
- [ ] The card cleared to the error alone with Audio off (fixed after the run).
- [x] Linux: with `mpv` and its MPRIS plugin playing, the card showed "VM Test Tone" by "Pocketdesk · mpv", and Pause on the phone paused it (`playerctl status`: Paused).
- [ ] Linux: `playerctl` missing.

## 59. The phone's media from the computer (T8d)

On the A71 against Windows, 2026-10-15, LAN, input off for the phone. The A71 (LineageOS) has no music player, so a throwaway instrumented test held a "playing" media session in Pocketdesk's own process; notification access was granted for the run and revoked after, and "My media on the computer" set and restored:
- [x] The phone's report reached the computer: `phone_media.json` held "Probe Song" by "Probe Artist", playing, all three buttons.
- [x] A next press and a play/pause press, written as the desktop app writes them, reached the session ("press: next", "press: pause"), and the report followed to paused.

Owed:
- [ ] The Sessions tab itself, and its buttons.
- [ ] A real player (Spotify, a podcast app): its title, and the app's name (Android 11+ package visibility may give the package name).
- [x] After the review fixes (2026-10-15, the probe again): the song gone within seconds when Media was turned off and when the session ended; reported again 9 s after a server restart mid-session.
- [x] Found on that run: with Media turned back on the song did not return until it changed; fixed (the phone says it again each minute). Owed: re-check that it returns within a minute.
- [ ] The phone's setting turned off clearing the song; a press sent while the app was in the background not acting later.
- [ ] Turning the setting on opening the notification access page.

## 60. Pinch-zoom in trackpad mode (T9a)

adb cannot make multi-finger gestures; the engine has 21 JVM tests. Owed, by hand on the A71:
- [ ] In trackpad mode, two fingers spreading zoom the view, pinching zooms out, back to 1x.
- [ ] Two fingers moving together still scroll the desktop, and a scroll that drifts apart does not start a zoom.
- [ ] While pinching, moving the fingers pans the view; one finger lifting and landing again does not jump the zoom.
- [ ] A two-finger tap is still a right click, a three-finger tap a middle click.
- [ ] Zoomed in, the cursor's travel matches the finger's on-screen distance, and scrolling is not slowed by the zoom.

## 61. Portrait sessions (T9b)

On the A71 against Windows, 2026-10-15, portrait, input off for the phone:
- [x] The session split: the 21:9 desktop in the top quarter, the trackpad dock and shortcut bar below (before, a letterboxed strip and black).
- [x] After the review fixes: the chrome faded after a few seconds, leaving the whole desktop visible.

Owed:
- [x] The on-screen keyboard opening in portrait: the layout stayed split, the desktop kept its full top pane, and the key bar and keyboard sat over the dock (2026-10-15).
- [ ] Turning the phone: on its side the A71 (860 dp wide) splits side by side, as before.
- [ ] A 16:9 desktop's pane, a monitor switch between shapes, picture-in-picture, and Pane: Single and Dual.

## 62. A mouse on the phone (T9c)

No mouse was at hand; with none connected, a session on the A71 started and Back ended it as before. Owed, with a USB OTG or Bluetooth mouse:
- [ ] Motion, all five buttons and the wheel (both ways) reach the desktop, and a fast mouse keeps its travel.
- [ ] The phone's pointer is hidden during the session and back over Home after Exit.
- [ ] Opening and closing the in-session keyboard: the mouse keeps working.
- [ ] The mouse's Back button goes to the desktop and does not end the session.
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
- [ ] A phone with no iroh key registered (paired, never streamed on the LAN) is refused, with one log line, and works again after one session at home.
- [ ] Unpairing on the desktop ends the internet link's packets at once.
- [ ] A file offer to a phone on the internet link fails as "not reachable", not stuck.
- [ ] The phone's battery over an hour with the link up in the background (iroh's 5 s heartbeat).
- [ ] A session riding a relay (no direct path) with the control link up at the same time. Founded: two endpoints with one key cannot share a relay (`server/tests/one_key_two_endpoints.rs`); since 2b both ride one endpoint. Owed on the VM, relayed (checklist §85).

## 65. The clipboard outside a session (T10b)

On the A71 against this box, 2026-10-16:
- [x] With "Computer's clipboard in the background" on and notification access, and the app in the background, text copied on the computer reached the phone's clipboard within one tick.
- [x] The "Send clipboard" tile sent the phone's clipboard text to the computer's clipboard.

Owed:
- [ ] Away from home (the internet link): both directions.
- [ ] Desktop "Clipboard to phones" off: nothing pushed; a session still gets the clipboard on request.
- [ ] A phone with Clipboard off on the desktop gets no pushes.
- [x] Linux (the Azure VM, headless Sway, this branch's release build): text copied with `wl-copy` reached the A71 in the background. The VM had no `wl-clipboard` at first; without it the computer's clipboard is never read (`wl-paste` is a runtime need on Wayland).
- [ ] Linux with an image on the clipboard, and a copy offering `x-kde-passwordManagerHint` (KeePassXC).
- [ ] Android 10–12: the tile's window reads the clipboard.

## 66. Answering the phone's notifications from the computer (T10c)

On the A71 against this box, 2026-10-16, with a throwaway chat app that posts a notification with Reply and "Mark as read" (not committed):
- [x] The computer showed it in full and kept it with its button and its reply.
- [x] A reply written as the desktop app writes it reached the app through its `RemoteInput`; "Mark as read" ran; a button it did not offer was refused by the server.
- [x] Found and fixed on the way: nearly every notification arrived as "New notification" (an app's default private was always hidden), and answers were dropped while the phone had no live link.
- [x] After the review fixes: a button that requires unlocking (`Approve`) was not offered, and a press of it was refused; reply and "Mark as read" ran again.
- [ ] "Hide sensitive content" on the lock screen turned on: the probe's notification arrives as "New notification" and offers nothing.

Owed:
- [ ] The desktop app's Notifications tab itself (its buttons and reply box), and a real chat app (WhatsApp, Signal, Messages).
- [ ] The switch off: nothing offered, an answer not run.
- [ ] From away from home (the internet link).
- [ ] An action that opens an activity on Android 14+ (background launches may be blocked).

## 67. Files from the computer without a session (T10d)

On the A71 against this box, 2026-10-16:
- [x] With "Files from the computer in the background" on and notification access, the app in the background and no session, the mirror listed the phone as linked with files, and `pocketdesk send … --wait` ended "saved on the phone", MD5s matching. Android 16 let the listener start the download at once.

Owed:
- [ ] A phone that refuses background starts: the "The computer sends a file" notification, and Save downloading it.
- [ ] A folder sent this way.
- [ ] The desktop app's Files tab listing a linked phone with no session.
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
- [ ] Send To in Explorer itself, with several files and a folder.
- [ ] Nautilus, Nemo and Dolphin on Linux.
- [ ] The desktop app's Settings switch, and `install` / `uninstall` adding and removing the entry.
- [ ] No phone connected: the notification says so.

## 70. The phone's folders on the computer (T10g)

On the A71 against this box, 2026-10-16:
- [x] Settings "Folders the computer may open" → Add folder → Android's picker (Download/Pocketdesk) → Allow: listed with Remove; Remove emptied it.
- [x] With the app in the background, a list request (written as the Phone tab writes it) showed the folder and a sub-folder; a fetch saved the file in Downloads, MD5s matching.
- [x] Again after the review fixes: a hidden file neither listed nor sent when named; a fetch of a file deleted on the phone answered "gone.bin is not on the phone any more" with its folder; a list of a removed root answered "That folder is no longer shared"; a fetch with MD5s matching.

Owed:
- [ ] The desktop app's Phone tab itself (roots, folders, Up, Download).
- [ ] A folder of more than 2000 entries; a file renamed between list and fetch.
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
- [ ] A screenshot with the side keys, and a JPEG screenshot (One UI's other format).
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
- [ ] The owner's approval prompt ("Ask before each session") before ready.
- [ ] Over the internet road, and a phone driving while another has a picture session (driving rules).

## 74. The phone's battery without a session

On the A71 against this box, 2026-10-16, with "Computer's clipboard in the background" keeping a link and no session:
- [x] `dumpsys battery set level 12` (unplugged): `phone_status.json` said 12, and the computer warned "battery low (12 %)".
- [x] With no session listed at all, level 9 reached `phone_status.json`.
- [x] After the review fix (the live link only): with no session listed, 13 % reached `phone_status.json` and was warned about.

Owed:
- [ ] With only folders, answers or files keeping the link; with none on, nothing sent and no connection made; the link dropping just before a report (nothing dialed).
- [ ] Over the internet road.

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
- [ ] The same computer saved under its home address: the network finding, with "This phone is sharing its connection" beside it.
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
- [ ] The notification's Stop (the A71 showed only the redacted notification in the shade while locked), and Sessions' "Show screen" after the window was closed.
- [ ] "Share one app" instead of the entire screen; a phone call or a secure screen (banking apps black out); a long cast's heat and battery.
- [ ] The Linux desktop app (Wayland and X11), and a computer without `ffmpeg` (the window says so).

## 78. Events and hooks (ecosystem 1a)

A71 → this Windows box, LAN, debug server, 2026-10-16, with `pocketdesk events --json` following the log and a `session_started` hook copying its stdin to a file:
- [x] `session_started` (mode `screen`, transport `lan`) and `phone_linked` as the app opened a session; `phone_unlinked` when the app was force-stopped; `session_ended` once the server noticed the silence (about 30 s later, as designed).
- [x] The hook ran, with the event's JSON on its stdin.
- [x] A battery faked at 13 % and unplugged: `phone_battery` (13, not charging) and `phone_battery_low`.
- [x] Nothing about content at the default detail.
- [x] After the review fixes: a hook starting with a quoted program (`"C:\Windows\System32\cmd.exe" /C more > "…"`) ran through the queue with the event on stdin, exit code 0.

Owed:
- [ ] `notification`, `text_received`, `file_received` and `link_received` from a phone, at both detail levels.
- [ ] The desktop Settings hooks box saving a hook the server then runs; "Event log" off writing nothing.
- [ ] A hook killed at 30 s, and the 33rd waiting hook skipped, on a real server.
- [ ] Linux: `sh -c` hooks, and the log's mode (0600).

## 79. The Activity Center (ecosystem 1b)

A71 → this Windows box, LAN, debug server, 2026-10-16:
- [x] A screen session, then "Remote" with the phone's Input off: the log held `session_started`, `phone_linked`, `session_ended`, `session_refused`, the `hook_ran` of a `session_refused` hook (exit 0) and `phone_unlinked`.

Owed:
- [ ] The Activity tab itself on screen (its rows, filters, Show in folder): a click posted to the window did not reach egui, and real input into this box's desktop is not used for tests.
- [ ] Pairing and unpairing a phone showing as rows; a folder's Show in folder after it finished arriving.

## 80. Named tasks (ecosystem 1c)

A71 → this Windows box, LAN, debug server, 2026-10-16, three tasks in config.toml:
- [x] Quick Settings, "Tasks on this computer": the three names, "Asked" marked as asking first; no command line on the phone.
- [x] "Say hello": "done (exit 0)" with its output on the phone; `task_started` / `task_finished` in the event log.
- [x] "Asked": "waiting for the computer's owner to allow it…", held in `task_requests.json`; allowed through `task_answers/` as the desktop app writes it; then "done (exit 0)" with its output.
- [x] "Slow" (`deadline_secs = 3`): "stopped at its deadline"; `task_finished` with `timed_out`.
- [x] After the review fixes: "Say hello" again over the live link, and "Slow" stopped at its deadline with what it wrote before it (its first lines) kept and shown on the phone.

Owed:
- [ ] The desktop app's Allow / Deny prompt itself (clicked, not written), Deny on the phone, and the Settings task editor.
- [ ] "Start my tasks" off: the phone gives up with "no answer" after 20 s.
- [ ] Linux (`sh -c`), a long output's end on the phone, and two phones at once.

## 81. Send from the browser (ecosystem 1d)

This Windows box and the A71, debug builds, 2026-10-16:
- [x] `pocketdesk browser-host --install`: registered with Chrome, Chromium, Edge, Brave, Vivaldi and Firefox (HKCU keys naming the manifests; the Chromium manifest allows only `chrome-extension://abojofjjnpcmejhdonndlegfaolfkcif/`); `--remove` took the keys and files away.
- [x] The host driven as a browser drives it (`pocketdesk.exe chrome-extension://…/` and framed JSON), with a session on the A71: a link answered "Sent to your phone" and showed in the phone's notification; an image (an SVG behind a redirect) arrived in `Download/Pocketdesk`, its MD5 matching the site's; `localhost` and an unknown request refused.
- [x] Found and fixed: images kept in the state folder were never sent (the server does not send its own folder's files); they wait in a folder of the user's own now.
- [x] After the review fixes, again through the host with a session on the A71: a PNG from Wikimedia arrived in `Download/Pocketdesk`, MD5 matching (it had answered 403 until the download named itself with a User-Agent); an SVG refused as not a picture Pocketdesk sends; a redirect to `127.0.0.1` refused with why; a link sent.

Owed:
- [ ] The extension itself loaded in Brave or Chrome and in Firefox (unpacked), and its menus and notifications.
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
- [ ] The remote file browser's download and a folder fetch over the internet road.
- [ ] 2e: a screenshot copied on the computer lands on the phone's clipboard over the internet link (and phone → computer, when an app on the phone can copy an image).
- [ ] A real WAN (the VM): 500 MB resumed with TCP 1716 unreachable.
- [ ] The too-old message against a computer from before 2c.
- [ ] Both Settings switches off: a send refused at once, with the reason.

## 86. Two computers: pairing, the link, the clipboard and files (2f–2h)

Two real servers on this box passed `server/tests/two_computers.rs` (2026-10-17): a forged certificate and a wrong PIN paired nothing, the code paired, B listed A as a computer with the narrow profile and held A's iroh key, the link came up over iroh, a copy crossed each way (stand-in clipboards), a 3 MB file each way byte for byte, Forget dropped the link. Owed on two real computers (Windows, the R6a throwaway instance for A so the installed server is untouched, and the Azure VM for B):

- [ ] Pairing from the Pair tab's copied link pasted in Settings → Other computers; the outcome shown; B's Devices tab lists A with Clipboard, Files and Links only, and ask first.
- [ ] The link survives a restart of either computer, and comes back after the network drops ("linked" again within a minute).
- [ ] The clipboard both ways in under 2 s with "Clipboard with my computers" on on both; off on one, nothing crosses either way; a password manager's copy stays.
- [ ] A 500 MB file each way from the Files tab, broken (the network off for 15 s) and continued, MD5 matching; `pocketdesk send <name> <file> --wait`.
- [ ] A folder from B to A lands under one new folder; a folder from A to B is refused with its reason.
- [ ] Unpair on A (Settings → Other computers): the link goes, and B's Devices page still lists A until unpaired there; after that, a pairing again works.

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
- [ ] Cancel after the device's Accept unpairs it on the phone; a second request from the phone while one waits shows nothing new.
- [x] 3c, battery: the A71's KDE Connect sent it as the link came up (100 %, charging), kept in `phone_status.json`.
- [x] 3c, the clipboard from here (the clipboard stand-in file): pasted on the phone. Found: the watcher rested while only a KDE device was connected, so nothing went; fixed.
- [x] 3c, the clipboard from the phone ("Send clipboard" in KDE Connect): the computer's set.
- [x] 3c, files: with Files off, a 3 MB file from KDE Connect's "Send files" was refused and nothing written; with Files on (the permissions request), it arrived in Downloads (the stand-in folder), MD5 matching, marked as downloaded (ZoneId=3), listed in `received.json`.
- [x] After the 3c review: Ring (now `kdeconnect.findmyphone.request`) rang the A71, whose find-my-phone plugin turned on for this computer; stopped with FOUND IT.
- [x] Unpaired here while away (the pins edited with the server stopped), the A71 came back thinking itself paired and was told it is not: KDE Connect moved this computer to Available devices.
- [x] Found and fixed: the battery the A71 sent as it accepted was dropped while the owner confirmed; now held and kept at pairing (none before Confirm, 100 % right after).
- [x] Files again after the review's intake changes: Files turned on with the one-switch request, a 3 MB file kept as `pd-kde-3c (1).bin` beside the first, MD5 matching, nothing left in `kdeconnect_parts/`.
- [ ] 3c, a text or link share refused with its notice, the low-battery warning from a KDE device, and the switches, battery and Ring on screen.
- [ ] A device that broadcasts first (this computer dialing it): not built (the receive half waits).
- [ ] Linux, with `kdeconnectd` holding UDP 1716.

## 88. The desktop app's shell (U1)

On this box, the app's own window captured (no input into it):

- [x] The navigation down the left in its groups; Settings at 720×480 scrolling, its heading and purpose shown.
- [x] File names in Hindi, Korean, Thai, Arabic, Chinese and Tamil drawn (they were boxes).
- [x] Windows UI Automation reads the navigation, the group titles, the page's heading and its text.
- [x] A second run opened on the page the first had shown.
- [ ] Settings' Save bar after an edit, a notice clearing after 6 s, "Copied.", and the two settings' switches while their change runs (they need clicks).
- [ ] Narrator reading the app; the Linux build (Orca, the Noto fonts through fc-list).
- [ ] A line mixing Arabic or Hebrew with Latin (egui has no bidirectional layout), read by someone who reads it.

## 89. The phone's navigation and feedback (U2)

On the A71, against this box and a throwaway server reached through `adb reverse` (phone-only taps; the throwaway's Input off for the phone):

- [x] Back from Settings, Add computer and the scanner to Home; from the pairing confirmation, cancelling it; from the remote, ending it.
- [x] A tapped computer that does not answer says "Checking whether it is awake…", and the internet try ends in about 16 s (31 s before).
- [x] A remote that could not connect says why in plain words.
- [x] Lock's question from the session's sheet and from the notification's "Lock desktop"; Cancel both times, and the server logged no privacy request.
- [x] The pairing sheet: the PIN field, Pair, "Show a PIN on the computer", Cancel; the eye says Show PIN and Hide PIN; paired with a PIN.
- [x] A wake's result as a snackbar that goes by itself; the scanner's Cancel and its hint.
- [ ] Lock confirmed, and refused with Power off (the server's answer is unit-tested): never against this box, so on the VM or a spare computer.
- [ ] Ring's snackbar (it rings the computer), and a shortcut key's failure message on a dropped link.
- [ ] TalkBack reading the snackbars and the checking card.

## 90. Names on both sides (U3)

On the A71, against a throwaway server named "Test Desk" (`COMPUTERNAME`) reached through `adb reverse`, and the desktop app's own window:

- [x] A computer's card shows its name after a connection, with its address and ports under it; Rename to "Lab box", and clearing it, back to its own.
- [x] The computer keeps the phone's name at pairing (Android's device name), and a name set in the phone's Settings, changed mid-run too (the link reopens), and the device name again once cleared.
- [x] `pocketdesk devices --json` gives `name`; the desktop app's Devices page shows the name over the id.
- [ ] Rename on the desktop's Devices page, and the approval prompt's "(calls itself)" (they need clicks).
- [ ] A computer's name from a scanned pairing code, the session notification and launcher shortcuts by name, and "Lock <name>?".
- [ ] The Linux host (its kernel host name), and a phone whose Android name holds a person's name.

## 91. Ring the phone from the computer (N1)

On the A71, against a throwaway server reached through `adb reverse`; each ring kept to seconds:

- [x] "Ring from the computer" on, the app in the background and the screen off: `pocketdesk ring` rang it on the alarm stream within about 4 s.
- [x] It went by itself after 30 s; Stop from the shade silenced the next one at 8 s; the alarm volume was back as it was.
- [x] With the switch off, the computer sent the ring and the phone dropped it ("not rung"); the switch shows its change at once (it had not, with eight others).
- [x] After the review: with the remote open (and the background link up), a ring sounded through the remote's own watcher with its Silence dialog, the alarm stream at 7 of 7 while it rang and back at 6 after Silence, the notification gone.
- [ ] The desktop's Ring button on screen (it needs a click), Ring off in Devices refusing it, and once a minute from the button.
- [ ] Do Not Disturb with alarms allowed and not; a phone with a secure lock screen (the full-screen ring and Stop); a phone where the full-screen permission is not granted.
- [ ] The Linux host.

## 92. Phone → computer text in one tap (N2)

On the A71, against a throwaway server with a stand-in clipboard (`POCKETDESK_CLIPBOARD_FILE`):

- [x] Android lists "To computer" among the apps that take selected text, and its intent put the text on the computer's clipboard.
- [x] The connect shortcuts carry the share category (long-lived); a share through a computer's shortcut id reached that computer, and one through an unknown id went nowhere.
- [x] After the review: the connect shortcuts' ids carry the token; a share through the real id reached the computer, one through the old `connect:<address>` form (as another app would forge it) went nowhere, and the selection menu's text still arrived.
- [ ] "To computer" in a real app's selection menu, on screen; the Sharesheet's Direct Share row showing the computers (adb opens only the app chooser).
- [ ] A copy marked sensitive (a password manager's) asking first through the tile.

## 93. The phone's Home card, Add form, Settings and session menu (U4)

On the A71, against a throwaway server (`adb reverse`, input off for the phone):

- [x] Home's card: the name with its status line, Connect, Remote and Show screen on one row, the ⋮ menu with Remove last in red.
- [x] The Add form: the top bar's arrow goes back, the example address shows, the URI keyboard with Go, and the label toggles the switch.
- [x] Settings scrolled through in its groups; an ⓘ shows a summary in full without toggling its switch; Ring from the computer still off.
- [x] A session: no dot while connecting, "Good connection" once streaming over the internet road; a refused session shows none.
- [x] The session menu: the groups, segmented choices (Trackpad chosen, the chip followed), no chevron on Ring, Lock, Blackout and picture-in-picture.
- [ ] TalkBack on a card ("Connect"), a Settings row (one stop) and a heading.
- [ ] "Show connection details" on, during a session: the numbers and the computer's battery.
- [ ] A row's "Allow notification access" with a feature on and access off.
- [x] Spanish and Tamil (the app's own language set by `cmd locale`): the card's buttons fit (Tamil wraps Show screen to a second row), and the four quality choices fit after the review.
- [ ] Landscape: the bottom chrome with 48 dp chips.

## 94. The desktop's Settings and words (U5)

On this box through the app's own window (screenshots only), with a throwaway config and state:

- [x] Settings in sections, Connection and Phones open, each heading with what it holds; rows with their notes under them.
- [x] Devices: "Unpair…" and a battery's age as "9 min ago"; Files with nothing to send to says which phones and computers can take a file.
- [ ] Opening and closing sections; a change shown as applying now or after a restart in the save bar, and the restart notice naming it.
- [ ] A narrow window wrapping every row; a section with a problem held open.
- [ ] Unpair's question for a phone and for a computer, and Cancel.
- [ ] Pair a phone and Overview buttons on the empty pages.

## 95. The phone app in Spanish and Tamil (U6)

On the A71 with the app's language set by `cmd locale set-app-locales` (reset after), against a throwaway server:

- [x] Tamil: Home's bottom row wraps to two lines; a session's chip says நேரடித் தொடுதல் as its menu does; the connection details in Tamil, clear of Exit after the fix.
- [x] Spanish: the details (retardo, perdidos, equipo, "tasas en 1,5 s"), the chip "Toque directo", and the shortcut bar's Ctrl+Alt+Supr and Impr Pant.
- [ ] The keyboard bar's keys and the remote's presenter keys in Spanish.
- [ ] Every screen in Spanish and Tamil for text that runs out of its box.

## Recording results

Update `AGENTS.md` Current Status (mark verified items / list failures) and
tick plan.md's Tier checkboxes with real device results — items marked
"device behavior unverified" are not done until this file says they are.
