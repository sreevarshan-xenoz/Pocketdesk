# Owed checks, by where they can run

Generated from `docs/device-verification-checklist.md` by `scripts/owed_checks.py` (C6); CI fails when it is stale. Each open item is placed by its own words and its section's, the first place that matches; a line can say `(where: <place>)` itself. Tick items in the checklist, then run the script again.

670 open items.

| Where | Items |
|---|---|
| Here: this Windows box and the A71, without touching the owner's power, lock or input | 383 |
| Power, lock or server stop on the owner's Windows box: never run here (the owner's rule) | 18 |
| Needs the owner's OK on this box: input reaching Windows, or its display settings | 49 |
| A phone with a SIM | 22 |
| A second phone | 7 |
| Away from home: another network, or through a router | 9 |
| A second computer | 7 |
| A Linux desktop (the Azure VM serves for some) | 88 |
| Other hardware: a foldable, tablet, mouse, game pad, older or newer Android | 24 |
| By hand or eye on this box: hearing, looking, the tray, drag and drop, a screen reader, a real app | 63 |

## Here: this Windows box and the A71, without touching the owner's power, lock or input

**0. Prerequisites**

- Phone: `adb install -r android/app/build/outputs/apk/debug/app-debug.apk` (build it first: `cargo ndk` for the bridge `.so`, then `assembleDebug`).
- Both on the same Wi-Fi LAN; phone can reach desktop port 1716 (TCP) and the QUIC streaming port (default 50000-range per config).
- Grant the app POST_NOTIFICATIONS when prompted (connect screen).

**1. Connect + pairing gate (Tier 1 #5, Tier-2 #11b)**

- Enter desktop IP + streaming port + control port → "Connect" opens a session.
- First connection: Pairing sheet auto-opens (pairing_required default true).
- Second connect to same host: no pairing prompt (persisted both sides).
- Manual flow: `pocketdesk pair` on desktop → type 6-digit PIN on phone → paired.
- Wrong/expired PIN → clear error, retry works.
- QUIC gate (v2 + video): before pairing completes, video must not flow — desktop log shows "Streaming session rejected: device not paired" (or "Unpaired device … on v2 QUIC") and the phone's status chip goes Down; after pairing, video works with no app restart beyond Retry.
- QUIC gate over WAN (iroh): unpaired dial is rejected the same way (desktop log + no frames).

**2. Session shell + video (Tier 1 #5, R2#2, R2#5)**

- Video appears within ~1 s; with "Show connection details" on, the HUD shows fps/kbps/rtt/delay/lost, plus the caption naming what the rates and the link figure were measured over (§2d).
- FGS notification is present ("Streaming to <ip>"), survives home/swipe-away attempts; its Disconnect action ends the session.
- Screen-off for 1 min, wake → stream still live (wake lock + keepalive; plan #13).
- Drop phone far from AP / saturate link → frames stall, then recover; the HUD's `lost` rises but no permanent freeze (gap-driven keyframe request, R2#5). "Drops" means **frames the phone never received** — the holes in the server's per-frame sequence numbers, counted where only the client can count them. So a saturated link must move it (the desktop's transport task trims its backlog before a stall clears), while a frame that arrives late and intact must not: the number can come back down when an out-of-order frame lands, and that is correct, not a bug. A decoder that cannot keep up is a different failure and shows as fps below the desktop's `src` rate, not here.
- R4 B1 idle pacing + freshness: keep the desktop static → `src` fps drops to ≤10 (damage-driven VFR); move a window → back to target fps within a frame or two, no tearing or upside-down frames (YInvert handling).
- R4 B1 multi-monitor: pick monitor N in the picker → log line "Screencopy capture: output at (x,y)" shows that monitor's wl_output origin and frames are its size.
- R4 C1 codec negotiation, default off: connect normally → server log shows "Client codec caps negotiated" with `codec=H.264` even on a HEVC-capable phone (allow_hevc defaults false); video unaffected.
- R4 C1 HEVC + window crop: pick a window during a HEVC session → encoder rebuild at window size keeps HEVC and the phone reconfigures on size change without codec change.
- R4 C2 mid-session stall, same-codec: force the supervisor on an H.264 session (kill its sidecar child) → video recovers on software x264 with NO phone toast (identical bitstream — the server log line is the only signal; that is expected).
- R4 C2 bottom rung exhausted: with software already live, keep killing encoders until the log shows "Encoder bottom rung exhausted; ending session" → stream goes Down cleanly (status chip + Retry works), never a silent infinite stall.
- R4 C2 crop stability: during a software-fallback session, pick/clear a window crop → encoder rebuild must NOT re-log the hardware fallback (sticky `encoder_preferred` — a rebuild must not re-probe the dead hardware).

**2c. In-process VAAPI encode (R4 C4) — desktop/box-observable, no device needed**

- R4 C4 host proof (no phone): `cargo test -p pocketdesk-core --lib encoder_vaapi` → `test_vaapi_encode_roundtrip` encodes 40 real frames through `h264_vaapi` on this box's Intel iHD node and asserts Annex-B packets + ≥1 keyframe (self-skips green where no VA node exists).

**2a. Desktop-side session visibility + kick (R4 D2)**

- Consent notice: first phone connect → desktop shows a "Pocketdesk: a device is streaming this desktop" notification with device id + LAN/WAN + peer (headless: the same line appears in the server log instead).

**2b. Compositor-true latency read (R4 E3)**

- Sanity: on-LAN idle desktop → HUD `delay` (the e2e estimate) should sit in the low tens of ms and track the `rtt` chip sensibly (e2e ≈ capture age + rtt/2); heavy desktop motion → `delay` rises (encoder/queue age grows), never reads 0 once video is flowing.
- The old bug-shape check: `delay` must NOT equal `rtt` while frames are under load (it used to be a literal RTT proxy).
- WAN session: `delay` should ≈ rtt/2 higher than a LAN session on the same desktop motion (the network-leg term is RTT/2 by construction).

**2d. Rate and link basis (roadmap 2056)**

- Right after connect: the caption starts at something like `rates over 0.5 s · rtt not measured yet` and reaches `rates over 3.0 s · rtt median of 8` within ~8 s; fps/kbps are dim until the caption says ~2.5 s or more, and `rtt` is dim until its first poll (~1 s). A number that arrives already bright means the basis is not being reported.
- Basis reset: end the session and reconnect to the same (or a different) desktop → the caption must come back up from a small span and `rtt median of 1`, never showing a figure or a sample count carried over from the session that just died.
- Portrait legibility: the caption is one line of 10 sp monospace; in portrait on a narrow phone it must not wrap into a single-character column the way the six metric chips did before they became a `FlowRow`, and must not be clipped by the HUD panel's padding.
- Optional cross-check: while the caption reads `rates over 3.0 s`, `pocketdesk sessions` for the same session should report a mean kbps within the same order of magnitude — the session record averages over the whole session, the HUD does not, and a large disagreement is a measurement bug in one of them.

**3. Input paths (R2#4, Tier 1 #2)**

- Mode toggle switches behavior live, no session restart.
- holdKey/repeat (e.g. hold an arrow key) behaves.
- R4 D1 view-only: tap "View-only: off" on the session bar → label flips ("View-only: on", amber), desktop log shows "View-only mode changed" (enabled=true), and EVERY remote input goes dead on the desktop (taps, shortcuts, workspace chips) while video, StatsHud, clipboard sync and monitor/window pickers keep working. Toggle back → input restored without reconnect. Re-arm check: while view-only is ON, trigger a stream Retry / monitor switch (fresh server pipeline starts interactive) — the phone must re-send the latch once the stream is Up again; verify input stays blocked across that rebuild.

**4. Zoom + display mapping (Tier 1 #6, R3#7)**

- Pinch-zoom in direct-touch mode pans/clamps correctly; taps still land on the visible desktop point while zoomed.
- Trackpad mode: two-finger still scrolls remotely (no zoom capture).

**5. Window + monitor streaming (R3#7, Tier-2 #10)**

- Moving the window mid-session: crop follows only after re-pick (known limitation — crop is static rect).
- "Whole desktop" clears the crop.
- R4 B2: close the streamed window mid-session → stream keeps running on full-output frames with software crop (warn log "did not stage"; no session drop).
- Monitor picker lists monitors via xcap; selecting index N streams that monitor's region; reconnect persists the choice per host.
- Mid-session monitor switch tears down + rebuilds cleanly (no zombie session — bridge "already active" path).

**6. Workspace HUD + battery (R3#8, Tier-2 #11a)**

- Workspace chips appear under StatsHud; open/close/focus a window on desktop → HUD updates within ~4 s.
- R4 E1: re-tap the highlighted window chip → crop clears back to whole-desktop (monitor) view; a *different* window chip tap switches the crop directly.
- Battery chip shows desktop percentage + charging state; matches `upower`.

**7. Clipboard + files (Tier 1 #4)**

- Copy on phone → desktop clipboard updated within ~4 s; copy on desktop → phone receives.
- No echo loop (toggle sync off/on, paste both ways repeatedly).
- History sheet: 20 entries survive app restart; pick restores local + pushes.
- Share text to the app from any Android app → lands in desktop clipboard, toast "Sent to clipboard."

**8. Notifications + find-my-device (R3#11c, Tier-2 #11)**

- Inline reply from the shade → desktop `replies.log` records it, text on desktop clipboard, confirmation popup; originating notification disappears.
- "Ring PC" → desktop plays sound 30 s (pw-play/paplay path; silent no-op is FAIL here since desktop has audio).
- Desktop pushes findmydevice ring → phone alarm loops at max volume 30 s (phone in alarm-volume >0; check DoNotDisturb doesn't block USAGE_ALARM).

**8a. Control-channel versioning (R4 D4)**

- Reply still works end-to-end after the `kdeconnect.notification-reply` → `kdeconnect.pocketdesk.notification_reply` rename (desktop-side check): reply from the phone shade → `replies.log` records it + confirmation popup (the server only matches the new type now, so an old client build's reply would silently no-op — both ends must be from this build).

**9. Privacy mode + lock (Tier-3 #15, R4 E4)**

- Without `input` group: clean error, no crash.

**10. Audio control (Tier-3 #16)**

- Audio sheet opens with volume %, mute state, sink list (matches `wpctl status`).
- Slider → desktop volume changes on release; mute switch works; selecting a sink routes default output (verify with `wpctl inspect` + real playback).

**10a. Phone mic share (R4 E2)**

- "Mic: off" tap → RECORD_AUDIO permission dialog; denying it toasts "Microphone permission denied" and the toggle stays off.
- Granting it → button reads "Mic: on" (amber) and the desktop gains a source: `pactl list short sources | grep pocketdesk_mic` (or pavucontrol → Recording).
- Speaking moves the pavucontrol input meter for "Pocketdesk Mic"; `pw-record --target pocketdesk_mic /tmp/mic.wav` captures audible speech (play it back).
- "Mic: on" tap → source disappears immediately; `pgrep -f pocketdesk_mic` empty (no leaked pw-loopback children after repeated toggling).
- Exit the session while mic is on → source disappears (release-on-dispose); force-killing the phone app → source gone within the QUIC idle timeout (~45 s).
- Screen off / PiP while mic is on (Android 14+): audio keeps flowing — the FGS carries the microphone type (logcat: no SecurityException, no silent AudioRecord).
- WAN (relayed or direct) session: the same checks pass over the iroh path.

**11. WAN over iroh (R1)**

- On same LAN: identity cached (logcat), status shows LAN.
- Phone on LTE (Wi-Fi off): dial from cached identity succeeds → badge shows "WAN · punching…" then settles to "WAN · direct" or "WAN · relayed — trying direct…", video + input + keyframe-on-gap all work through relays (R4 A1).
- Hole punching through a real NAT (relay-free if possible): check `directAddrs` paths get used.
- After both a LAN and a WAN session: `pocketdesk sessions` shows one line per session with distinct `outcome=` (`lan_direct` vs `wan_punched`/`wan_relayed`), plausible `rtt_ms`/`kbps`, and `dev=` = the paired deviceId.
- Tail latency (roadmap 2141-2146): the same lines carry `enc_n`/`enc_p50`/`enc_p90`/`enc_p95`/`enc_p99`/`enc_max` and the matching `rtt_*` set. `enc_n` counts frames the encoder actually produced a packet for, so it must be non-zero and of the same order as fps × session duration — a large shortfall is its own finding (frames dropped upstream, or a path that never samples). `enc_p50` must sit near the typical frame time for the encoder rung in use (the log line names the backend at session start), and a deliberate stall — crop to a window, or unplug/replug Wi-Fi mid-session — has to move `enc_max` or `rtt_max` above `enc_p50`/`rtt_p50`. **A line with no `enc_*` at all means the sampling was never wired to the live pipeline: that is a bug, not a healthy session.**
- Relayed → direct upgrade while streaming: badge flips to "WAN · direct" on its own (iroh keeps punching) and the bridge logs "upgraded from relay to direct path" (logcat). Relayed session must stay usable the whole time, not stall on the transition.
- Relayed session (R4 A3): within ~2 s of the badge reading "relayed", the desktop log shows "Encoder bitrate target changed" (relay_cap engaged) at effective ≤ 2 Mbit/s followed by "Encoder rebuilt for new bitrate", and picture is visibly softer than LAN/direct.
- Full-quality override (R4 A3): while relaying, the badge area offers "Full quality: off" — tap → log shows "Relay quality override changed" then a fresh "Encoder bitrate target changed" back to configured (`pocketdesk sessions` kbps over the next session line). Tap again to re-clamp; toggle survives a stream retry (re-arms).
- Clamp release on punch-through: start relayed (override off), wait for the direct upgrade → within ~2 s the log shows "Encoder bitrate target changed" back to configured without touching anything.
- LAN session (R4 A3): the relay clamp never engages (`stats().relayed` is always false on quinn → relay_cap is unbounded); the arbiter task still runs but emits no bitrate-change line until a preset is chosen below or the link starts losing packets.
- Loss response (roadmap 2053): degrade the link — `tc qdisc add dev <iface> root netem loss 5%` on the desktop, or walk the phone toward the far edge of the Wi-Fi mid-session — and within a few 2 s ticks the desktop log shows "Encoder bitrate target changed" carrying a `loss_cap=` field with effective below configured, then "Encoder rebuilt for new bitrate" and a visibly softer picture. The cut is 20 % per congested tick down to a 1 Mbit/s floor (a configured rate below the floor is never raised to it). Restore the link → the same line reappears **without** `loss_cap=` as the rate climbs back ~10 % per tick to configured, then stops being a term at all.
- Loss response must not fire on a quiet desktop: hold the screen still (no window movement, no video) for a minute and the log must show no bitrate-change line. Below 100 datagrams in a tick the controller treats the sample as no information, which is the only thing distinguishing "the link is lossy" from "there was nothing to lose" — a bitrate cut on an idle LAN session is a bug, not a measurement.

**11b. HUD link-profile presets (R4 E5)**

- Session bar shows "Quality: auto" by default. Tap it once → "Quality: max" (blue), then "balanced", "economy", back to "auto" — the button cycles all four.
- On LAN, pick "economy": desktop log shows "Link-profile preset changed" then "Encoder bitrate target changed" with effective ≤ 1.5 Mbit/s and "Encoder rebuilt for new bitrate"; the HUD bitrate/`pocketdesk sessions` kbps drop and the picture softens. Native resolution is NOT reduced (only bitrate) — verify the HUD reports the same width×height.
- Pick "balanced": effective ≤ 5 Mbit/s. A configured rate below a band (low-res monitor) is never raised above it (the band is a ceiling, not a floor).
- Preset survives a stream retry / monitor switch: choose "economy", trigger Retry — the new pipeline re-arms the preset (stays ≤ 1.5 Mbit/s without re-tapping). Server latches are per-session, so a full reconnect to a fresh `StreamingServer` resets to Auto (phone must re-pick) — expected.
- A preset change mid-action is seamless: no freeze longer than one keyframe; the post-rebuild IDR reseeds the decoder cleanly (no green/garbled persist).
- Auto is a no-op on (re)connect: with "Quality: auto" and a LAN session, tapping Retry sends no preset packet and logs no "Encoder bitrate target changed" (only the relay floor would fire, and not on quinn).

**12. Roaming (Tier-3 #13 remainder)**

- Mid-stream, toggle Wi-Fi off → phone falls to LTE → within a few seconds status chip shows connecting and a fresh WAN/LAN session comes up (reconnect-on-new-radio, not live migration).
- Wi-Fi↔LTE ping-pong with 3 s cooldown: flapping radio must not thrash a healthy stream.
- In PiP: network change must NOT tear the float session (pip guard).

**13. PiP + multitask (Tier-3 #14, DeX)**

- "PiP" button → chrome hidden, video-only float with correct aspect (matches decoded frame size after first SPS); input goes to system.
- Expand back → chrome returns, session uninterrupted.

**14. Blackout / pocket mode (Tier-3 #15 remainder)**

- "Blackout" → black screen, "double-tap to unlock" hint, screen dimmed; stream continues (watch desktop from another device or check fps in logcat).
- Recents thumbnail blank during blackout.
- Double-tap unlocks.
- Entering PiP while blacked-out auto-clears the overlay.
- Pocket test: with overlay on, phone in pocket should not send spurious input (touch consumed by overlay).

**15. Language / i18n (Tier-3 #17 remainder)**

- Switch system per-app language to Español / தமிழ் → all screens, sheets, FGS notification text localize (telemetry units, key labels, window titles stay as-is by policy).
- Mid-session language change (if the OS applies live) → no crash; format strings (`%1$s`) render correctly in both locales.

**16. Long-run / hygiene**

- 1 h continuous session: memory stable (no decoder leak across resolution changes), fps steady, no watchdog kills.
- Airplane-mode mid-session then back: clean Down chip + retry, no ANR.

**17. Foldable / tablet dual-pane (R4 E6)**

- Rotate to a horizontal hinge → split flips to top/bottom (stream top, dock bottom) and vice-versa.
- Toggling panes mid-session does NOT drop the stream (the decode session survives the fold — check fps continuity / no re-Keyframe gap).
- Direct-touch taps on the video pane still hit the right desktop point (letterbox/aspect-fit recompute for the half-width pane).
- Shortcut bar appears once — in the dock while dual, under the stream while single.
- Blackout still covers BOTH panes (whole window), not just the stream.
- es/ta: "Pane:" and trackpad hint localize.

**18. Login autostart + tailnet access (server-side)**

- Phone reconnects to the auto-started server WITHOUT re-pairing (QUIC identity persisted in `~/.local/state/pocketdesk/certs/`; a regenerated cert would trip the TOFU pin).
- Log out / session end → service stops with the session (PartOf), never orphaned.

**20. Saved-computers home screen (multi-host store + add-computer form)**

- Wake row + wake status appear only for hosts the phone knows how to wake (since N6: what the computer announced, or a typed MAC; see §42).
- Multi-host scenarios (second card ordering, remove-then-readd) and es/ta strings for the new keys.

**21. UI polish pass C1–C9 (design system, immersive session, error UX, settings)**

- Theme follows the system light/dark; toggling the mode in Settings restyles every screen immediately (no restart), and the window background matches on cold start (no white flash in dark mode or vice-versa). *Partly verified: the Dark radio restyles Settings immediately; cold-start window colour not yet checked.*
- Edge-to-edge on all screens: content draws behind the status/nav bars and no control is unreachable under them.
- Pairing sheet: progress bar while waiting, PIN field auto-focused, eye toggle honors the reveal-PIN pref, animated check on success then auto-dismiss.
- Home empty state (no saved computers) shows the desktop glyph + explainer.
- Siren: a desktop-pushed `kdeconnect.findmydevice {ring:true}` rings the alarm AND shows the full-screen dialog; Silence stops both; the 30 s auto-stop still works if the dialog is ignored.
- Clipboard sheet: Clear asks for confirmation first (Cancel keeps history); empty state has the clipboard icon.
- Haptics: tick on the action disc, mode toggle, picker picks and shortcut chips; heavier buzz on Exit; the Settings haptics switch mutes all of them live.

**22. Owed verification register**

- **PASS condition:** zero `Incoming v1 TCP connection from` lines (`server/src/service.rs:330`) after the single session-establishing connection, while every sheet and picker above is used.
- **PASS condition:** zero `Broken pipe (os error 32)` WARN lines from any plugin.
- **PASS condition:** zero `Kicked 1 stale session(s) due to reconnect storm` from the server.
- Battery/clipboard/monitors/window rows still show *current* values — the shared connection must not trade churn for staleness.
- The no-session fallback still works: a query issued with **no** live control session must take the `oneshot_request` branch (`android/bridge/src/api.rs`, reached from `control_request` when `CONTROL_WRITER` is `None`) and still get an answer — that is the path that runs before you connect, and it is the one the rewrite could plausibly have broken. Same for `push_packet`'s dedicated socket.

**23. Windows host (W1: core remote desktop)**

- `install` registers the "Pocketdesk" task (Task Scheduler → Task Scheduler Library), removes a "Pocketdesk" task an older version left, starts the server with no console window, and the log file appears under `%LOCALAPPDATA%\pocketdesk\logs`.
- A toast "Pocketdesk pairing PIN: …" appears for `pair` (the `a_toast_is_shown` test only proves Windows accepted it).
- Window crop: picking a window streams just that window; closing it returns to the monitor.

**24. Desktop app and QR pairing**

- Settings → Save on a real `config.toml` keeps comments; the server picks the change up after Restart.
- Open a `pocketdesk://pair` link, finish or dismiss it, then switch dark mode or reopen the app from Recents: the pairing is not offered again. Switching theme while the confirmation is showing keeps it.

**25. Desktop sound on the phone**

- Quick Settings → "Desktop sound on this phone" off: silent at once; on again: sound returns within a second. The setting survives an app restart.
- Pause the desktop's audio for 10 s, then resume: it comes back cleanly, without a crackle burst.
- Connecting to a silent desktop does not pause music already playing on the phone; the first desktop sound takes over. "Silent" means no app holds an audio stream open: Windows loopback sends packets of silence while any app has one (seen on the A71 with a browser open: focus was taken 1.3 s after connect), and focus follows packets, not loudness.
- A phone call silences it, and it resumes after the call. Another app starting music takes over for good: switching "Desktop sound on this phone" off and on brings it back.
- In PiP the sound continues; Exit stops it.

**26. In-session keyboard**

- Landscape: the stream stays visible above the keyboard, and zoom survives opening and closing it.

**27. Trackpad gestures**

- Two-finger scroll is natural (content follows the fingers, like the phone itself) at about one wheel notch per 24 dp, in both axes. Unverified: adb cannot make two-finger touches.

**28. Permissions and session approval**

- No answer: the phone shows the refusal about 30 s after connecting.
- Running commands: denied by default; allowed after the switch is turned on.
- Control off: the session is view-only and the phone cannot lift it. Sound off: no desktop sound. Microphone off: the mic does not reach the desktop.
- Turning control or the microphone off during a session ends that session. Removing a connected phone ends its session.
- LAN session: the prompt shows the phone's address.

**29. Computers on this network**

- Pair from a found computer: after the PIN it appears under Saved.
- "Visible on this network" in the desktop app, then a server restart: the computer no longer appears.

**30. Sending files to the desktop**

- Share several photos at once from the gallery (`SEND_MULTIPLE`): "Sending 2 of 3" with a moving progress bar.
- The desktop shows a notification per file; the app's Files tab lists them, and Show in folder selects each in Explorer.
- A file from a provider without a size (e.g. some cloud apps) still arrives (copied first).
- A share with the desktop off or unreachable ends in "Not sent: 1 of 1" with the reason.
- A non-English file name arrives with its name.

**31. Files both ways and the remote browser**

- A download refused by the desktop (an offer used twice) fails at once with its reason, not after 30 s.
- Send a file while a large share from the phone is running; the offer still arrives.
- A phone that has no room declines an offer; the desktop's Sent list says "the phone declined it: …".
- Tap "Saved … to Downloads"; the file opens.
- Download into a phone without enough space: "Not enough space for …", nothing left in Downloads.
- A file over 1 GB both ways, and a download stopped halfway (Wi-Fi off): no partial file is published.

**32. Switching screens**

- Unplug the monitor being streamed; the session falls back to the primary screen.
- Three or more monitors: a chip each, in the desktop's order.

**33. Shortcuts outside the app (F11)**

- Place the widget on the home screen: it names the last computer and connects on a tap; with none saved it opens the app.
- A Wake shortcut appears for a computer the phone can wake (§42), and sends the packet.
- Pin a shortcut to the home screen; it survives the computer being reconnected, and stops working once the computer is removed.

**34. Desktop diagnostics (F10)**

- A network Windows files as Public, with only `install`'s rules: the firewall check says no rule lets phones in.
- A Block rule for pocketdesk: the firewall check reports it.

**35. Reaching the desktop from outside (N1)**

- A network with public IPv6: the IPv6 line is ok.

**36. Your own relay (N2)**

- HTTPS mode on a cloud VM with a real name: the certificate is issued, and a session connects with `--allow` ids from `relay-ids`.
- A phone that is not paired (or an id not listed) is refused by the relay.

**37. Found by id on the Mainline DHT (N3)**

- `dht_publish = "addresses"` on a network with public IPv6: the record holds the IPv6 address, and a phone with relays unreachable connects directly.
- A network that blocks outgoing UDP: "Found by id" warns that it was not read back.

**38. Image clipboard (N4)**

- Desktop → phone: paste it into a chat app on the phone. (The A71 has no app that pastes images.)
- Phone → desktop: copy a photo on the phone and paste it into Paint. (The A71 has no app that copies an image: no browser.)
- With the phone's Clipboard permission off, neither direction moves.

**39. The phone's notifications on the computer (N5a)**

- A messaging app that hides its content on the lock screen shows only "New notification".
- With no computer reachable, the phone's battery and the queue stay calm over an hour.

**40. Closed on the phone, closed on the computer (N5c, part 1)**

- A chat app that clears its notification when read on the phone.

**42. Wake from the phone (N6)**

- A computer saved by its tailnet address counts as awake once its server answers.
- Turn "Wake from the phone" off on the desktop: after the next connection the phone forgets the MAC, and Wake disappears.

**43. Several phones at once (N7)**

- After 10 s without input from the driver (or its session ending), the other phone can take control.
- Sessions tab: the driver is marked "Driving"; Give control hands it over within a second.
- Settings "Several phones": every phone drives; the hand-back time.
- Two quick Ends in the Sessions tab both end their sessions.

**45. Phase R3 and R4: hostile connections, and idle cost**

- The phone's battery over an hour with the app in the background, and while a session is open.
- Network traffic while idle (both ends).

**46. Phase R2: long sessions and recovery**

- An hour with one session from start to end, to confirm the memory levels off.
- A file transfer cut by a drop (today it fails and is sent again whole).

**47. Check connection (T1)**

- A VPN that captures home traffic, and Tailscale off for a tailnet address.
- Guest Wi-Fi with AP isolation, and another subnet.
- A UDP-only block (the computer's firewall dropping the stream port).
- Copy report on a device: no address or id in the text.

**48. Staying connected in the background (T2)**

- After allowing it in the battery settings, the card is gone on return.
- Battery use set to Restricted: the card says so, and "Allow" opens the app's page.
- Other makers (Xiaomi, OPPO): the guide link and their own battery managers.

**49. Protocol audit (what Pocketdesk sends, and to whom)**

- Android backup: `adb shell bmgr backupnow dev.pocketdesk.android` backs up nothing, and a device-to-device move (Smart Switch) leaves the app unpaired on the new phone.
- A Wi-Fi with Tailscale up: a phone browsing mDNS sees the LAN address and no 100.x or fd7a: address.
- Transfers both ways still end "saved" over many runs (the clean close after the status byte), and a desktop share to a full disk fails at once instead of after 10 s.

**50. Resumable transfers (T6a)**

- A file from the remote browser broken and continued.
- "Retry failed files" off: a broken file fails at once, and no part is kept. Tried 2026-10-19: the phone's Wi-Fi off for 200 s in a 900 MB download did not break it; QUIC carried it on when Wi-Fi came back and the whole file arrived ("sent"), so a real failure is still owed.
- "Unfinished files: not kept" on the desktop: a retry starts over.
- A file that changes between attempts starts over (the prefix hash).
- The app killed mid-download, then the offer fetched again within its 10 minutes.

**51. Folders (T6b)**

- A folder from the phone shows one "folder arriving" toast, not one per file.
- The phone shows "Not all of … was sent" when it leaves names out, and "Stopped sending …" when the computer goes away mid-folder.
- A folder broken by Wi-Fi mid-way, continued.
- A folder over the limits (10 000 files, 32 levels) says what was left out.
- The desktop app's "Choose folder…" and dropping a folder on the window.
- Names Windows refuses on the phone (`a:b`) left out and counted.

**52. Download a folder from the remote browser**

- Closing the sheet before the answer, and the toast still showing.
- A folder of thousands of files: the answer arrives, and other queries keep working while the offers come.

**54. Automation apps on the phone (T7b)**

- A share naming one of two saved computers goes to that one (the A71 has one saved).
- From the background, with and without "Display over other apps".

**55. A slow or lossy link (fix/slow-link-watchdog)**

- A healthy LAN: frame rate and e2e unchanged (the in-flight bound must not throttle it).
- WAN, direct and relayed: frame rate at a 150-300 ms round trip.
- The picture's delay on the lossy link (how stale it runs), and a keyframe after a long stall.

**56. Links both ways (T8a)**

- Chrome's and Firefox's share sheets ("Open on computer" listed, the title and link they share).
- The desktop app's Send link, and its refusal for a phone with Links off.

**57. The phone's battery on the computer (T8b)**

- The Devices tab's text, and its age after the session ends.
- Catching up after a Wi-Fi drop.
- The desktop's "Phones' batteries" off: no notification.
- A real discharge to 15 %, and Android's own low flag.

**58. The computer's media from the phone (T8c)**

- Nothing playing: "Nothing is playing" (2026-10-19: something was playing on this box, so not reached).
- The card cleared to the error alone with Audio off (fixed after the run).

**59. The phone's media from the computer (T8d)**

- The Sessions tab itself, and its buttons.
- The phone's setting turned off clearing the song; a press sent while the app was in the background not acting later.
- Turning the setting on opening the notification access page.

**60. Pinch-zoom in trackpad mode (T9a)**

- In trackpad mode, two fingers spreading zoom the view, pinching zooms out, back to 1x.
- Two fingers moving together still scroll the desktop, and a scroll that drifts apart does not start a zoom.
- While pinching, moving the fingers pans the view; one finger lifting and landing again does not jump the zoom.

**61. Portrait sessions (T9b)**

- A 16:9 desktop's pane, a monitor switch between shapes, picture-in-picture, and Pane: Single and Dual.
- Pinch-zoomed upright: the zoomed picture stays inside its area, not under the chrome (adb cannot pinch).

**62. A mouse on the phone (T9c)**

- Motion, all five buttons and the wheel (both ways) reach the desktop, and a fast mouse keeps its travel.
- Opening and closing the in-session keyboard: the mouse keeps working.
- The mouse's Back button goes to the desktop and does not end the session.

**64. The control channel over the internet (T10a)**

- A phone with no iroh key registered (paired, never streamed on the LAN) is refused, with one log line, and works again after one session at home.
- A file offer to a phone on the internet link fails as "not reachable", not stuck.

**65. The clipboard outside a session (T10b)**

- With it off, a session still gets the clipboard on request.

**66. Answering the phone's notifications from the computer (T10c)**

- "Hide sensitive content" on the lock screen turned on: the probe's notification arrives as "New notification" and offers nothing.
- The switch off: nothing offered, an answer not run.
- An action that opens an activity on Android 14+ (background launches may be blocked).

**67. Files from the computer without a session (T10d)**

- A phone that refuses background starts: the "The computer sends a file" notification, and Save downloading it.
- The desktop app's Files tab listing a linked phone with no session.

**69. "Send to phone" in the file manager (T10e)**

- Send To in Explorer itself, with several files and a folder.
- The desktop app's Settings switch, and `install` / `uninstall` adding and removing the entry.
- No phone connected: the notification says so.

**70. The phone's folders on the computer (T10g)**

- The desktop app's Phone tab itself (roots, folders, Up, Download).
- A folder of more than 2000 entries; a file renamed between list and fetch.

**72. New phone screenshots on the computer (T10h)**

- A screenshot with the side keys, and a JPEG screenshot (One UI's other format).

**73. The phone as a remote (T10i)**

- The owner's approval prompt ("Ask before each session") before ready.
- Over the internet road, and a phone driving while another has a picture session (driving rules).
- I1: the computer's server restarted under a remote: "Reconnecting in N s…" on its card, then the remote back by itself, the talk timer still counting; Kick from the computer: not retried; Settings "Reconnect by itself" off: Retry only; after ten minutes, "Stopped trying" and the session's notification gone.

**74. The phone's battery without a session**

- With only folders, answers or files keeping the link; with none on, nothing sent and no connection made; the link dropping just before a report (nothing dialed).
- Over the internet road.

**76. Hotspot and tethering in Check connection**

- The same computer saved under its home address: the network finding, with "This phone is sharing its connection" beside it.

**77. This phone's screen on the computer (T10k)**

- The notification's Stop (the A71 showed only the redacted notification in the shade while locked), and Sessions' "Show screen" after the window was closed.
- "Share one app" instead of the entire screen; a phone call or a secure screen (banking apps black out); a long cast's heat and battery.

**78. Events and hooks (ecosystem 1a)**

- `notification`, `text_received`, `file_received` and `link_received` from a phone, at both detail levels.
- The desktop Settings hooks box saving a hook the server then runs; "Event log" off writing nothing.
- A hook killed at 30 s, and the 33rd waiting hook skipped, on a real server.

**79. The Activity Center (ecosystem 1b)**

- Pairing and unpairing a phone showing as rows; a folder's Show in folder after it finished arriving.

**80. Named tasks (ecosystem 1c)**

- The desktop app's Allow / Deny prompt itself (clicked, not written), Deny on the phone, and the Settings task editor.

**81. Send from the browser (ecosystem 1d)**

- The extension itself loaded in Brave or Chrome and in Firefox (unpacked), and its menus and notifications.

**84. The reliability pass before ecosystem phase 2 (R6)**

- A download that waits for the network (next phase, R6i).

**85. iroh 1.3, one endpoint per computer, files over the internet road (2a–2d)**

- The remote file browser's download and a folder fetch over the internet road.
- 2e: a screenshot copied on the computer lands on the phone's clipboard over the internet link (and phone → computer, when an app on the phone can copy an image).
- The too-old message against a computer from before 2c.
- Both Settings switches off: a send refused at once, with the reason.

**86. Two computers: pairing, the link, the clipboard and files (2f–2h)**

- Pairing from the Pair tab's copied link pasted in Settings → Other computers; the outcome shown; B's Devices tab lists A with Clipboard, Files and Links only, and ask first.
- The link survives a restart of either computer, and comes back after the network drops ("linked" again within a minute).
- The clipboard both ways in under 2 s with "Clipboard with my computers" on on both; off on one, nothing crosses either way; a password manager's copy stays.
- A 500 MB file each way from the Files tab, broken (the network off for 15 s) and continued, MD5 matching; `pocketdesk send <name> <file> --wait`.
- A folder from B to A lands under one new folder; a folder from A to B is refused with its reason.
- Unpair on A (Settings → Other computers): the link goes, and B's Devices page still lists A until unpaired there; after that, a pairing again works.

**87. KDE Connect devices (3a–3c)**

- Cancel after the device's Accept unpairs it on the phone; a second request from the phone while one waits shows nothing new.
- A device that broadcasts first (this computer dialing it): not built (the receive half waits).

**88. The desktop app's shell (U1)**

- A notice clearing after 6 s, "Copied.", and the two settings' switches while their change runs.
- A line mixing Arabic or Hebrew with Latin (egui has no bidirectional layout), read by someone who reads it.

**89. The phone's navigation and feedback (U2)**

- Ring's snackbar (it rings the computer), and a shortcut key's failure message on a dropped link.

**90. Names on both sides (U3)**

- A computer's name from a scanned pairing code, the session notification and launcher shortcuts by name, and "Lock <name>?".

**91. Ring the phone from the computer (N1)**

- Do Not Disturb with alarms allowed and not; a phone with a secure lock screen (the full-screen ring and Stop); a phone where the full-screen permission is not granted.

**92. Phone → computer text in one tap (N2)**

- A copy marked sensitive (a password manager's) asking first through the tile.

**93. The phone's Home card, Add form, Settings and session menu (U4)**

- A row's "Allow notification access" with a feature on and access off.
- Landscape: the bottom chrome with 48 dp chips.

**94. The desktop's Settings and words (U5)**

- Opening and closing sections; a change shown as applying now or after a restart in the save bar, and the restart notice naming it.
- A narrow window wrapping every row; a section with a problem held open.
- Unpair's question for a computer.

**95. The phone app in Spanish and Tamil (U6)**

- The keyboard bar's keys and the remote's presenter keys in Spanish.
- Every screen in Spanish and Tamil for text that runs out of its box.

**96. Answer a phone notification in the toast itself (N3)**

- Typing a reply in the toast and pressing Send: the phone's app gets it (its RemoteInput), at most 10 a minute.
- Pressing "Mark as read" in the toast, and from the Action Center after the banner has gone.
- Settings "Answer in the notification" off: the toast is plain.

**97. The phone as the computer's speaker and mic (N5)**

- Ten minutes without the computer: it stops trying and releases the service.
- An hour with the screen off: the session holds, and what it costs the phone's battery.

**98. An Android 17 pass (N4)**

- The app starts, asks for local network access, and a session works once it is granted.
- Ring from the computer with the app in the background and `adb shell cmd audio set-enable-hardening enable`: the notification sounds at the alarm volume, the volume is not raised, and no `AudioHardening` entry names the app.
- On Android 16 and older (the A71): the ring still raises the alarm volume and puts it back.
- A text with a code: does it reach the computer at all after the three hours, and with its own time?
- A ring that raised the volume on Android 16, cut short, then the phone on 17: the volume is put back when the app is opened.
- A screen, remote and sound session with the screen off: the computer's sound keeps playing (the session's foreground service holds while-in-use).

**100. GNOME and KDE hosts that work unattended (N8)**

- "Remember the screen-sharing grant" on: a session from the phone asks once at the desk; a second session and a reconnect start with no dialog.
- No RemoteDesktop portal (or refused): the screen alone.

**101. KDE Connect, next steps (N9)**

- A chat app's notification, an update to one (replacing the toast), and a flood held to the limit.

**102. The phone's background cost (N7)**

- The same hour on battery, without USB: the battery's drop, and the CPU while the phone really sleeps.
- Over Wi-Fi rather than `adb reverse`: the radio's wakes and bytes for the link's keepalives.
- A release build (R8): the CPU again.

**103. Transfers you can watch and stop (C1)**

- A folder cancelled on the phone: the app's notifications sit folded under "Silent" in the shade, so its Cancel was not reached by script (a single 1.5 GB file's was, in C1).
- A share from the phone cancelled on the computer, then shared again.

**104. Control the phone from the computer (C8)**

- The phone locked with a PIN: taps and typing refused, Back and Home taken.
- A password field.
- Over the home network (not the relay): latency again.

**105. Choose where received files land (C2)**

- The phone's grant taken back in Android's settings, then a file.
- A download broken and continued in a picked folder.

**106. Notifications that wait and go later (C3)**

- Doze on battery without USB, where the phone suspends; a notification waiting through it and arriving marked late.
- A computer from before C3: notifications arrive once each, as before.

**108. The phone with large fonts, landscape and TalkBack (C5)**

- Split screen: Home and a session in half a screen.
- Landscape with the keyboard open (§26).
- Spanish and Tamil at 1.0, 1.3 and 2.0 (a long word such as "Individual" may already make a list at 1.0).

**109. The owed checks by where they run (C6)**

- The rest of "here", in later batches.

**110. Recent photos on the computer (C7)**

- The Phone page by hand: Refresh, Save, Open and Copy.
- HEIC photos: thumbnails shown, Copy saying it cannot.

**111. Health after updates (D1)**

- Advanced Protection on, with control on (needs the owner's Google account).

**112. Quiet hours kept in step (D2)**

- The Overview's Mute and Unmute by hand, and the refusal's words.
- Android 14 and older (one filter for all, no app mode).
- A Do Not Disturb that silences but still lists notifications.
- A phone with a bedtime mode switching on during a mute.

**113. Clipboard history and Resend (D3)**

- The Clipboard page by hand: Copy, Send to…, Clear and its confirm, a cut entry's Send greyed.

**114. Continue reading on phone (D6)**

- Chrome (Windows): "Continue reading on phone" on a long article mid-way; the phone's notification opens it in Chrome scrolled to that paragraph.
- A page with a `#section` link: the fragment replaced, the page still scrolled there.
- The browser's own page (`chrome://extensions`): sent at its top, no error.

**116. The tray you act from (D5)**

- Windows, the A71 linked: its submenu titled with its battery, which changes in the menu when the battery does.
- Send clipboard: the computer's copy on the phone; a password manager's private copy not sent.
- Send file…: the window opens on Files with the phone chosen.
- The A71 away: its items greyed out.
- Send clipboard with an empty clipboard, and with over 256 KiB of text: a notification says it was not sent.

**117. The phone's folders as a drive (D8)**

- File Explorer at the Windows address: browse, open a photo, copy one out.
- A file over 50 MB in Explorer (refused by Windows' limit).
- A phone from before D8: the file lands in Downloads, the drive answers 504 after the wait.

**118. Lock when the phone leaves (D9)**

- The phone back, and gone again: a second line; staying gone: no more.
- The switch on the Devices page, and the time in Settings, applied without a restart.
- A failed lock (the seam's file not writable): tried again a minute later, and `away_locked` with `failed`.

**119. Memory at rest (D7)**

- The app with a phone linked and a session running, then at rest again: back to its number.

**120. Answers that say how they went (E1)**

- A desktop notification for such a failure.
- A phone from before E1: "sent, but no word from the phone" after a minute. Seen in passing 2026-10-19: an answer sent into a link the phone had just dropped (the app force-stopped) stays "sent", which the page shows as no word back.
- From a toast (N3), the window hidden: a failure's desktop notification.

**121. The tray on its own (E2)**

- With the window closed: a phone that must ask, an ask-first task, and a phone's "Show screen" each start the window (once each), with the prompt or the screen.

**122. The phone as a pen (E3)**

- Windows: a pen session from the A71 into that window: pressure following the touch, Down and Up once per stroke.
- A drawing app with pressure (Krita or Paint) on Windows: strokes thin and thick with the touch.
- Pen pressure Soft, Even and Firm change the ink for one touch.
- A pinch while in Pen lifts the pen and zooms the view, and the stroke does not go on after it.
- Pen is not offered on a computer from before E3, and a session that meets one again goes back to Direct touch.

**123. A gamepad on the phone (E4)**

- Closing it (× or Back) with a button held lets the button go.
- A game (one that takes Enter, the arrows and Space) by hand.
- A real controller connected to the phone (none here): buttons, D-pad or hat, both sticks; Settings off leaves it to Android.
- Two buttons at once, and a stick held while pressing a button.

**124. The desktop app's design pass (E5)**

- Each page at the 640 px minimum width, in both themes: no row runs off the page (Devices, KDE Connect, Activity, Notifications, Phone files, Sessions).
- The Phone page with a long folder: the photos below are in reach, and every photo row shows.
- The cast window's bar in the light theme: its buttons readable.
- KDE Connect's Unpair… asks by name; Cancel keeps it.
- Devices: a paired computer has no Ring, Mute, cast or lock switches; the lock's note shows while it is on.
- Notices: "Asked the phone…" on the Phone page goes in seconds; an error shows in red.

**125. The phone app's design pass (E6)**

- The session menu with Pen offered: three segments at 1.0; at 2.0 crowded.
- The history sheet's entries 48 dp, and no Clear when empty.
- The Health card's Choose opens the folder picker.
- Spanish and Tamil on the changed screens.

**126. The deck (F1)**

- A button of each kind set up in the desktop app's Settings, listed on the A71's Deck from Home and from the session menu, with its icon.

**127. How the computer is doing (F2)**

- "How it is doing" on the A71 against this box: the CPU moving under load, memory, each fixed drive (no removable or network drive), the battery when there is one, uptime.
- "This computer's state" off in desktop Settings: the sheet says so, without a restart.
- The sheet's asking stops when it closes; Spanish and Tamil; font scale 2.0.

**128. Take a photo with the phone (F3)**

- "Also onto its clipboard" off: the file only; "Photos the computer asks for" off (the default): no notification, and the Phone page's button disabled while the phone is not linked.
- Spanish and Tamil.

**129. An air mouse (F4)**

- "Air mouse speed" at 0.5 and 3; a slow link (moves add up, no replay).
- A phone without a gyroscope keeps the trackpad.

**130. Ringing calls on the computer (F5)**

- A call ringing on the phone: "Call from <name> (<number>)" on the computer within a second or two, in the toast and on the Notifications page; gone when it stops ringing (answered on the phone, declined, missed).
- Silence from the toast: the ring stops (or the refusal, said); the ringer's volume back after the call.
- Decline (with "Let the computer decline calls"): the call ends; with a call waiting, which one ends.
- The permission granted while notification access was already on: the next call shows (no restart).
- Two calls one after another; a call ending while its note is on its way.

**134. The deck on the home screen (G3)**

- The server stopped: "No answer from the computer yet" after 8 s, never a stuck "Asking…".

**136. The computer's music on the home screen (G5)**

- Place "Pocketdesk: playing" with music playing on the computer: its title and artist, Previous, Pause and Next as the player offers them, "Read at <time>".
- Pause from the widget: the music stops, and the button turns to Play; Next: the next track shown.
- Nothing playing: "Nothing is playing", no buttons; Audio off for the phone on the computer: the refusal, no old track.
- The server stopped: "No answer from the computer yet" after 8 s, never a stuck "Asking…".

**137. The computer's music from Quick Settings (G6)**

- Add "Computer's music" to the A71's Quick Settings; with music on the computer, a tap pauses it, the shade still open showing "Asking the computer…", then the track, the tile off; a tap again plays, the tile on.
- On the lock screen: a tap asks to unlock first.
- Opened after an hour: "Tap to play or pause", not the old track.

**138. Wake the computer from Quick Settings (G7)**

- Add "Wake computer" to the A71's Quick Settings: its subtitle names the last computer; with its MAC not known yet, "Connect once while it is awake".
- A tap with the computer awake: "Sending the wake…", "Sent, waiting for it to answer…", then "It is awake"; a tap again at once is ignored.
- On the lock screen: a tap asks to unlock first. With no computer saved: a tap opens the app.
- A computer asleep (not this box): woken, "It is awake" within a minute.

**139. A talk timer in Remote (H2)**

- The screen off, the phone in a pocket: the buzzes still come on time (the session's foreground service).
- Do Not Disturb on: the buzzes still come; Settings "Talk timer buzzes" off: none.

**140. Send the clipboard to the phone with a key (H3)**

- Settings "Send the clipboard with a key" = `win+shift+c`, Save: "Registered." under it; with the A71 linked, copy a text on the computer and press it: the text on the phone.
- A combination another app holds: "Another app holds this combination"; freed, registered within 30 s.
- A password manager's copy: not sent (the server's notification says why).

**141. The home-screen widgets, a design pass (I2)**

- The music widget with a track: title, artist, the buttons at 56 dp and "Read at <time>" in its header; nothing playing.
- The state widget made one cell tall: the line, no "%" left alone; on its side at two cells: the line where bars do not fit.
- Spanish and Tamil (the bar labels at 68 dp); font scale 2.0.

## Power, lock or server stop on the owner's Windows box: never run here (the owner's rule)

**2. Session shell + video (Tier 1 #5, R2#2, R2#5)**

- R4 C1 forced HEVC round-trip: `allow_hevc = true` in config.toml, restart server, reconnect → log shows `codec=H.265 (HEVC)`; phone decodes (sniffed `video/hevc`) with video within ~1 s and input still mapped. A pre-C1 phone build (no caps stream) must still get H.264.

**2d. Rate and link basis (roadmap 2056)**

- The stall check this replaces: freeze the desktop's output (sleep the display, or `SIGSTOP` the server) → HUD `fps` and `kbps` must reach 0 within ~3 s and stay there, with the caption still naming a real span. Under the old lifetime average this was the bug: a dead link read healthy for minutes. Resume → the rates recover within ~3 s, not within a session's worth of averaging.

**7. Clipboard + files (Tier 1 #4)**

- Reboot phone → history + saved hosts survive.

**9. Privacy mode + lock (Tier-3 #15, R4 E4)**

- "Lock PC" / session-notification "Lock desktop" → screen locks (loginctl path).

**11. WAN over iroh (R1)**

- Desktop reconnects (restart server) → identity re-announced within 30 s, phone re-dials.

**18. Login autostart + tailnet access (server-side)**

- Reboot the desktop → after reaching the Hyprland session, `systemctl --user status pocketdesk` is active without any manual start (WantedBy=graphical-session.target).

**23. Windows host (W1: core remote desktop)**

- A UAC prompt or Win+L mid-session: the stream recovers after it is dismissed (WGC keeps running; on the DXGI fallback, access-lost rebuilds the duplication).
- `pocketdesk stop` from a second terminal stops the server cleanly (the log ends with `Shutdown requested (pocketdesk stop)`) — run this yourself; it is deliberately not automated.
- `uninstall` removes the task and the firewall rules (`wf.msc`), including the "Pocketdesk" ones an older version created.

**24. Desktop app and QR pairing**

- Pair over the LAN address in the code, not `adb reverse` (needs the firewall rules `install` adds).
- Start / Restart server from the app, on Windows (logon task) and Linux (systemd user unit). This is not run on the developer box by rule.

**42. Wake from the phone (N6)**

- **Sleep, then wake from the phone at home** (Ethernet): the computer wakes, and the card connects once it answers.
- **From outside:** forward a UDP port on the router to the computer, set it as "Outside wake port", sleep the computer, and wake it from a phone on mobile data. Note how long after sleep the router still delivers it.
- Hibernate and shutdown (Windows fast startup off), and Wi-Fi only.

**49. Protocol audit (what Pocketdesk sends, and to whom)**

- Wake through a relay computer with Power off for the phone: the phone says the relay did not answer and asks about Power, not "Wake sent". With a target off the relay's networks: "The relay computer refused: not on this computer's network".
- The desktop app's Devices tab reads "Lock, sleep, privacy mode and waking others".
- Windows: after `pocketdesk install`, the firewall has TCP 1716 and UDP 4716 rules only, and a phone on a fresh machine still lists the computer by mDNS (no UDP 5353 rule is made).

**89. The phone's navigation and feedback (U2)**

- Lock confirmed, and refused with Power off (the server's answer is unit-tested): never against this box, so on the VM or a spare computer.

## Needs the owner's OK on this box: input reaching Windows, or its display settings

**0. Prerequisites**

- Desktop: user in `input` group (uinput + `/dev/input/event*` read for privacy mode) — `groups $USER` should show `input`.

**3. Input paths (R2#4, Tier 1 #2)**

- ...and a tap is actually a **click** (roadmap 2061, re-verify 2026-09-24). The 2026-09-22 note above claimed "click (BTN_LEFT) lands", but the code that ran sent absolute motion plus a left *release* and never a press — so that half was either an inference or a window reacting to something else. A tap must now put `BTN_LEFT` down (value 1) and up (value 0) at the touched point: check with `evtest` on the abs-pointer device rather than by whether a window happened to react.
- Drag (roadmap 2061, shipped 2026-09-24, device-unverified): start a slow drag-select across desktop text — `evtest` should show the pointer warp to the touched point, `BTN_LEFT 1`, moving `ABS_X/ABS_Y` while held, `BTN_LEFT 0` on lift, and the selection must follow the finger (before this change a drag was motion-with-button-up: nothing began, and the release landed on a button that was never pressed). Then the leak check: begin a one-finger drag and add a second finger mid-drag (pinch) — the button must still come up, and the desktop must not be left dragging. Note there is no server-side safety net for a lost release packet yet (roadmap 2272): a session that dies mid-drag leaves `BTN_LEFT` held on the uinput device until the next release.
- Trackpad mode: pointer moves without jumping to touch point; tap = click; two-finger = scroll.
- Keyboard: type letters/digits/modifiers via on-screen shortcuts + remote input — check against `evtest`: Super, Alt+Tab, Ctrl+Alt+Del, PrtSc, Esc, Super+1..9 work (ShortcutBar). (Esc tap verified 2026-09-22; this box binds workspace cycling to Super+Tab, so the bar's Alt+Tab is config-dependent, not a pipeline failure.)
- The same keyboard list **run twice, once per injection backend** (roadmap 2057-2060). Under Wayland the server prefers uinput and emits the evdev code the phone sent, so nothing is translated; the enigo/X11 rung is the one that has to name every key, and it is the rung a machine without `/dev/uinput` access lands on. Read which one is live from the server log ("using uinput" vs "using enigo"), then re-run the list with the other by starting the server as a user who cannot open `/dev/uinput`. Symbols matter most here: `- = [ ] ; ' \` , . /` and the dialpad `*`/`+` were refused outright on that rung before the table was completed.

**4. Zoom + display mapping (Tier 1 #6, R3#7)**

- Letterbox: on a phone/desktop aspect mismatch, taps map through the letterbox offset (no systematic tap drift).

**5. Window + monitor streaming (R3#7, Tier-2 #10)**

- Picking a window: video re-encodes at window size (encoder rebuild), crop tracks the window, taps map to desktop coords via the screen box.

**6. Workspace HUD + battery (R3#8, Tier-2 #11a)**

- Tap a chip ≤9 → Super+N injected → workspace switches.

**8a. Control-channel versioning (R4 D4)**

- `llVersion` tag is present on our extension packets only: capture the control-channel line (e.g. `journalctl`/strace the JSON, or a debug log of `to_wire` output) — a `kdeconnect.pocketdesk.privacy`/`.audio`/`.state` packet carries `"llVersion":1`; a native `kdeconnect.identity`/`kdeconnect.pair`/`kdeconnect.clipboard` packet does NOT.
- Forward-compat: hand-inject a `kdeconnect.pocketdesk.privacy` line with an extra unknown top-level field and an unknown body key → server still parses and acts (unknown fields ignored).

**9. Privacy mode + lock (Tier-3 #15, R4 E4)**

- "Privacy: on" → physical keyboard/mouse on desktop stop responding (`evtest`: grabs held), while phone input still flows.
- Compositor shield (R4 E4, Hyprland/wlroots): the same "Privacy: on" also paints a full-perimeter red frame on every monitor within ~1 s; the desktop centre stays visible and interactive from the phone (the ring is `KeyboardInteractivity::None`, click-through). Desktop log shows "privacy: layer-shell privacy shield engaged".
- Leave session with privacy on → release happens (phone app exit); also verify 10-min TTL auto-release by force-killing the phone app mid-grab.
- Shield tears down with the grab: "Privacy: off" → the red frame disappears immediately; force-killing the phone app mid-grab → frame gone after the TTL auto-release.

**10a. Phone mic share (R4 E2)**

- View-only latched → mic keeps flowing (it is session media, not injected input); input is still dropped.

**11. WAN over iroh (R1)**

- Device-reported tails (roadmap 2143-2146, the phone's half of the chain): the same lines must carry `dec_*`, `rnd_*` and `e2e_*` plus `phone_rtt`/`phone_lost`. `dec_n` counts frames the decoder actually rendered, so it has to track `enc_n` within the loss the link imposed (a `dec_n` far below `enc_n` is dropped-or-stalled frames, not a telemetry bug), `dec_p50` must be single-digit ms on a hardware rung and grow when the desktop is cropped to a bigger window, and `e2e_p50` must sit near the HUD's e2e figure for the same session. `rnd_p50` is the rendered-frame interval, so it should be near 1000/fps ms and must jump when the bitrate clamp or a preset slows the link. On a session that never delivered video (rejected, or killed before the first frame) `dec_*`/`rnd_*` are legitimately absent — that is the "nobody measured" case, and `key=0` in their place would be a bug. **The server side of this path is already proven without a phone: `cargo test -p pocketdesk-server --test session_record -- --ignored --nocapture` drives a real capture→encode→send→receive session over loopback and asserts the record carries all of these tails; what remains device-only is that `H264Decoder.kt` feeds `dec_*`/`rnd_*` with real MediaCodec timings rather than the numbers the test injects.**

**11b. HUD link-profile presets (R4 E5)**

- View-only interaction: while a preset is active, toggling View-only still drops injected input but the bitrate preset keeps applying (both are control-plane).

**13. PiP + multitask (Tier-3 #14, DeX)**

- DeX/docked or split-screen: session is resizeable, letterboxing recomputes, taps still map correctly.

**16. Long-run / hygiene**

- Force-stop app mid-stream: desktop auto-releases privacy within TTL, encoder sessions stop (no server-side zombie capture), siren/WoL relays unaffected.

**17. Foldable / tablet dual-pane (R4 E6)**

- On a foldable folded to half-open (vertical hinge): session auto-splits — desktop video on one pane, trackpad + shortcut dock on the other; tapping/dragging the dock moves the desktop cursor and the finger never covers the video.

**23. Windows host (W1: core remote desktop)**

- Video is live and correct-sized on a 100 % and on a scaled (125/150 %) display; a still desktop keeps the HUD alive (the 1 s heartbeat) instead of tripping the 10 s dead-link card. *Run 1: live, correct-sized and kept alive on a still desktop at 100 %; no scaled display tried.*
- Taps land on the pixel tapped, on a single monitor and on a secondary monitor left of/above the primary (negative origin); drag selects text. *Run 1: taps on the primary land within 5 px with a second screen present, after the monitor → layout remap fix. Not tried: streaming the secondary, a monitor left of the primary, drag.*
- Phone keyboard: letters, digits, shifted capitals, arrows, F11/F12, Esc, Super (Start menu), right Alt, the app key; pasted text with capitals, accents and an emoji arrives intact. *Run 1: not testable — the app has no in-session keyboard (only the shortcut bar); Android Share sends text to the PC clipboard, not as typing.*
- Focusing an elevated window (e.g. an admin terminal) makes input stop with one `Windows refused injected input` log line, and input resumes on returning to a normal window.

**24. Desktop app and QR pairing**

- Leaving the scanner (after a scan, and with Back) turns the camera off: the privacy indicator goes out at once, not when the app closes.

**26. In-session keyboard**

- Swipe typing a word, and voice dictation.
- Double-tap locks a modifier (ringed cap); typing with Shift locked; Super latched + a key on the desktop.
- F-keys, Ins, PgUp/PgDn and Ctrl+Alt+Del from the bar.
- A Bluetooth or USB keyboard on the phone: letters, Ctrl shortcuts, arrows, F-keys.
- Tamil and Spanish keyboards: composing text arrives whole and correct.

**27. Trackpad gestures**

- Speed 0.5× and 3× feel right; slow movements still move the cursor.

**45. Phase R3 and R4: hostile connections, and idle cost**

- Turning off a phone's Power permission while it holds privacy mode (limit 6 in SECURITY.md): the grab ends at its 10 minute expiry.

**60. Pinch-zoom in trackpad mode (T9a)**

- Zoomed in, the cursor's travel matches the finger's on-screen distance, and scrolling is not slowed by the zoom.

**61. Portrait sessions (T9b)**

- Pane: two panes with Direct touch: taps land on the point under the finger in the smaller picture.

**62. A mouse on the phone (T9c)**

- The phone's pointer is hidden during the session and back over Home after Exit.
- Settings off: the phone's own pointer, no capture.

**63. Contact names in texts (T8e)**

- A text from a contact shows "Ann (number)" in the notification and in Messages; a stranger shows the number only; a sender ID with letters gets no name.

**71. Input on wlroots without uinput (T10f)**

- Super+N, and the text fallback typed as keys.
- Privacy mode where `/dev/input` is root-only too: the local keyboard is likely not blocked.

**98. An Android 17 pass (N4)**

- A touchpad connected to the phone drives the computer's pointer and scrolls during a session: a tap and a press of a clickpad click, two-finger scroll goes the right way at a sensible step, and a game controller's touchpad does nothing unwanted.

**104. Control the phone from the computer (C8)**

- Rotation: taps land right after the phone turns.

**122. The phone as a pen (E3)**

- A stylus (the A71 has none): hover moves the cursor without drawing, tilt and the side button reach the app.

**123. A gamepad on the phone (E4)**

- The on-screen gamepad from the A71 into a scratch window that logs keys: each button's key down and up once, the D-pad's arrows, the left stick moving the pointer, the right stick scrolling.

**126. The deck (F1)**

- Keys (`ctrl+shift+m`, `alt+tab`) into a scratch window here: pressed in order, all let go (input reaches this box: with the owner).

**129. An air mouse (F4)**

- Remote's "Air mouse" on the A71: held flat, turning moves the pointer across and tilting moves it up and down, a still hand moving nothing; the last choice kept.
- A tap clicks without moving the pointer; a press and turn moves without clicking; nothing moves after letting go.

**135. A laser pointer for the air mouse (G4)**

- Remote on the A71 with the air mouse on: "Laser pointer" under the pad; on, a press shows a red dot in the middle of the pointer's screen, turning moves it, the pointer stays put.
- A slide show (PowerPoint or a browser's): a tap with the laser on still advances or clicks at the pointer.

## A phone with a SIM

**11. WAN over iroh (R1)**

- Transport exposure (roadmap 2151-2158): every line from a session that lasted longer than the 5 s sampler must carry `lost_pk`/`lost_b`/`dgrams`/`tx_b`/`path_chg`/`relayed_s`, and `cong_ev`/`cwnd_pk`/`mtu`/`black` **on either transport**: quinn exposes them on the connection, and the iroh arm reads them off the *selected* path (`Connection::paths()`), so a WAN session riding a punched direct path reports the same set. A WAN line missing them is no longer the expected shape — the earlier "iroh drops the per-path values" claim was read off its connection-level aggregate, which really does discard them — and the only legitimate reason for a gap now is a snapshot taken with no selected path, which is why the field is `Option` and never a `0` pretending to be a measurement. `mtu` must be ≥ 1200 (quinn's `INITIAL_MTU`) and may climb as PLPMTUD probes pay off; `cwnd_pk` must be non-zero on any session that sent video. Walk between Wi-Fi bands, or kick the phone onto cellular mid-session, and `path_chg` has to count it; start a relayed WAN session and `relayed_s` must be non-zero with `outcome=wan_punched` if punching eventually won.

**18. Login autostart + tailnet access (server-side)**

- Phone on cellular (Wi-Fi off): same connect via tailnet IP works (Tailscale traverses NAT). Then iroh WAN fallback (`## 11`) with the two machines on genuinely different networks — still pending (phone cellular was OUT_OF_SERVICE at test time).

**35. Reaching the desktop from outside (N1)**

- **Cellular:** the phone on mobile data, the desktop at home. A session connects, and the HUD says direct or relayed. (Owed: the test phone has no mobile service.)

**36. Your own relay (N2)**

- **From outside:** the phone on mobile data, the relay on the desktop with port 3340 forwarded. A session connects through it.

**37. Found by id on the Mainline DHT (N3)**

- **From outside:** the phone on mobile data after the desktop's home relay or public IP changed. A session connects, found by the DHT.

**41. Texts (SMS) on the computer, and replies (N5b)**

- A request from a computer that is not the phone's own is ignored.
- A sixth reply within a minute fails with "the phone sends at most 5 replies a minute".
- **With a SIM:** a text that arrives while the computer is off reaches Messages once it is back.
- Take the computer off the network with the phone connected: within about a minute of it coming back, the phone has dialed again (keepalive).
- **With a SIM:** an incoming text appears in Messages and as a notification.
- **With a SIM:** a reply from Messages arrives on the other phone, and the tab says sent.
- A long reply (over 160 characters) arrives whole.
- "New text" to a new number.
- The phone's Texts permission off on the desktop: texts are not kept, and replies fail with the reason.

**44. Phase R1: end-to-end on Linux, across networks**

- Files both ways, phone notifications and texts on Linux, desktop audio heard on the phone.

**46. Phase R2: long sessions and recovery**

- The computer sleeping and waking (the user's to run), and the phone switching between Wi-Fi and mobile data (needs a SIM).

**48. Staying connected in the background (T2)**

- With the app allowed, phone notifications and texts keep arriving overnight with the screen off (the reason for all this).

**63. Contact names in texts (T8e)**

- A text queued while the computer is away, then names turned off: it goes without a name.
- Desktop "keep the contact names" off: new texts carry none, and after a server restart the kept names are gone.

**64. The control channel over the internet (T10a)**

- From outside the home network (mobile data, or another Wi-Fi), with Tailscale off: notifications, texts, battery and the phone's media reach the computer with the app in the background, and Send link reaches the phone.

**98. An Android 17 pass (N4)**

- Settings shows the Android 17 notes beside "Ring from the computer" and "Show texts on my computer".

**132. The computer's state on the home screen (G1)**

- "State widget refreshes by itself" off: no ask in the logcat at the next period; on mobile data with it on: none either.

## A second phone

**1. Connect + pairing gate (Tier 1 #5, Tier-2 #11b)**

- With an *unpaired* second phone: control features (clipboard, monitors) must stay locked out.

**24. Desktop app and QR pairing**

- One code, one phone: after a phone pairs, the Pair tab shows "Paired ✓" and no code, `pairing.pin` is gone, and "Pair another phone" shows a new code. Five wrong PINs typed on a phone make the tab say the code no longer works, and no new code appears until it is clicked. Leaving the tab, closing the window to the tray, and Quit each delete `pairing.pin`.

**43. Several phones at once (N7)**

- Two phones in sessions: the first to send input drives, and the other's taps do nothing while its session says "Another phone is driving".

**80. Named tasks (ecosystem 1c)**

- Linux (`sh -c`), a long output's end on the phone, and two phones at once.

**100. GNOME and KDE hosts that work unattended (N8)**

- Two phones at once, then one leaving: the other's picture and input go on.

**122. The phone as a pen (E3)**

- Control moving to another phone mid-stroke lifts the pen.

**126. The deck (F1)**

- Each refusal on the button: Input, Tasks, Audio or Links off for the phone; "Deck" off; another phone driving.

## Away from home: another network, or through a router

**35. Reaching the desktop from outside (N1)**

- A strict-NAT network (or a hotspot behind carrier NAT): "From outside" warns, and the session rides the relay.

**42. Wake from the phone (N6)**

- Away from home (another Wi-Fi): no broadcast is sent, only the outside packet; with no outside port, Wake says the phone is away and nothing is set up.

**65. The clipboard outside a session (T10b)**

- Away from home (the internet link): both directions.

**66. Answering the phone's notifications from the computer (T10c)**

- From away from home (the internet link).

**67. Files from the computer without a session (T10d)**

- Away from home: `pocketdesk send` refused at once as not reachable.

**72. New phone screenshots on the computer (T10h)**

- Away from home (the internet road), and the Linux host.

**76. Hotspot and tethering in Check connection**

- A computer joined to the phone's hotspot, saved under that address: "on this phone's hotspot or tethering".
- USB tethering (`rndis0`/`ncm0`), and the interface names on other makers' phones (Pixel's hotspot may be `wlan1`, not read).

**85. iroh 1.3, one endpoint per computer, files over the internet road (2a–2d)**

- 2b on a relayed path: A71 → the VM with direct UDP from home dropped, session and control link both up 10 minutes ("WAN · relayed").

## A second computer

**29. Computers on this network**

- Two computers on the network, both listed.

**42. Wake from the phone (N6)**

- "Through another computer": an always-on computer relays the wake.

**70. The phone's folders on the computer (T10g)**

- A second computer asking: nothing answered.

**105. Choose where received files land (C2)**

- The computer's chosen folder with a phone's share, a KDE device's file and another computer's; a folder that is gone; Settings' Choose folder… and Use Downloads.

**113. Clipboard history and Resend (D3)**

- A KDE Connect device's text, and another computer's, in the history.

**132. The computer's state on the home screen (G1)**

- Connect to another computer, then go Home: the widget follows it, asked once.

**134. The deck on the home screen (G3)**

- Connect to another computer, then press a button still drawn for the first: nothing is sent, the widget redrawn for the new one.

## A Linux desktop (the Azure VM serves for some)

**0. Prerequisites**

- Desktop: `cargo run -p pocketdesk-server` (or installed server) on the Hyprland machine; FFmpeg + PipeWire + xdg-desktop-portal present.
- Note which items are Hyprland-specific (window picker, workspace HUD, monitor geometry) — retest on a plain session if that's a target.

**1. Connect + pairing gate (Tier 1 #5, Tier-2 #11b)**

- "Show PIN on desktop" → desktop pops a notify-send PIN → phone confirms "Paired ✓", button flips to "Paired ✓".

**2. Session shell + video (Tier 1 #5, R2#2, R2#5)**

- R4 B1 screencopy backend (desktop-observable): starting a stream on Hyprland must NOT raise the portal screen-share grant dialog; server log shows "Capture started via backend Screencopy". If instead it shows "Capture backend Screencopy unavailable (…); trying next" followed by "Capture started via backend Portal", the reason is in the line — that's the fallback working, but note it as a finding. `POCKETDESK_SCREENCOPY=0` forces the portal path.
- R4 B3 capture_backend config override: set `capture_backend = "portal"` in config.toml, restart, start a stream → log shows "Capture started via backend Portal" and NO screencopy attempt line (pipeline pinned). With `capture_backend = "screencopy"` on a NON-wlroots/portal-only Wayland, the server must fail the session with a "Capture backend Screencopy unavailable" error rather than silently falling back to the portal (explicit intent surfaces). Omitting the key (or `"auto"`) restores the screencopy→portal fallback above. An unknown value (e.g. `"v4l2"`) makes the server refuse to start with a TOML parse error.
- R4 C2 startup ladder (desktop-observable): on a box where hardware encoding is broken (e.g. force `hardware_encoder = "nvenc"` here — this box's NVIDIA driver predates FFmpeg 9 NVENC, so the sidecar dies), start a session → server log shows "Hardware encoder unavailable (…); falling back to software (C2)" and video still appears within ~2 s on x264.
- R4 C2 mid-session encoder kill (HEVC session): during a live `codec=H.265` session, `pkill -f 'ffmpeg .*vaapi'` (or kill the encoder child seen in `pstree` of the server) → log shows "Rebuilt encoder on software rung (C2)", video resuming within ~2 s and the phone toasts "Desktop switched encoder — now H.264" (MediaCodec swap re-sniffed from the first software IDR).
- R4 C3 NVENC power pin (**NVIDIA hardware only — skip on non-NVIDIA boxes**): start an NVENC session (server log "Video encoder sidecar" on `h264_nvenc`/`hevc_nvenc`) → server log "pinned NVENC GPU 0 graphics clock to N MHz"; `nvidia-smi -q -d CLOCK -i 0` shows the graphics clock held at its max. End the session → log "restored NVENC GPU 0 to default clock management" and `nvidia-smi -q -d CLOCK` shows clocks back under adaptive management. On an unprivileged desktop the pin self-declines (log "clock lock refused ... running unpinned") and no reset is issued.

**2c. In-process VAAPI encode (R4 C4) — desktop/box-observable, no device needed**

- R4 C4 device-node selection: on the hybrid Intel+NVIDIA box the probe must land on the VA-capable node, NOT the hardcoded NVIDIA `renderD128` — `POCKETDESK_VAAPI_DEVICE=/dev/dri/renderD129` starts a session → server log "Video encoder: in-process VAAPI (h264_vaapi) on /dev/dri/renderD129" and **no `ffmpeg` sidecar child** is spawned for the vaapi path (`pstree` of the server). Point the override at a VA-less node (e.g. `/dev/dri/renderD128`) → it degrades to sidecar-then-software (C2), never hangs.
- R4 C4 quality (needs a phone): a VAAPI-encoded session decodes with correct color (no NV12 chroma shift) and the GOP (fps×2) keyframe lets first-frame render land within ~2 s.

**5. Window + monitor streaming (R3#7, Tier-2 #10)**

- Window picker lists Hyprland windows with title/class/workspace/size + "focused" marker.
- R4 B2 (Hyprland): pick a window → server log shows "Streaming compositor-cropped window frames"; partly-occluded window streams correctly (occluder NOT in the video), and only the window's pixels are encoded.
- R4 B2 (non-Hyprland wlroots, no export global): window pick still works via software crop exactly as pre-B2.

**6. Workspace HUD + battery (R3#8, Tier-2 #11a)**

- R4 E1: a row of window chips (title/class) shows under the workspace chips on a Hyprland session; tap one → video re-encodes cropped to that window (same as the picker), the tapped chip goes solid/highlighted.
- Roadmap 2066 (needs a second output or a hotplug-capable desktop): with the HUD open, unplug/replug a monitor → server log shows `Hyprland output changed, refreshing compositor state` within a second of the event (not up to 4 s) → HUD chips and window-crop geometry re-layout immediately. Never observed here: this box has one built-in output and Hyprland's socket1 write dispatchers are broken upstream, so no fake hotplug exists.

**8. Notifications + find-my-device (R3#11c, Tier-2 #11)**

- `notify-send -a Test "Hi" "body"` on desktop → phone notification appears (channel "Desktop notifications").

**9. Privacy mode + lock (Tier-3 #15, R4 E4)**

- Non-wlroots desktop (GNOME/KDE/X11, or `POCKETDESK_SCREENCOPY=0` headless): no shield is attempted — grab still works, no crash, no "shield engaged" log line.

**18. Login autostart + tailnet access (server-side)**

- `install.sh` run → unit lands in `~/.config/systemd/user/`, ExecStart points at the real prefix path, prompt enables it; `--status` reports the user unit; `--uninstall` removes it and any legacy `/etc/systemd/system` unit.

**23. Windows host (W1: core remote desktop)**

- The phone discovers the PC on the LAN (mDNS) and over the tailnet, pairs with the PIN, and the log shows `Auto-detect: selecting NVENC` (or AMF/MF on other GPUs) and `WGC capture started`. *Run 1: PIN pairing, NVENC and WGC confirmed over USB; LAN discovery and the tailnet not tried.*

**24. Desktop app and QR pairing**

- Linux: the app builds and opens on X11 and on Wayland (Hyprland), closing it quits (no tray on Linux yet), and the single-instance socket works.
- Linux tray: build one on a GTK thread (or tray-icon's ksni backend) and verify it with a StatusNotifier host, e.g. waybar's tray.

**26. In-session keyboard**

- Linux host: ASCII typing over uinput. Non-ASCII and capitals in pasted text wait for roadmap F4 (2666).

**29. Computers on this network**

- The Linux host is found (Avahi may also be announcing on the same host).

**31. Files both ways and the remote browser**

- Drag a file onto the desktop app's window; it is sent to the chosen phone (Windows, and X11 Linux).
- The Linux host: browse `/home/<user>`, a download, and a send from the app's path field.

**32. Switching screens**

- The Linux host (portal or screencopy capture per monitor).

**34. Desktop diagnostics (F10)**

- Linux: the checks with the systemd user service (no firewall line).

**37. Found by id on the Mainline DHT (N3)**

- The Linux host: the same checks.

**38. Image clipboard (N4)**

- The Linux host (`wl-copy` / `wl-paste` with `image/png`).

**39. The phone's notifications on the computer (N5a)**

- The Linux host (`notify-send`).

**40. Closed on the phone, closed on the computer (N5c, part 1)**

- The Linux host: a notification closed on the phone closes in the notification server (D-Bus `CloseNotification`).

**41. Texts (SMS) on the computer, and replies (N5b)**

- The Linux host.

**42. Wake from the phone (N6)**

- The Linux host (ethtool's Wake-on, and the adapter chosen).

**44. Phase R1: end-to-end on Linux, across networks**

- Input reaching a real desktop (a headless Sway has no input devices; uinput was created but nothing consumed it).
- Hyprland and GNOME/KDE (portal) hosts, and a GPU encoder.
- A WAN session's peer shows as `0.0.0.0:0` in the live-session list (cosmetic).

**45. Phase R3 and R4: hostile connections, and idle cost**

- Linux.

**46. Phase R2: long sessions and recovery**

- The same on the Linux host.

**47. Check connection (T1)**

- The Linux host (a stopped server there should be "nothing is listening").

**49. Protocol audit (what Pocketdesk sends, and to whom)**

- Linux: after `pocketdesk start`, `stat -c %a ~/.local/state/pocketdesk` is `700`, and the log says "Made … owner-only" once; a `POCKETDESK_STATE_DIR` the user cannot chmod logs a warning and the server still starts.

**50. Resumable transfers (T6a)**

- Android 8–9 (the legacy download file), and the Linux host.

**51. Folders (T6b)**

- Android 8–9, and the Linux host.

**52. Download a folder from the remote browser**

- The Linux host.

**53. The local API (T7)**

- The Linux host: `status`, `devices`, `send --wait`.

**58. The computer's media from the phone (T8c)**

- Linux: `playerctl` missing.

**64. The control channel over the internet (T10a)**

- A session riding a relay (no direct path) with the control link up at the same time. Founded: two endpoints with one key cannot share a relay (`server/tests/one_key_two_endpoints.rs`); since 2b both ride one endpoint. Owed on the VM, relayed (checklist §85).

**65. The clipboard outside a session (T10b)**

- Linux with an image on the clipboard, and a copy offering `x-kde-passwordManagerHint` (KeePassXC).

**68. Typing any character on Linux (F4)**

- Hyprland; a desktop with a non-US layout (`fr`), where text must still arrive as sent.
- GNOME or KDE (no virtual keyboard): capitals and `!@#` arrive through uinput with Shift.
- The in-session keyboard (IME text) as well as "Type phone clipboard".

**69. "Send to phone" in the file manager (T10e)**

- Nautilus, Nemo and Dolphin on Linux.

**71. Input on wlroots without uinput (T10f)**

- Hyprland, river, labwc and Wayfire.

**75. A second screen at the phone's shape (T10j)**

- "Phones may add a screen" off: refused with its reason; on Windows and Hyprland: "only Sway can for now".

**77. This phone's screen on the computer (T10k)**

- The Linux desktop app (Wayland and X11), and a computer without `ffmpeg` (the window says so).

**78. Events and hooks (ecosystem 1a)**

- Linux: `sh -c` hooks, and the log's mode (0600).

**81. Send from the browser (ecosystem 1d)**

- Linux registration (`~/.config/<browser>/NativeMessagingHosts`, `~/.mozilla/native-messaging-hosts`), images waiting in `~/.cache/pocketdesk-browser`, and the desktop Settings switch.

**82. Home Assistant over MQTT (ecosystem 1e)**

- The desktop Settings section on screen (the switch, the fields, the status line), a broker with a password, and Linux.

**84. The reliability pass before ecosystem phase 2 (R6)**

- The A71's reconnects on a slow link, reproduced: the VM with `tc qdisc add dev eth0 root tbf rate 1mbit burst 32kbit latency 400ms`, 10 minutes with no reconnect, then climbing back once it is removed (R6f).
- The idle cost on Linux after R6c (the VM), and with R6d's clipboard watch.
- R6d on the VM (Sway) with a phone linked: `pgrep -c wl-paste` stays at 1 over a minute, `wl-copy` text reaches the phone within 2 s, and the watcher ends when the phone unlinks.

**85. iroh 1.3, one endpoint per computer, files over the internet road (2a–2d)**

- A real WAN (the VM): 500 MB resumed with TCP 1716 unreachable.

**87. KDE Connect devices (3a–3c)**

- Linux, with `kdeconnectd` holding UDP 1716.

**88. The desktop app's shell (U1)**

- Narrator reading the app; the Linux build (Orca, the Noto fonts through fc-list).

**90. Names on both sides (U3)**

- The Linux host (its kernel host name), and a phone whose Android name holds a person's name.

**91. Ring the phone from the computer (N1)**

- The Linux host.

**96. Answer a phone notification in the toast itself (N3)**

- Linux: a notification server with actions (mako's `makoctl invoke`, dunst's menu) and KDE's inline reply.

**97. The phone as the computer's speaker and mic (N5)**

- Linux: the computer's sound through PipeWire and the phone's mic as "Pocketdesk Mic" (`pw-loopback`).

**99. The phone panel and a Linux tray (N6)**

- Send link… sends a typed link from the Overview, and Ring rings from there.
- The tray in a real panel: KDE Plasma, waybar or swaybar, GNOME with the AppIndicator extension. Closing hides the window to it, Open brings it back, Pair opens Pair, Quit quits.
- A panel restarted while the window is hidden: the item comes back, and starting the app again raises the window.
- Wayland (Sway, Hyprland, KDE's Wayland session): closing the window quits; the tray's entries work while it runs.

**100. GNOME and KDE hosts that work unattended (N8)**

- Without uinput access: the phone's taps, typing (accents, emoji), keys and the wheel (its direction) reach GNOME through the RemoteDesktop portal; a remote without the screen is told it gets no input there.
- Revoking in GNOME's settings brings the dialog back; Forget the grant and turning the setting off make the next session ask.
- The server started as a systemd user service: does the portal keep its restore data with no desktop app id?

**101. KDE Connect, next steps (N9)**

- Linux: the notification over D-Bus, and closed by its id.

**104. Control the phone from the computer (C8)**

- Linux.

**105. Choose where received files land (C2)**

- Linux.

**106. Notifications that wait and go later (C3)**

- Linux: the late line in a D-Bus notification.

**113. Clipboard history and Resend (D3)**

- Linux (`wl-paste`).

**116. The tray you act from (D5)**

- A phone named with `&` (Windows) or `_` (Linux): the name as written.
- Linux: the same menu in a real panel.

**117. The phone's folders as a drive (D8)**

- Linux: Files or Dolphin at the `dav://` address.

**118. Lock when the phone leaves (D9)**

- A real lock, on a computer where locking is allowed (not this box), Windows; on Linux, that it never locks.

**119. Memory at rest (D7)**

- Linux: the same by hand (`/proc/<pid>/status` VmRSS and RssAnon), the app in a Wayland session.

**121. The tray on its own (E2)**

- Linux: the tray program on a StatusNotifierItem host (or the WSL stand-in), and on a desktop with none (the window then quits on close).

**122. The phone as a pen (E3)**

- Linux: the uinput tablet (Krita, or `libinput debug-events` showing a tablet tool), and the mouse fallback without uinput.

**123. A gamepad on the phone (E4)**

- Linux.

**127. How the computer is doing (F2)**

- The same against Linux: `df`'s disks once each (btrfs subvolumes, bind mounts), the battery.

## Other hardware: a foldable, tablet, mouse, game pad, older or newer Android

**15. Language / i18n (Tier-3 #17 remainder)**

- Connect screen shows "Language" button on Android 13+ (absent on 12).

**17. Foldable / tablet dual-pane (R4 E6)**

- Tablet / unfolded (≥600 dp, no hinge): dual-pane auto-engages with a left|right split.
- "Pane: auto/dual/single" button cycles the override; forcing Single on a foldable restores the old full-screen layout; forcing Dual on a phone splits it.

**21. UI polish pass C1–C9 (design system, immersive session, error UX, settings)**

- Material You: on Android 12+ the palette follows the wallpaper when "Dynamic color" is on; turning it off falls back to the brand palette.

**27. Trackpad gestures**

- The foldable dock's trackpad behaves the same.

**29. Computers on this network**

- Android 17: the local-network permission prompt at first launch; LAN sessions and discovery work after Allow, and time out after Deny.
- Android 13 or older (the resolveService path).

**31. Files both ways and the remote browser**

- Android 8 or 9: the file lands in the app's own Downloads/Pocketdesk.

**33. Shortcuts outside the app (F11)**

- Android 13 or older: the tile opens the session (the older collapse path).

**38. Image clipboard (N4)**

- A 4K screenshot (over 10 MB): it moves, and the phone's memory holds.

**47. Check connection (T1)**

- Android 17's local network permission denied.

**59. The phone's media from the computer (T8d)**

- A real player (Spotify, a podcast app): its title, and the app's name (Android 11+ package visibility may give the package name).

**65. The clipboard outside a session (T10b)**

- Android 10–12: the tile's window reads the clipboard.

**72. New phone screenshots on the computer (T10h)**

- Android 14+'s "selected photos only" answer, and Android 12 and older.

**98. An Android 17 pass (N4)**

- A tablet-size window: the session lays out, rotates and resizes (orientation limits are ignored there).

**104. Control the phone from the computer (C8)**

- Android 13+ sideloaded: the restricted-settings step.
- Advanced Protection (Android 17): the service refused.

**105. Choose where received files land (C2)**

- A picked folder on an SD card or a cloud provider.

**110. Recent photos on the computer (C7)**

- Android 8 or 9: the legacy thumbnail path.

**111. Health after updates (D1)**

- A low-RAM phone on Android 10 or older: no "stopped" card.

**125. The phone app's design pass (E6)**

- The gamepad in portrait and landscape on the A71 and a 360 dp phone; at font scale 2.0.
- TalkBack on the new rows (the mode chip's action, the gamepad's keys, the folders' Remove).

**135. A laser pointer for the air mouse (G4)**

- Two screens: the dot crosses where they meet and stops at an edge, never between them; larger on a 4K screen.

**141. The home-screen widgets, a design pass (I2)**

- Android 11 or older: the pine theme in both modes, no previews, the state widget's layout by its height.

## By hand or eye on this box: hearing, looking, the tray, drag and drop, a screen reader, a real app

**2b. Compositor-true latency read (R4 E3)**

- Cross-check (eyes, optional): point the phone camera at the desktop with an on-screen clock/stopwatch — the phone's rendered image lags the physical panel by roughly the HUD `delay` figure (± the phone's own present latency, which the probe can't see).

**24. Desktop app and QR pairing**

- Scan the desktop app's code with the phone camera (the USB-tethered phone could not see the screen).
- Tray: Open, Pair a phone… and Quit from the tray menu itself.

**25. Desktop sound on the phone**

- Play a video on the desktop during a session: the phone plays its sound, and the lag behind the picture is small (≤ ~150 ms by ear).
- With sound on, the HUD's fps and delay match a session with it off, and taps on a still desktop feel immediate (the bridge lock fixes).

**27. Trackpad gestures**

- Two-finger tap opens a context menu; three-finger tap middle-clicks (e.g. opens a link in a new tab). adb cannot make multi-finger touches.
- Two fingers resting without moving do not right-click, and a two-finger tap does not scroll the page first.
- Tap, then drag a window by its title bar: it moves without maximising (no double click), and a tap left alone clicks about 0.3 s later.
- A drag survives a second finger landing mid-drag and releases on lift.

**28. Permissions and session approval**

- The desktop app's prompt: it comes to the front (also from the tray) and its three buttons work; Always allow turns the switch off.

**34. Desktop diagnostics (F10)**

- Open the Diagnostics tab in the app and click Run checks; Copy debug report pastes the same text.

**39. The phone's notifications on the computer (N5a)**

- A real app's public notification (a chat message) shows its title and text.

**47. Check connection (T1)**

- A changed security key (pin a wrong one), and "Scan code again" from the finding: the computer and its key removed, the scanner opened, and the new code trusted.

**52. Download a folder from the remote browser**

- The toast "Asked the computer for …" (the folder arrived, the toast was not caught on screen), and a third folder while two are on the way.

**54. Automation apps on the phone (T7b)**

- Tasker itself: the clipboard recipe, a file with "Grant read permission", Connect and Wake from a task.

**58. The computer's media from the phone (T8c)**

- A real player with a title: a browser video, Spotify; Next and Previous.

**60. Pinch-zoom in trackpad mode (T9a)**

- A two-finger tap is still a right click, a three-finger tap a middle click.

**64. The control channel over the internet (T10a)**

- The phone's battery over an hour with the link up in the background (iroh's 5 s heartbeat).

**66. Answering the phone's notifications from the computer (T10c)**

- The desktop app's Notifications tab itself (its buttons and reply box), and a real chat app (WhatsApp, Signal, Messages).

**71. Input on wlroots without uinput (T10f)**

- Direct touch (absolute moves), scrolling, right and middle clicks, and a two-output layout.

**77. This phone's screen on the computer (T10k)**

- The latency against a stopwatch, and over the internet road (WAN, relayed and direct).

**79. The Activity Center (ecosystem 1b)**

- The Activity tab itself on screen (its rows, filters, Show in folder): a click posted to the window did not reach egui, and real input into this box's desktop is not used for tests.

**82. Home Assistant over MQTT (ecosystem 1e)**

- Home Assistant itself (its Mosquitto add-on): the entities on its device page, the Activity entity's history, a button press from its UI.

**83. What the phone tells other apps (ecosystem 1f)**

- Tasker or MacroDroid receiving them (no automation app on the A71), a remote's events, a send's `FILES_SENT`, a task's `TASK_FINISHED`, a link lost (`REASON=lost`), and the Settings row on screen.

**87. KDE Connect devices (3a–3c)**

- The desktop app's Find button, list, Allow/Deny and Pair on screen.
- 3c, a text or link share refused with its notice, the low-battery warning from a KDE device, and the switches, battery and Ring on screen.

**89. The phone's navigation and feedback (U2)**

- TalkBack reading the snackbars and the checking card.

**90. Names on both sides (U3)**

- Rename on the desktop's Devices page, and the approval prompt's "(calls itself)" (they need clicks).

**91. Ring the phone from the computer (N1)**

- The desktop's Ring button on screen (it needs a click), Ring off in Devices refusing it, and once a minute from the button.

**92. Phone → computer text in one tap (N2)**

- "To computer" in a real app's selection menu, on screen; the Sharesheet's Direct Share row showing the computers (adb opens only the app chooser).

**93. The phone's Home card, Add form, Settings and session menu (U4)**

- TalkBack on a card ("Connect"), a Settings row (one stop) and a heading.

**97. The phone as the computer's speaker and mic (N5)**

- By ear: the computer's sound on the phone (lag, quality, across the room).
- Windows with VB-CABLE: the phone's mic heard in a recording or a call on the computer.

**104. Control the phone from the computer (C8)**

- The window by hand: a click, a drag, the wheel, the right and middle buttons, typing, the bar's buttons, Watch only, and a drag that leaves the window.

**106. Notifications that wait and go later (C3)**

- "Wait up to 10 minutes" on screen, and one posted more than 10 minutes before the computer comes back.

**107. The desktop app's headless harness (C4)**

- Clicks through the app's own window that a harness cannot stand in for: the tray, drag and drop, the cast window.

**108. The phone with large fonts, landscape and TalkBack (C5)**

- A TalkBack walk by hand: Home, the Add form, Settings, the session menu (TalkBack's tutorial takes the screen when it is turned on from adb, and adb cannot read which node it reads).

**110. Recent photos on the computer (C7)**

- Photo access denied: the same with its reason; and the page showing either (by eye).

**111. Health after updates (D1)**

- A real system update (the fingerprint path), and a listener that stays unbound after one.
- The card in Spanish and Tamil, and with TalkBack.

**114. Continue reading on phone (D6)**

- Firefox: the same, the extension loaded from `about:debugging`.

**115. Conversations (D4)**

- A real chat app on the A71: the thread on the Notifications page, oldest first, the owner's own lines set apart, a reply going through.

**116. The tray you act from (D5)**

- Ring, Mute for an hour and then Unmute, from the tray with the window hidden: the phone answers, the window stays hidden.
- A tray pick with the server stopped, and with a server from before D5: nothing sent, and the notice says only that it asked.

**117. The phone's folders as a drive (D8)**

- Explorer's repeated looks (`desktop.ini`, thumbnails) answered without a new listing each time (the phone's log).
- The setting off: the address gone from the Phone page (by eye).

**120. Answers that say how they went (E1)**

- The phone away for more than a minute, its notification still kept: "the phone did not connect within a minute". (Run 2026-10-19 with the listener off: the phone never sent the notification, so the answer was refused at once as no longer offered, before any wait.)

**121. The tray on its own (E2)**

- Windows: starting `pocketdesk-app` starts `pocketdesk-tray` once (a second window launch starts no second tray); closing the window leaves the tray.
- Each item from the tray program: Ring, Mute and Unmute, Send clipboard; Open, Pair a phone… and Send file… with the window closed (it starts on that page, Files with the phone chosen) and with it open (it comes to the front on that page).
- Quit from the tray program closes the window too; with no window open, an app started just after Quit stays open.
- An install without `pocketdesk-tray`: the app's own tray, as before.

**124. The desktop app's design pass (E5)**

- A screen reader (Narrator) names each photo's Save, Open and Copy, the pairing code and the phone's screen.
- The tray: a phone's title says Do Not Disturb or the mute's minutes left; Send the clipboard and Pair a phone.

**126. The deck (F1)**

- 2, 3 and 4 a row; a long label; Spanish and Tamil; TalkBack reads each button's label.

**128. Take a photo with the phone (F3)**

- Its tap: the camera permission prompt the first time, then the camera; the photo in the received files and on the computer's clipboard.
- The camera left with Back: nothing sent; the activity killed while the camera runs (Don't keep activities): the photo still sent.

**131. The phone's camera on the computer (F6, part 1)**

- "Show camera" on the A71: the camera permission asked once, then the back camera in the computer's cast window, landscape, live; the notification's Stop ends it and the camera's indicator goes.
- "Show its screen or camera here" off for the phone: refused, with its reason.
- Stop pressed while the camera is still opening: the camera's indicator goes.
- Another app taking the camera: "The camera stopped", the cast ended.

**133. Take a photo from the tray and the Overview (G2)**

- With the A71 linked and "Photos the computer asks for" on: the tray's Take a photo under it, then the phone's notification, the camera, and the photo in the received files; the same from the Overview's button, with its notice.
- The phone away, or Files off for it: Take a photo greyed in the tray and on the Overview, its hover saying why.

**140. Send the clipboard to the phone with a key (H3)**

- The tray's menu (H4): the phone it sends to shows "Send the clipboard (Win+Shift+C)"; with the key cleared, the plain label again.
