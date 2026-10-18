# Owed checks, by where they can run

Generated from `docs/device-verification-checklist.md` by `scripts/owed_checks.py` (C6); CI fails when it is stale. Each open item is placed by its own words and its section's, the first place that matches; a line can say `(where: <place>)` itself. Tick items in the checklist, then run the script again.

551 open items.

| Where | Items |
|---|---|
| Here: this Windows box and the A71, without touching the owner's power, lock or input | 320 |
| Power, lock or server stop on the owner's Windows box: never run here (the owner's rule) | 12 |
| Needs the owner's OK on this box: input reaching Windows, or its display settings | 22 |
| A phone with a SIM | 22 |
| A second phone | 5 |
| Away from home: another network, or through a router | 9 |
| A second computer | 4 |
| A Linux desktop (the Azure VM serves for some) | 82 |
| Other hardware: a foldable, tablet, mouse, game pad, older or newer Android | 25 |
| By hand or eye on this box: hearing, looking, the tray, drag and drop, a screen reader, a real app | 50 |

## Here: this Windows box and the A71, without touching the owner's power, lock or input

**0. Prerequisites**

- Desktop: `cargo run -p pocketdesk-server` (or installed server) on the
- Phone: `adb install -r android/app/build/outputs/apk/debug/app-debug.apk`
- Both on the same Wi-Fi LAN; phone can reach desktop port 1716 (TCP)
- Grant the app POST_NOTIFICATIONS when prompted (connect screen).

**1. Connect + pairing gate (Tier 1 #5, Tier-2 #11b)**

- Enter desktop IP + streaming port + control port → "Connect" opens a session.
- First connection: Pairing sheet auto-opens (pairing_required default true).
- Second connect to same host: no pairing prompt (persisted both sides).
- Manual flow: `pocketdesk pair` on desktop → type 6-digit PIN on phone → paired.
- Wrong/expired PIN → clear error, retry works.
- QUIC gate (v2 + video): before pairing completes, video must not flow — desktop log shows
- QUIC gate over WAN (iroh): unpaired dial is rejected the same way (desktop log + no frames).

**2. Session shell + video (Tier 1 #5, R2#2, R2#5)**

- Video appears within ~1 s; with "Show connection details" on, the HUD shows fps/kbps/rtt/delay/lost, plus the caption naming what
- FGS notification is present ("Streaming to <ip>"), survives home/swipe-away attempts; its Disconnect action ends the session.
- Screen-off for 1 min, wake → stream still live (wake lock + keepalive; plan #13).
- Status chip: kill the server → "Down: <reason>" chip + Retry rebuilds after restarting the server.
- R4 B1 screencopy backend (desktop-observable): starting a stream on
- R4 B3 capture_backend config override: set `capture_backend = "portal"` in
- R4 B1 idle pacing + freshness: keep the desktop static → `src` fps drops to ≤10
- R4 B1 multi-monitor: pick monitor N in the picker → log line "Screencopy capture: output at
- R4 C1 codec negotiation, default off: connect normally → server log shows
- R4 C1 HEVC + window crop: pick a window during a HEVC session → encoder rebuild at
- R4 C2 startup ladder (desktop-observable): on a box where hardware encoding is broken
- R4 C2 mid-session encoder kill (HEVC session): during a live `codec=H.265` session,
- R4 C2 mid-session stall, same-codec: force the supervisor on an H.264 session (kill its
- R4 C2 bottom rung exhausted: with software already live, keep killing encoders until the
- R4 C2 crop stability: during a software-fallback session, pick/clear a window crop →

**2a. Desktop-side session visibility + kick (R4 D2)**

- Consent notice: first phone connect → desktop shows a "Pocketdesk: a device is
- `pocketdesk status` during the session → tailscale output plus a "Streaming sessions
- `pocketdesk kick <id-prefix>` (≥6 chars, or full id, or peer IP) → phone shows the Down
- `pocketdesk kick all` ends every live session at once.
- kick with no running server → "no running Pocketdesk server to kick through", nothing
- Stale request safety: with no daemon running, hand-write `$XDG_STATE_HOME/pocketdesk/kick`
- Phone disconnect (normal Exit) → status line goes away within ~2 s; no leftover session in

**2b. Compositor-true latency read (R4 E3)**

- Sanity: on-LAN idle desktop → HUD `delay` (the e2e estimate) should sit in the low tens of ms and track the
- The old bug-shape check: `delay` must NOT equal `rtt` while frames are under load (it used
- WAN session: `delay` should ≈ rtt/2 higher than a LAN session on the same desktop motion

**2d. Rate and link basis (roadmap 2056)**

- Right after connect: the caption starts at something like `rates over 0.5 s · rtt not measured yet` and
- Basis reset: end the session and reconnect to the same (or a different) desktop → the caption must
- Portrait legibility: the caption is one line of 10 sp monospace; in portrait on a narrow phone it
- Optional cross-check: while the caption reads `rates over 3.0 s`, `pocketdesk sessions` for the same

**3. Input paths (R2#4, Tier 1 #2)**

- Mode toggle switches behavior live, no session restart.
- holdKey/repeat (e.g. hold an arrow key) behaves.
- R4 D1 view-only: tap "View-only: off" on the session bar → label flips

**4. Zoom + display mapping (Tier 1 #6, R3#7)**

- Pinch-zoom in direct-touch mode pans/clamps correctly; taps still land on the visible desktop point while zoomed.
- Trackpad mode: two-finger still scrolls remotely (no zoom capture).

**5. Window + monitor streaming (R3#7, Tier-2 #10)**

- Moving the window mid-session: crop follows only after re-pick (known limitation — crop is static rect).
- "Whole desktop" clears the crop.
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

- Reply still works end-to-end after the `kdeconnect.notification-reply` →
- `llVersion` tag is present on our extension packets only: capture the control-channel line

**9. Privacy mode + lock (Tier-3 #15, R4 E4)**

- Leave session with privacy on → release happens (phone app exit); also verify 10-min TTL auto-release by force-killing the phone app mid-grab.
- Shield tears down with the grab: "Privacy: off" → the red frame disappears immediately; force-killing the phone app mid-grab → frame gone after the TTL auto-release.
- "Lock PC" / session-notification "Lock desktop" → screen locks (loginctl path).
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
- Relayed → direct upgrade while streaming: badge flips to "WAN · direct" on its own (iroh keeps punching) and the bridge logs "upgraded from relay to direct path" (logcat). Relayed session must stay usable the whole time, not stall on the transition.
- Relayed session (R4 A3): within ~2 s of the badge reading "relayed", the desktop log shows "Encoder bitrate target changed" (relay_cap engaged) at effective ≤ 2 Mbit/s followed by "Encoder rebuilt for new bitrate", and picture is visibly softer than LAN/direct.
- Full-quality override (R4 A3): while relaying, the badge area offers "Full quality: off" — tap → log shows "Relay quality override changed" then a fresh "Encoder bitrate target changed" back to configured (`pocketdesk sessions` kbps over the next session line). Tap again to re-clamp; toggle survives a stream retry (re-arms).
- Clamp release on punch-through: start relayed (override off), wait for the direct upgrade → within ~2 s the log shows "Encoder bitrate target changed" back to configured without touching anything.
- LAN session (R4 A3): the relay clamp never engages (`stats().relayed` is always false on quinn → relay_cap is unbounded); the arbiter task still runs but emits no bitrate-change line until a preset is chosen below or the link starts losing packets.
- Loss response (roadmap 2053): degrade the link — `tc qdisc add dev <iface> root netem loss 5%` on the desktop, or walk the phone toward the far edge of the Wi-Fi mid-session — and within a few 2 s ticks the desktop log shows "Encoder bitrate target changed" carrying a `loss_cap=` field with effective below configured, then "Encoder rebuilt for new bitrate" and a visibly softer picture. The cut is 20 % per congested tick down to a 1 Mbit/s floor (a configured rate below the floor is never raised to it). Restore the link → the same line reappears **without** `loss_cap=` as the rate climbs back ~10 % per tick to configured, then stops being a term at all.
- Loss response must not fire on a quiet desktop: hold the screen still (no window movement, no video) for a minute and the log must show no bitrate-change line. Below 100 datagrams in a tick the controller treats the sample as no information, which is the only thing distinguishing "the link is lossy" from "there was nothing to lose" — a bitrate cut on an idle LAN session is a bug, not a measurement.

**11b. HUD link-profile presets (R4 E5)**

- Session bar shows "Quality: auto" by default. Tap it once → "Quality: max" (blue), then "balanced", "economy", back to "auto" — the button cycles all four.
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

- Screenshot during blackout → blocked (FLAG_SECURE); Recents thumbnail blank/secure.
- Double-tap or system back unlocks; back never exits the session while blacked out.
- Entering PiP while blacked-out auto-clears the overlay.
- Pocket test: with overlay on, phone in pocket should not send spurious input (touch consumed by overlay).

**15. Language / i18n (Tier-3 #17 remainder)**

- Switch system per-app language to Español / தமிழ் → all screens, sheets, FGS notification text localize (telemetry units, key labels, window titles stay as-is by policy).
- Mid-session language change (if the OS applies live) → no crash; format strings (`%1$s`) render correctly in both locales.

**16. Long-run / hygiene**

- 1 h continuous session: memory stable (no decoder leak across resolution changes), fps steady, no watchdog kills.
- Force-stop app mid-stream: desktop auto-releases privacy within TTL, encoder sessions stop (no server-side zombie capture), siren/WoL relays unaffected.
- Airplane-mode mid-session then back: clean Down chip + retry, no ANR.

**17. Foldable / tablet dual-pane (R4 E6)**

- Rotate to a horizontal hinge → split flips to top/bottom (stream top, dock bottom) and vice-versa.
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

- **PASS condition:** zero `Incoming v1 TCP connection from` lines (`server/src/service.rs:330`) after
- **PASS condition:** zero `Broken pipe (os error 32)` WARN lines from any plugin.
- **PASS condition:** zero `Kicked 1 stale session(s) due to reconnect storm` from the server.
- Battery/clipboard/monitors/window rows still show *current* values — the shared connection must not
- The no-session fallback still works: a query issued with **no** live control session must take the

**23. Windows host (W1: core remote desktop)**

- `install` registers the "Pocketdesk" task (Task Scheduler → Task Scheduler Library), removes a
- A toast "Pocketdesk pairing PIN: …" appears for `pair` (the `a_toast_is_shown` test only proves
- The phone discovers the PC on the LAN (mDNS) and over the tailnet, pairs with the PIN, and the
- Video is live and correct-sized on a 100 % and on a scaled (125/150 %) display; a still desktop
- Phone keyboard: letters, digits, shifted capitals, arrows, F11/F12, Esc, Super (Start menu),
- Focusing an elevated window (e.g. an admin terminal) makes input stop with one `Windows refused
- Window crop: picking a window streams just that window; closing it returns to the monitor.
- A UAC prompt or Win+L mid-session: the stream recovers after it is dismissed (WGC keeps running; on
- `pocketdesk stop` from a second terminal stops the server cleanly (the log ends with
- `uninstall` removes the task and the firewall rules (`wf.msc`), including the "Pocketdesk" ones an

**24. Desktop app and QR pairing**

- Pair over the LAN address in the code, not `adb reverse` (needs the firewall rules `install` adds).
- Settings → Save on a real `config.toml` keeps comments; the server picks the change up after Restart.
- Open a `pocketdesk://pair` link, finish or dismiss it, then switch dark mode or reopen the app from Recents: the pairing is not offered again. Switching theme while the confirmation is showing keeps it.

**25. Desktop sound on the phone**

- Quick Settings → "Desktop sound on this phone" off: silent at once; on again: sound returns within a second. The setting survives an app restart.
- Pause the desktop's audio for 10 s, then resume: it comes back cleanly, without a crackle burst.
- Connecting to a silent desktop does not pause music already playing on the phone; the first desktop sound takes over. "Silent" means no app holds an audio stream open: Windows loopback sends packets of silence while any app has one (seen on the A71 with a browser open: focus was taken 1.3 s after connect), and focus follows packets, not loudness.
- A phone call silences it, and it resumes after the call. Another app starting music takes over for good: switching "Desktop sound on this phone" off and on brings it back.
- In PiP the sound continues; Exit stops it.
- `adb logcat -s DesktopAudio`: "desktop sound started", and on exit "stopped (N underruns)" with N small.

**26. In-session keyboard**

- Swipe typing a word, and voice dictation.
- Double-tap locks a modifier (ringed cap); typing with Shift locked; Super latched + a key on the desktop.
- Landscape: the stream stays visible above the keyboard, and zoom survives opening and closing it.
- Tamil and Spanish keyboards: composing text arrives whole and correct.

**27. Trackpad gestures**

- Two-finger scroll is natural (content follows the fingers, like the phone itself) at about one wheel notch per 24 dp, in both axes. Unverified: adb cannot make two-finger touches.

**28. Permissions and session approval**

- No answer: the phone shows the refusal about 30 s after connecting.
- Running commands: denied by default; allowed after the switch is turned on.
- Control off: the session is view-only and the phone cannot lift it. Sound off: no desktop sound. Microphone off: the mic does not reach the desktop.
- Turning control, sound or the microphone off during a session ends that session; Retry starts one with the change. Removing a connected phone ends its session.
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

- Turning off a phone's Power permission while it holds privacy mode (limit 6 in SECURITY.md): the grab ends at its 10 minute expiry.
- The phone's battery over an hour with the app in the background, and while a session is open.
- Network traffic while idle (both ends).

**46. Phase R2: long sessions and recovery**

- An hour with one session from start to end, to confirm the memory levels off.

**47. Check connection (T1)**

- A VPN that captures home traffic, and Tailscale off for a tailnet address.
- Guest Wi-Fi with AP isolation, and another subnet.
- Copy report on a device: no address or id in the text.

**48. Staying connected in the background (T2)**

- After allowing it in the battery settings, the card is gone on return.
- Battery use set to Restricted: the card says so, and "Allow" opens the app's page.
- Other makers (Xiaomi, OPPO): the guide link and their own battery managers.

**49. Protocol audit (what Pocketdesk sends, and to whom)**

- Android backup: `adb shell bmgr backupnow dev.pocketdesk.android` backs up nothing, and a device-to-device move (Smart Switch) leaves the app unpaired on the new phone.
- Windows: after `pocketdesk install`, the firewall has TCP 1716 and UDP 4716 rules only, and a phone on a fresh machine still lists the computer by mDNS (no UDP 5353 rule is made).
- A Wi-Fi with Tailscale up: a phone browsing mDNS sees the LAN address and no 100.x or fd7a: address.
- Transfers both ways still end "saved" over many runs (the clean close after the status byte), and a desktop share to a full disk fails at once instead of after 10 s.

**50. Resumable transfers (T6a)**

- A file from the remote browser broken and continued.
- "Retry failed files" off: a broken file fails at once, and no part is kept.
- "Unfinished files: not kept" on the desktop: a retry starts over.
- A file that changes between attempts starts over (the prefix hash).
- The app killed mid-download, then the offer fetched again within its 10 minutes.

**51. Folders (T6b)**

- A folder from the phone shows one "folder arriving" toast, not one per file.
- The phone shows "Not all of … was sent" when it leaves names out, and "Stopped sending …" when the computer goes away mid-folder.
- A folder broken by Wi-Fi mid-way, continued.
- A folder over the limits (10 000 files, 32 levels) says what was left out.
- Names Windows refuses on the phone (`a:b`) left out and counted.

**52. Download a folder from the remote browser**

- The toast: "Asked the computer for …", and a refusal's reason (Files off, a third folder while two are on the way).
- Closing the sheet before the answer, and the toast still showing.
- A folder of thousands of files: the answer arrives, and other queries keep working while the offers come.

**53. The local API (T7)**

- `send` of a folder with `--wait`, and a phone that leaves mid-send (exit 1).
- A broken config.toml with `--json` (exit 1, JSON).

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
- Unpairing the phone: its report gone from the file within 5 s.
- The phone's "Share battery with the computer" off: no report; the desktop's "Phones' batteries" off: no notification.
- A real discharge to 15 %, and Android's own low flag.

**58. The computer's media from the phone (T8c)**

- Nothing playing: "Nothing is playing".
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

**64. The control channel over the internet (T10a)**

- A phone with no iroh key registered (paired, never streamed on the LAN) is refused, with one log line, and works again after one session at home.
- Unpairing on the desktop ends the internet link's packets at once.
- A file offer to a phone on the internet link fails as "not reachable", not stuck.

**65. The clipboard outside a session (T10b)**

- Desktop "Clipboard to phones" off: nothing pushed; a session still gets the clipboard on request.
- A phone with Clipboard off on the desktop gets no pushes.

**66. Answering the phone's notifications from the computer (T10c)**

- "Hide sensitive content" on the lock screen turned on: the probe's notification arrives as "New notification" and offers nothing.
- The switch off: nothing offered, an answer not run.
- An action that opens an activity on Android 14+ (background launches may be blocked).

**67. Files from the computer without a session (T10d)**

- A phone that refuses background starts: the "The computer sends a file" notification, and Save downloading it.
- A folder sent this way.
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

**74. The phone's battery without a session**

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
- "Start my tasks" off: the phone gives up with "no answer" after 20 s.

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

**88. The desktop app's shell (U1)**

- A notice clearing after 6 s, "Copied.", and the two settings' switches while their change runs.
- A line mixing Arabic or Hebrew with Latin (egui has no bidirectional layout), read by someone who reads it.

**90. Names on both sides (U3)**

- A computer's name from a scanned pairing code, the session notification and launcher shortcuts by name, and "Lock <name>?".

**91. Ring the phone from the computer (N1)**

- Do Not Disturb with alarms allowed and not; a phone with a secure lock screen (the full-screen ring and Stop); a phone where the full-screen permission is not granted.

**92. Phone → computer text in one tap (N2)**

- A copy marked sensitive (a password manager's) asking first through the tile.

**93. The phone's Home card, Add form, Settings and session menu (U4)**

- "Show connection details" on, during a session: the numbers and the computer's battery.
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

**102. The phone's background cost (N7)**

- Over Wi-Fi rather than `adb reverse`: the radio's wakes and bytes for the link's keepalives.
- A release build (R8): the CPU again.

**103. Transfers you can watch and stop (C1)**

- A folder: one Cancel stops the whole folder, each way.
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
- The computer's Notifications permission off: nothing waits on the phone.

**108. The phone with large fonts, landscape and TalkBack (C5)**

- Split screen: Home and a session in half a screen.
- Landscape with the keyboard open (§26).
- Spanish and Tamil at 1.0, 1.3 and 2.0 (a long word such as "Individual" may already make a list at 1.0).

## Power, lock or server stop on the owner's Windows box: never run here (the owner's rule)

**2. Session shell + video (Tier 1 #5, R2#2, R2#5)**

- R4 C1 forced HEVC round-trip: `allow_hevc = true` in config.toml, restart server,

**2d. Rate and link basis (roadmap 2056)**

- The stall check this replaces: freeze the desktop's output (sleep the display, or `SIGSTOP` the

**7. Clipboard + files (Tier 1 #4)**

- Reboot phone → history + saved hosts survive.

**11. WAN over iroh (R1)**

- Desktop reconnects (restart server) → identity re-announced within 30 s, phone re-dials.

**18. Login autostart + tailnet access (server-side)**

- Reboot the desktop → after reaching the Hyprland session, `systemctl --user status pocketdesk` is active without any manual start (WantedBy=graphical-session.target).

**24. Desktop app and QR pairing**

- Start / Restart server from the app, on Windows (logon task) and Linux (systemd user unit). This is not run on the developer box by rule.

**42. Wake from the phone (N6)**

- **Sleep, then wake from the phone at home** (Ethernet): the computer wakes, and the card connects once it answers.
- **From outside:** forward a UDP port on the router to the computer, set it as "Outside wake port", sleep the computer, and wake it from a phone on mobile data. Note how long after sleep the router still delivers it.
- Hibernate and shutdown (Windows fast startup off), and Wi-Fi only.

**49. Protocol audit (what Pocketdesk sends, and to whom)**

- Wake through a relay computer with Power off for the phone: the phone says the relay did not answer and asks about Power, not "Wake sent". With a target off the relay's networks: "The relay computer refused: not on this computer's network".
- The desktop app's Devices tab reads "Lock, sleep, privacy mode and waking others".

**89. The phone's navigation and feedback (U2)**

- Lock confirmed, and refused with Power off (the server's answer is unit-tested): never against this box, so on the VM or a spare computer.

## Needs the owner's OK on this box: input reaching Windows, or its display settings

**3. Input paths (R2#4, Tier 1 #2)**

- Drag (roadmap 2061, shipped 2026-09-24, device-unverified): start a slow drag-select across desktop text — `evtest` should show the pointer warp to the touched point, `BTN_LEFT 1`, moving `ABS_X/ABS_Y` while held, `BTN_LEFT 0` on lift, and the selection must follow the finger (before this change a drag was motion-with-button-up: nothing began, and the release landed on a button that was never pressed). Then the leak check: begin a one-finger drag and add a second finger mid-drag (pinch) — the button must still come up, and the desktop must not be left dragging. Note there is no server-side safety net for a lost release packet yet (roadmap 2272): a session that dies mid-drag leaves `BTN_LEFT` held on the uinput device until the next release.
- Trackpad mode: pointer moves without jumping to touch point; tap = click; two-finger = scroll.
- Keyboard: type letters/digits/modifiers via on-screen shortcuts + remote input — check against `evtest`: Super, Alt+Tab, Ctrl+Alt+Del, PrtSc, Esc, Super+1..9 work (ShortcutBar). (Esc tap verified 2026-09-22; this box binds workspace cycling to Super+Tab, so the bar's Alt+Tab is config-dependent, not a pipeline failure.)
- The same keyboard list **run twice, once per injection backend** (roadmap 2057-2060). Under

**4. Zoom + display mapping (Tier 1 #6, R3#7)**

- Letterbox: on a phone/desktop aspect mismatch, taps map through the letterbox offset (no systematic tap drift).

**5. Window + monitor streaming (R3#7, Tier-2 #10)**

- Picking a window: video re-encodes at window size (encoder rebuild), crop tracks the window, taps map to desktop coords via the screen box.

**6. Workspace HUD + battery (R3#8, Tier-2 #11a)**

- Tap a chip ≤9 → Super+N injected → workspace switches.

**8a. Control-channel versioning (R4 D4)**

- Forward-compat: hand-inject a `kdeconnect.pocketdesk.privacy` line with an extra unknown

**10a. Phone mic share (R4 E2)**

- View-only latched → mic keeps flowing (it is session media, not injected input); input is still dropped.

**11. WAN over iroh (R1)**

- Device-reported tails (roadmap 2143-2146, the phone's half of the chain): the same lines must carry `dec_*`, `rnd_*` and `e2e_*` plus `phone_rtt`/`phone_lost`. `dec_n` counts frames the decoder actually rendered, so it has to track `enc_n` within the loss the link imposed (a `dec_n` far below `enc_n` is dropped-or-stalled frames, not a telemetry bug), `dec_p50` must be single-digit ms on a hardware rung and grow when the desktop is cropped to a bigger window, and `e2e_p50` must sit near the HUD's e2e figure for the same session. `rnd_p50` is the rendered-frame interval, so it should be near 1000/fps ms and must jump when the bitrate clamp or a preset slows the link. On a session that never delivered video (rejected, or killed before the first frame) `dec_*`/`rnd_*` are legitimately absent — that is the "nobody measured" case, and `key=0` in their place would be a bug. **The server side of this path is already proven without a phone: `cargo test -p pocketdesk-server --test session_record -- --ignored --nocapture` drives a real capture→encode→send→receive session over loopback and asserts the record carries all of these tails; what remains device-only is that `H264Decoder.kt` feeds `dec_*`/`rnd_*` with real MediaCodec timings rather than the numbers the test injects.**

**11b. HUD link-profile presets (R4 E5)**

- View-only interaction: while a preset is active, toggling View-only still drops injected input but the bitrate preset keeps applying (both are control-plane).

**13. PiP + multitask (Tier-3 #14, DeX)**

- DeX/docked or split-screen: session is resizeable, letterboxing recomputes, taps still map correctly.

**17. Foldable / tablet dual-pane (R4 E6)**

- On a foldable folded to half-open (vertical hinge): session auto-splits — desktop video on one pane, trackpad + shortcut dock on the other; tapping/dragging the dock moves the desktop cursor and the finger never covers the video.

**23. Windows host (W1: core remote desktop)**

- Taps land on the pixel tapped, on a single monitor and on a secondary monitor left of/above the

**26. In-session keyboard**

- F-keys, Ins, PgUp/PgDn and Ctrl+Alt+Del from the bar.

**27. Trackpad gestures**

- Speed 0.5× and 3× feel right; slow movements still move the cursor.

**60. Pinch-zoom in trackpad mode (T9a)**

- Zoomed in, the cursor's travel matches the finger's on-screen distance, and scrolling is not slowed by the zoom.

**62. A mouse on the phone (T9c)**

- The phone's pointer is hidden during the session and back over Home after Exit.
- Settings off: the phone's own pointer, no capture.

**71. Input on wlroots without uinput (T10f)**

- Super+N, and the text fallback typed as keys.

**98. An Android 17 pass (N4)**

- A touchpad connected to the phone drives the computer's pointer and scrolls during a session: a tap and a press of a clickpad click, two-finger scroll goes the right way at a sensible step, and a game controller's touchpad does nothing unwanted.

**104. Control the phone from the computer (C8)**

- Rotation: taps land right after the phone turns.

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

- A text from a contact shows "Ann (number)" in the notification and in Messages; a stranger shows the number only; a sender ID with letters gets no name.
- A text queued while the computer is away, then names turned off: it goes without a name.
- Desktop "keep the contact names" off: new texts carry none, and after a server restart the kept names are gone.

**64. The control channel over the internet (T10a)**

- From outside the home network (mobile data, or another Wi-Fi), with Tailscale off: notifications, texts, battery and the phone's media reach the computer with the app in the background, and Send link reaches the phone.

**98. An Android 17 pass (N4)**

- Settings shows the Android 17 notes beside "Ring from the computer" and "Show texts on my computer".

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

## A Linux desktop (the Azure VM serves for some)

**0. Prerequisites**

- Desktop: user in `input` group (uinput + `/dev/input/event*` read for
- Note which items are Hyprland-specific (window picker, workspace HUD,

**1. Connect + pairing gate (Tier 1 #5, Tier-2 #11b)**

- "Show PIN on desktop" → desktop pops a notify-send PIN → phone confirms "Paired ✓", button flips to "Paired ✓".

**2. Session shell + video (Tier 1 #5, R2#2, R2#5)**

- R4 C3 NVENC power pin (**NVIDIA hardware only — skip on non-NVIDIA boxes**): start an NVENC session (server log "Video encoder sidecar" on `h264_nvenc`/`hevc_nvenc`) → server log "pinned NVENC GPU 0 graphics clock to N MHz"; `nvidia-smi -q -d CLOCK -i 0` shows the graphics clock held at its max. End the session → log "restored NVENC GPU 0 to default clock management" and `nvidia-smi -q -d CLOCK` shows clocks back under adaptive management. On an unprivileged desktop the pin self-declines (log "clock lock refused ... running unpinned") and no reset is issued.

**2c. In-process VAAPI encode (R4 C4) — desktop/box-observable, no device needed**

- R4 C4 host proof (no phone): `cargo test -p pocketdesk-core --lib encoder_vaapi` →
- R4 C4 device-node selection: on the hybrid Intel+NVIDIA box the probe must land on the
- R4 C4 quality (needs a phone): a VAAPI-encoded session decodes with correct color (no NV12 chroma

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

- Compositor shield (R4 E4, Hyprland/wlroots): the same "Privacy: on" also paints a full-perimeter red frame on every monitor within ~1 s; the desktop centre stays visible and interactive from the phone (the ring is `KeyboardInteractivity::None`, click-through). Desktop log shows "privacy: layer-shell privacy shield engaged".
- Non-wlroots desktop (GNOME/KDE/X11, or `POCKETDESK_SCREENCOPY=0` headless): no shield is attempted — grab still works, no crash, no "shield engaged" log line.

**18. Login autostart + tailnet access (server-side)**

- `install.sh` run → unit lands in `~/.config/systemd/user/`, ExecStart points at the real prefix path, prompt enables it; `--status` reports the user unit; `--uninstall` removes it and any legacy `/etc/systemd/system` unit.

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
- Privacy mode where `/dev/input` is root-only too: the local keyboard is likely not blocked.

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

- Cancel after the device's Accept unpairs it on the phone; a second request from the phone while one waits shows nothing new.
- 3c, a text or link share refused with its notice, the low-battery warning from a KDE device, and the switches, battery and Ring on screen.
- A device that broadcasts first (this computer dialing it): not built (the receive half waits).
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

- "Remember the screen-sharing grant" on: a session from the phone asks once at the desk; a second session and a reconnect start with no dialog.
- Without uinput access: the phone's taps, typing (accents, emoji), keys and the wheel (its direction) reach GNOME through the RemoteDesktop portal; a remote without the screen is told it gets no input there.
- Revoking in GNOME's settings brings the dialog back; Forget the grant and turning the setting off make the next session ask.
- No RemoteDesktop portal (or refused): the screen alone.
- The server started as a systemd user service: does the portal keep its restore data with no desktop app id?

**101. KDE Connect, next steps (N9)**

- A chat app's notification, an update to one (replacing the toast), and a flood held to the limit.
- Linux: the notification over D-Bus, and closed by its id.

**104. Control the phone from the computer (C8)**

- Linux.

**105. Choose where received files land (C2)**

- Linux.

**106. Notifications that wait and go later (C3)**

- Linux: the late line in a D-Bus notification.

## Other hardware: a foldable, tablet, mouse, game pad, older or newer Android

**9. Privacy mode + lock (Tier-3 #15, R4 E4)**

- "Privacy: on" → physical keyboard/mouse on desktop stop responding (`evtest`: grabs held), while phone input still flows.

**14. Blackout / pocket mode (Tier-3 #15 remainder)**

- "Blackout" → black screen, "double-tap to unlock" hint, screen dimmed; stream continues (watch desktop from another device or check fps in logcat).

**15. Language / i18n (Tier-3 #17 remainder)**

- Connect screen shows "Language" button on Android 13+ (absent on 12).

**17. Foldable / tablet dual-pane (R4 E6)**

- Tablet / unfolded (≥600 dp, no hinge): dual-pane auto-engages with a left|right split.
- "Pane: auto/dual/single" button cycles the override; forcing Single on a foldable restores the old full-screen layout; forcing Dual on a phone splits it.

**21. UI polish pass C1–C9 (design system, immersive session, error UX, settings)**

- Material You: on Android 12+ the palette follows the wallpaper when "Dynamic color" is on; turning it off falls back to the brand palette.

**26. In-session keyboard**

- A Bluetooth or USB keyboard on the phone: letters, Ctrl shortcuts, arrows, F-keys.

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

**62. A mouse on the phone (T9c)**

- Motion, all five buttons and the wheel (both ways) reach the desktop, and a fast mouse keeps its travel.
- Opening and closing the in-session keyboard: the mouse keeps working.
- The mouse's Back button goes to the desktop and does not end the session.

**65. The clipboard outside a session (T10b)**

- Android 10–12: the tile's window reads the clipboard.

**72. New phone screenshots on the computer (T10h)**

- Android 14+'s "selected photos only" answer, and Android 12 and older.

**98. An Android 17 pass (N4)**

- A tablet-size window: the session lays out, rotates and resizes (orientation limits are ignored there).

**104. Control the phone from the computer (C8)**

- The window by hand: a click, a drag, the wheel, the right and middle buttons, typing, the bar's buttons, Watch only, and a drag that leaves the window.
- Android 13+ sideloaded: the restricted-settings step.
- Advanced Protection (Android 17): the service refused.

**105. Choose where received files land (C2)**

- A picked folder on an SD card or a cloud provider.

## By hand or eye on this box: hearing, looking, the tray, drag and drop, a screen reader, a real app

**2. Session shell + video (Tier 1 #5, R2#2, R2#5)**

- Drop phone far from AP / saturate link → frames stall, then recover; the HUD's `lost` rises but no permanent freeze (gap-driven keyframe request, R2#5).

**2b. Compositor-true latency read (R4 E3)**

- Cross-check (eyes, optional): point the phone camera at the desktop with an on-screen

**3. Input paths (R2#4, Tier 1 #2)**

- ...and a tap is actually a **click** (roadmap 2061, re-verify 2026-09-24). The 2026-09-22 note above

**5. Window + monitor streaming (R3#7, Tier-2 #10)**

- R4 B2: close the streamed window mid-session → stream keeps running on full-output frames with software crop (warn log "did not stage"; no session drop).

**11. WAN over iroh (R1)**

- Tail latency (roadmap 2141-2146): the same lines carry `enc_n`/`enc_p50`/`enc_p90`/`enc_p95`/`enc_p99`/`enc_max` and the matching `rtt_*` set. `enc_n` counts frames the encoder actually produced a packet for, so it must be non-zero and of the same order as fps × session duration — a large shortfall is its own finding (frames dropped upstream, or a path that never samples). `enc_p50` must sit near the typical frame time for the encoder rung in use (the log line names the backend at session start), and a deliberate stall — crop to a window, or unplug/replug Wi-Fi mid-session — has to move `enc_max` or `rtt_max` above `enc_p50`/`rtt_p50`. **A line with no `enc_*` at all means the sampling was never wired to the live pipeline: that is a bug, not a healthy session.**

**11b. HUD link-profile presets (R4 E5)**

- On LAN, pick "economy": desktop log shows "Link-profile preset changed" then "Encoder bitrate target changed" with effective ≤ 1.5 Mbit/s and "Encoder rebuilt for new bitrate"; the HUD bitrate/`pocketdesk sessions` kbps drop and the picture softens. Native resolution is NOT reduced (only bitrate) — verify the HUD reports the same width×height.

**17. Foldable / tablet dual-pane (R4 E6)**

- Toggling panes mid-session does NOT drop the stream (the decode session survives the fold — check fps continuity / no re-Keyframe gap).

**24. Desktop app and QR pairing**

- Scan the desktop app's code with the phone camera (the USB-tethered phone could not see the screen).
- Tray: Open, Pair a phone… and Quit from the tray menu itself.
- Leaving the scanner (after a scan, and with Back) turns the camera off: the privacy indicator goes out at once, not when the app closes.

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
- Clipboard off: the phone's clipboard actions are dropped (`Denied … clipboard` in the log); turn it on again and they work at once.

**34. Desktop diagnostics (F10)**

- Open the Diagnostics tab in the app and click Run checks; Copy debug report pastes the same text.

**39. The phone's notifications on the computer (N5a)**

- A real app's public notification (a chat message) shows its title and text.

**46. Phase R2: long sessions and recovery**

- A file transfer cut by a drop (today it fails and is sent again whole).

**47. Check connection (T1)**

- A changed security key (pin a wrong one), and "Scan code again" from the finding: the computer and its key removed, the scanner opened, and the new code trusted.
- A UDP-only block (the computer's firewall dropping the stream port).

**51. Folders (T6b)**

- The desktop app's "Choose folder…" and dropping a folder on the window.

**54. Automation apps on the phone (T7b)**

- Tasker itself: the clipboard recipe, a file with "Grant read permission", Connect and Wake from a task.

**56. Links both ways (T8a)**

- The phone's "Links from the computer" off: a link dropped.

**57. The phone's battery on the computer (T8b)**

- Catching up after a Wi-Fi drop.

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

**74. The phone's battery without a session**

- With only folders, answers or files keeping the link; with none on, nothing sent and no connection made; the link dropping just before a report (nothing dialed).

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

**89. The phone's navigation and feedback (U2)**

- Ring's snackbar (it rings the computer), and a shortcut key's failure message on a dropped link.
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

**102. The phone's background cost (N7)**

- The same hour on battery, without USB: the battery's drop, and the CPU while the phone really sleeps.

**106. Notifications that wait and go later (C3)**

- Settings "When the computer is out of reach": each choice on screen, and Don't wait dropping a notification posted while the computer is away.

**107. The desktop app's headless harness (C4)**

- Clicks through the app's own window that a harness cannot stand in for: the tray, drag and drop, the cast window.

**108. The phone with large fonts, landscape and TalkBack (C5)**

- A TalkBack walk by hand: Home, the Add form, Settings, the session menu (TalkBack's tutorial takes the screen when it is turned on from adb, and adb cannot read which node it reads).
