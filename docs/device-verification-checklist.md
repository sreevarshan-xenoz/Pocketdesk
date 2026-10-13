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

- [ ] Video appears within ~1 s; stats HUD shows fps/bitrate/rtt/e2e/drops, plus the caption naming what
      the rates and the link figure were measured over (§2d).
- [ ] FGS notification is present ("Streaming to <ip>"), survives home/swipe-away attempts; its Disconnect action ends the session.
- [ ] Screen-off for 1 min, wake → stream still live (wake lock + keepalive; plan #13).
- [ ] Drop phone far from AP / saturate link → frames stall, then recover; drops counter rises but no permanent freeze (gap-driven keyframe request, R2#5).
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

- [ ] Sanity: on-LAN idle desktop → HUD `e2e` should sit in the low tens of ms and track the
      `rtt` chip sensibly (e2e ≈ capture age + rtt/2); heavy desktop motion → `e2e` rises
      (encoder/queue age grows), never reads 0 once video is flowing.
- [ ] The old bug-shape check: `e2e` must NOT equal `rtt` while frames are under load (it used
      to be a literal RTT proxy).
- [ ] Cross-check (eyes, optional): point the phone camera at the desktop with an on-screen
      clock/stopwatch — the phone's rendered image lags the physical panel by roughly the HUD
      `e2e` figure (± the phone's own present latency, which the probe can't see).
- [ ] WAN session: `e2e` should ≈ rtt/2 higher than a LAN session on the same desktop motion
      (the network-leg term is RTT/2 by construction).

## 2d. Rate and link basis (roadmap 2056)

The HUD's fps/kbps are now diffs over the newest 3 s of received video and its `rtt` is the median of
the session's last ≤8 one-second polls; a caption under the numbers names both, and a figure whose basis
is still filling in is dimmed. What to confirm on a real phone:

- [ ] Right after connect: the caption starts at something like `rates over 0.5s · rtt unsampled` and
      reaches `rates over 3.0s · rtt median of 8` within ~8 s; fps/kbps are dim until the caption says
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
- [ ] Optional cross-check: while the caption reads `rates over 3.0s`, `pocketdesk sessions` for the same
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
- [ ] Wake row + wake status appear only for hosts with a saved WoL MAC (not exercised — none saved).
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
- [ ] With sound on, the HUD's fps and e2e match a session with it off, and taps on a still desktop feel immediate (the bridge lock fixes).
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
- [ ] A Wake shortcut appears for a computer with a Wake-on-LAN address, and sends the packet.
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
- [ ] **From outside:** the phone on mobile data, the relay on the desktop with port 3340 forwarded. A session connects through it.
- [ ] HTTPS mode on a cloud VM with a real name: the certificate is issued, and a session connects with `--allow` ids from `relay-ids`.
- [ ] A phone that is not paired (or an id not listed) is refused by the relay.

## Recording results

Update `AGENTS.md` Current Status (mark verified items / list failures) and
tick plan.md's Tier checkboxes with real device results — items marked
"device behavior unverified" are not done until this file says they are.
