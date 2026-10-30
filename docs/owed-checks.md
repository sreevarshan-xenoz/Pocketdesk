# Owed checks, by where they can run

Generated from `docs/device-verification-checklist.md` by `scripts/owed_checks.py` (C6); CI fails when it is stale. Each open item is placed by its own words and its section's, the first place that matches; a line can say `(where: <place>)` itself. Tick items in the checklist, then run the script again.

1023 open items.

| Where | Items |
|---|---|
| Here: this Windows box and the test phone (the OPPO), without touching the owner's power, lock or input | 36 |
| Power, lock or server stop on the owner's Windows box: never run here (the owner's rule) | 16 |
| Needs the owner's OK on this box: input reaching Windows, or its display settings | 156 |
| A phone with a SIM | 39 |
| A second phone | 15 |
| Away from home: another network, or through a router | 39 |
| A second computer | 110 |
| A Linux desktop (the Azure VM serves for some) | 178 |
| Other hardware: a foldable, tablet, mouse, game pad, older or newer Android | 63 |
| By hand or eye on this box: hearing, looking, the tray, drag and drop, a screen reader, a real app | 371 |

## Here: this Windows box and the test phone (the OPPO), without touching the owner's power, lock or input

**1. Connect + pairing gate (Tier 1 #5, Tier-2 #11b)**

- Enter desktop IP + streaming port + control port → "Connect" opens a session.

**2b. Compositor-true latency read (R4 E3)**

- Sanity: on-LAN idle desktop → HUD `delay` (the e2e estimate) should sit in the low tens of ms and track the `rtt` chip sensibly (e2e ≈ capture age + rtt/2); heavy desktop motion → `delay` rises (encoder/queue age grows), never reads 0 once video is flowing. (2026-10-26, the runner's `au2-hud-delay` on the OPPO by the internet road, an idle desktop: delay 16 to 19 ms against an rtt of 6 to 7 ms in four runs, never equal to it. On an idle desktop the frames are the capture's repeats, so this is their encode and queue, not the compositor's age. The LAN and heavy motion still owed: no LAN session runs through adb, and motion would need input on this computer.)
- The old bug-shape check: `delay` must NOT equal `rtt` while frames are under load (it used to be a literal RTT proxy).
- WAN session: `delay` should ≈ rtt/2 higher than a LAN session on the same desktop motion (the network-leg term is RTT/2 by construction).

**2d. Rate and link basis (roadmap 2056)**

- Right after connect: the caption starts at something like `rates over 0.5 s · rtt not measured yet` and reaches `rates over 3.0 s · rtt median of 8` within ~8 s; fps/kbps are dim until the caption says ~2.5 s or more, and `rtt` is dim until its first poll (~1 s). A number that arrives already bright means the basis is not being reported. (2026-10-24, the OPPO over the internet road, the caption only: `rates over 1.0 s · rtt not measured yet` at 3.5 s, `rates over 2.6 s · rtt median of 6` at 18.5 s. The first poll came ~14 s in, as the connect itself took that long by the internet road; the 8 s and the dimming still owed on the LAN.)
- Optional cross-check: while the caption reads `rates over 3.0 s`, `pocketdesk sessions` for the same session should report a mean kbps within the same order of magnitude — the session record averages over the whole session, the HUD does not, and a large disagreement is a measurement bug in one of them.

**7. Clipboard + files (Tier 1 #4)**

- The same from the phone's side, and with the sync turned off and on: it needs the phone's Copies by themselves (Pocketdesk control).

**11. WAN over iroh (R1)**

- `dec_*` grows when the desktop is cropped to a bigger window, and `rnd_p50` jumps when the bitrate clamp or a preset slows the link.

**16. Long-run / hygiene**

- The same across resolution changes (no decoder leak), fps steady, no watchdog kills. (2026-10-25, the runner's `ae2-crops`: twelve changes of size in one session (1395x786 and 2560x1440, six rounds) with no redial, the app's memory level 175.9 → 186.3 MB, at most 186.7; three runs. The frame rate was not read: the HUD's numbers need Show connection details on the phone.)

**17. Foldable / tablet dual-pane (R4 E6)**

- Rotate to a horizontal hinge → split flips to top/bottom (stream top, dock bottom) and vice-versa.
- Toggling panes mid-session does NOT drop the stream (the decode session survives the fold — check fps continuity / no re-Keyframe gap).
- Shortcut bar appears once — in the dock while dual, under the stream while single.
- Blackout still covers BOTH panes (whole window), not just the stream.
- es/ta: "Pane:" and trackpad hint localize.

**21. UI polish pass C1–C9 (design system, immersive session, error UX, settings)**

- Theme follows the system light/dark; toggling the mode in Settings restyles every screen immediately (no restart), and the window background matches on cold start (no white flash in dark mode or vice-versa). *Partly verified: the Dark radio restyles Settings immediately; cold-start window colour not yet checked.* *2026-10-25: the system's dark mode turned on and off restyled Home at once, the app's process running throughout (the runner's `ag3-theme`, Home's background 251 light, 20 dark). The cold-start window colour is still to see.*
- Edge-to-edge on all screens: content draws behind the status/nav bars and no control is unreachable under them.
- Pairing sheet: progress bar while waiting, PIN field auto-focused, eye toggle honors the reveal-PIN pref, animated check on success then auto-dismiss.
- Home empty state (no saved computers) shows the desktop glyph + explainer.

**22. Owed verification register**

- The no-session fallback still works: a query issued with **no** live control session must take the `oneshot_request` branch (`android/bridge/src/api.rs`, reached from `control_request` when `CONTROL_WRITER` is `None`) and still get an answer — that is the path that runs before you connect, and it is the one the rewrite could plausibly have broken. Same for `push_packet`'s dedicated socket.

**31. Files both ways and the remote browser**

- A download refused by the desktop (an offer used twice) fails at once with its reason, not after 30 s.
- A download from the phone cut halfway (Wi-Fi off).

**38. Image clipboard (N4)**

- With the phone's Clipboard permission off, neither direction moves. (2026-10-25, the computer-to-phone half: the runner's `ad1-image-copy` with the permission off on a throwaway server: the image not told to the OPPO, not fetched. The phone-to-computer half is owed: the OPPO cannot copy an image by script.)

**49. Protocol audit (what Pocketdesk sends, and to whom)**

- Transfers both ways still end "saved" over many runs (the clean close after the status byte), and a desktop share to a full disk fails at once instead of after 10 s.

**74. The phone's battery without a session**

- With only folders, answers or files keeping the link; with none on, nothing sent and no connection made; the link dropping just before a report (nothing dialed).

**78. Events and hooks (ecosystem 1a)**

- The 33rd waiting hook skipped, on a real server: 37 events of a kind within 30 s (a phone sends at most 20 notifications a minute, so another source).

**84. The reliability pass before ecosystem phase 2 (R6)**

- A download that waits for the network (next phase, R6i).

**100. GNOME and KDE hosts that work unattended (N8)**

- "Remember the screen-sharing grant" on: a session from the phone asks once at the desk; a second session and a reconnect start with no dialog.

**109. The owed checks by where they run (C6)**

- The rest of "here": what the S cycle (S1 to S5, 2026-10-23 and 24) did not reach, mostly long transfer and battery runs.

**120. Answers that say how they went (E1)**

- A desktop notification for such a failure.

**125. The phone app's design pass (E6)**

- The Health card's Choose opens the folder picker. (2026-10-25: it shows only when a chosen downloads folder has lost its grant; the OPPO has none chosen, and choosing one is the folder picker, by hand. The card's Fix for Pocketdesk control did open Android's Accessibility settings.)

**128. Take a photo with the phone (F3)**

- "Also onto its clipboard" off: the file only; "Photos the computer asks for" off (the default): no notification, and the Phone page's button disabled while the phone is not linked.
- Spanish and Tamil.

**136. The computer's music on the home screen (G5)**

- Nothing playing: "Nothing is playing", no buttons; Audio off for the phone on the computer: the refusal, no old track.

**149. Files keep their dates (K1)**

- A file downloaded from the remote file browser, and a folder each way. (2026-10-25, to the phone: the runner's `ad3-dates`, a file and a folder's file dated 2020 kept their dates on the OPPO. A folder to the computer is owed: its picker is by hand.)

**162. The phone's state on the computer (L9)**

- The phone away: the line is gone.

**230. Look and feel (Desktop UX 3)**

- On a phone's Devices entry, a group switch turned off ends that phone's running session (when it holds Control or sound), and turned on leaves Run commands off. (where: OPPO)

## Power, lock or server stop on the owner's Windows box: never run here (the owner's rule)

**2. Session shell + video (Tier 1 #5, R2#2, R2#5)**

- R4 C1 forced HEVC round-trip: `allow_hevc = true` in config.toml, restart server, reconnect → log shows `codec=H.265 (HEVC)`; phone decodes (sniffed `video/hevc`) with video within ~1 s and input still mapped. A pre-C1 phone build (no caps stream) must still get H.264. (The log part seen 2026-10-24 by `x3-codec`, the session up 8 s later; the decode not checked.)

**9. Privacy mode + lock (Tier-3 #15, R4 E4)**

- "Lock PC" / session-notification "Lock desktop" → screen locks (loginctl path).

**11. WAN over iroh (R1)**

- Desktop reconnects (restart server) → identity re-announced within 30 s, phone re-dials.

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
- The desktop app's Devices tab reads "Lock, sleep, privacy and wake others".
- Windows: after `pocketdesk install`, the firewall has TCP 1716-1725, UDP 4716 and TCP 1739-1764 rules only, and a phone on a fresh machine still lists the computer by mDNS (no UDP 5353 rule is made).

**89. The phone's navigation and feedback (U2)**

- Lock confirmed, and refused with Power off (the server's answer is unit-tested): never against this box, so on the VM or a spare computer.

**166. Linked with the screen off (M1)**

- After a reboot, the "Keeps the link" notification is back without opening the app.

## Needs the owner's OK on this box: input reaching Windows, or its display settings

**2. Session shell + video (Tier 1 #5, R2#2, R2#5)**

- Video appears within ~1 s of Connect on a LAN session. (where: owner) A throwaway server's LAN port needs a firewall rule on this box; through `adb reverse` the phone waits out the LAN dial first, and its first picture came 11–14 s after Connect by the internet road.

**3. Input paths (R2#4, Tier 1 #2)**

- Trackpad mode: pointer moves without jumping to touch point; tap = click; two-finger = scroll. (where: owner)
- Mode toggle switches behavior live, no session restart. (where: owner)
- holdKey/repeat (e.g. hold an arrow key) behaves. (where: owner)
- R4 D1 view-only: with the "View-only" switch on in the session menu, EVERY remote input goes dead on the desktop (taps, shortcuts, workspace chips) while video, the HUD, clipboard sync and the screen and window pickers keep working; switched off, input is restored without a reconnect; on across a screen switch, input stays blocked through the rebuild. (where: owner)

**4. Zoom + display mapping (Tier 1 #6, R3#7)**

- Pinch-zoom in direct-touch mode pans/clamps correctly; taps still land on the visible desktop point while zoomed. (where: owner)
- Trackpad mode: two-finger still scrolls remotely (no zoom capture). (where: owner)
- Letterbox: on a phone/desktop aspect mismatch, taps map through the letterbox offset (no systematic tap drift). (where: owner)

**8a. Control-channel versioning (R4 D4)**

- `llVersion` tag is present on our extension packets only: capture the control-channel line (e.g. `journalctl`/strace the JSON, or a debug log of `to_wire` output) — a `kdeconnect.pocketdesk.privacy`/`.audio`/`.state` packet carries `"llVersion":1`; a native `kdeconnect.identity`/`kdeconnect.pair`/`kdeconnect.clipboard` packet does NOT.
- Forward-compat: hand-inject a `kdeconnect.pocketdesk.privacy` line with an extra unknown top-level field and an unknown body key → server still parses and acts (unknown fields ignored).

**11. WAN over iroh (R1)**

- On same LAN: identity cached (logcat), status shows LAN. (where: owner)
- After both a LAN and a WAN session: `pocketdesk sessions` shows one line per session with distinct `outcome=` (`lan_direct` vs `wan_punched`/`wan_relayed`), plausible `rtt_ms`/`kbps`, and `dev=` = the paired deviceId. (where: owner)
- LAN session (R4 A3): the relay clamp never engages (`stats().relayed` is always false on quinn → relay_cap is unbounded); the arbiter task still runs but emits no bitrate-change line until a preset is chosen below or the link starts losing packets. (where: owner)

**11b. HUD link-profile presets (R4 E5)**

- View-only interaction: while a preset is active, toggling View-only still drops injected input but the bitrate preset keeps applying (both are control-plane).

**17. Foldable / tablet dual-pane (R4 E6)**

- On a foldable folded to half-open (vertical hinge): session auto-splits — desktop video on one pane, trackpad + shortcut dock on the other; tapping/dragging the dock moves the desktop cursor and the finger never covers the video.
- Direct-touch taps on the video pane still hit the right desktop point (letterbox/aspect-fit recompute for the half-width pane). (where: owner)

**23. Windows host (W1: core remote desktop)**

- `install` registers the "Pocketdesk" task (Task Scheduler → Task Scheduler Library), removes a "Pocketdesk" task an older version left, starts the server with no console window, and the log file appears under `%LOCALAPPDATA%\pocketdesk\logs`. (where: owner) (it registers a logon task and firewall rules on this computer.)
- Video is live and correct-sized on a 100 % and on a scaled (125/150 %) display; a still desktop keeps the HUD alive (the 1 s heartbeat) instead of tripping the 10 s dead-link card. *Run 1: live, correct-sized and kept alive on a still desktop at 100 %; no scaled display tried.*
- Taps land on the pixel tapped, on a single monitor and on a secondary monitor left of/above the primary (negative origin); drag selects text. *Run 1: taps on the primary land within 5 px with a second screen present, after the monitor → layout remap fix. Not tried: streaming the secondary, a monitor left of the primary, drag.*
- Phone keyboard: letters, digits, shifted capitals, arrows, F11/F12, Esc, Super (Start menu), right Alt, the app key; pasted text with capitals, accents and an emoji arrives intact. *Run 1: not testable — the app has no in-session keyboard (only the shortcut bar); Android Share sends text to the PC clipboard, not as typing.*
- Focusing an elevated window (e.g. an admin terminal) makes input stop with one `Windows refused injected input` log line, and input resumes on returning to a normal window.
- Window crop: picking a window streams just that window; closing it returns to the monitor. (where: owner) (closing a window on this computer's desktop.)

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

**28. Permissions and session approval**

- LAN session: the prompt shows the phone's address. (where: owner) (a throwaway server's LAN port needs a firewall rule.)

**29. Computers on this network**

- Pair from a found computer: after the PIN it appears under Saved. (where: owner) (a throwaway server's LAN port needs a firewall rule.)
- "Visible on this network" in the desktop app, then a server restart: the computer no longer appears. (where: owner) (a throwaway server's LAN port needs a firewall rule.)

**32. Switching screens**

- Unplug the monitor being streamed; the session falls back to the primary screen. (where: owner)

**34. Desktop diagnostics (F10)**

- A network Windows files as Public, with only `install`'s rules: the firewall check says no rule lets phones in. (where: owner)
- A Block rule for pocketdesk: the firewall check reports it. (where: owner)

**45. Phase R3 and R4: hostile connections, and idle cost**

- Turning off a phone's Power permission while it holds privacy mode (limit 6 in SECURITY.md): the grab ends at its 10 minute expiry.

**47. Check connection (T1)**

- A UDP-only block (the computer's firewall dropping the stream port). (where: owner)

**54. Automation apps on the phone (T7b)**

- A share naming one of two saved computers goes to that one (the A71 has one saved). (where: owner) (the other saved computer is the owner's own.)

**55. A slow or lossy link (fix/slow-link-watchdog)**

- A healthy LAN: frame rate and e2e unchanged (the in-flight bound must not throttle it). (where: owner) (a throwaway server's LAN port needs a firewall rule.)

**60. Pinch-zoom in trackpad mode (T9a)**

- Zoomed in, the cursor's travel matches the finger's on-screen distance, and scrolling is not slowed by the zoom.

**61. Portrait sessions (T9b)**

- Pane: two panes with Direct touch: taps land on the point under the finger in the smaller picture.

**62. A mouse on the phone (T9c)**

- The phone's pointer is hidden during the session and back over Home after Exit.
- Settings off: the phone's own pointer, no capture.

**63. Contact names in texts (T8e)**

- A text from a contact shows "Ann (number)" in the notification and in Messages; a stranger shows the number only; a sender ID with letters gets no name.

**64. The control channel over the internet (T10a)**

- A phone with no iroh key registered (paired, never streamed on the LAN) is refused, with one log line, and works again after one session at home. (where: owner) (a session at home needs the throwaway's LAN port open.)

**69. "Send to phone" in the file manager (T10e)**

- The desktop app's Settings switch, and `install` / `uninstall` adding and removing the entry. (where: owner) (`install` changes this computer.)

**71. Input on wlroots without uinput (T10f)**

- Super+N, and the text fallback typed as keys.
- Privacy mode where `/dev/input` is root-only too: the local keyboard is likely not blocked.

**73. The phone as a remote (T10i)**

- Over the internet road, and a phone driving while another has a picture session (driving rules). (where: owner) (a remote needs the Input permission.)
- I1: the computer's server restarted under a remote: "Reconnecting in N s…" on its card, then the remote back by itself, the talk timer still counting; Kick from the computer: not retried; Settings "Reconnect by itself" off: Retry only; after ten minutes, "Stopped trying" and the session's notification gone. (where: owner) (a remote needs the Input permission.)

**89. The phone's navigation and feedback (U2)**

- Ring's snackbar (it rings the computer), and a shortcut key's failure message on a dropped link. (where: owner) (it sounds a siren on this computer.)

**95. The phone app in Spanish and Tamil (U6)**

- The remote's presenter keys in Spanish. (where: owner) (A remote needs the Input permission.)

**98. An Android 17 pass (N4)**

- A touchpad connected to the phone drives the computer's pointer and scrolls during a session: a tap and a press of a clickpad click, two-finger scroll goes the right way at a sensible step, and a game controller's touchpad does nothing unwanted.

**104. Control the phone from the computer (C8)**

- Over the home network (not the relay): latency again. (where: owner)
- Rotation: taps land right after the phone turns.

**111. Health after updates (D1)**

- Advanced Protection on, with control on (needs the owner's Google account). (where: owner)

**122. The phone as a pen (E3)**

- Windows: a pen session from the A71 into that window: pressure following the touch, Down and Up once per stroke. (where: owner) (pen input on this computer.)
- A drawing app with pressure (Krita or Paint) on Windows: strokes thin and thick with the touch. (where: owner) (pen input on this computer.)
- A stylus (the A71 has none): hover moves the cursor without drawing, tilt and the side button reach the app.
- Pen pressure Soft, Even and Firm change the ink for one touch. (where: owner) (pen input on this computer.)
- A pinch while in Pen lifts the pen and zooms the view, and the stroke does not go on after it. (where: owner) (pen input on this computer.)

**123. A gamepad on the phone (E4)**

- The on-screen gamepad from the A71 into a scratch window that logs keys: each button's key down and up once, the D-pad's arrows, the left stick moving the pointer, the right stick scrolling.
- Closing it (× or Back) with a button held lets the button go. (where: owner) (game pad input on this computer.)
- A game (one that takes Enter, the arrows and Space) by hand. (where: owner)
- Two buttons at once, and a stick held while pressing a button. (where: owner) (game pad input on this computer.)

**125. The phone app's design pass (E6)**

- The session menu with Pen offered: three segments at 1.0; at 2.0 crowded. (where: owner) (Pen is offered only where the computer takes pen input.)

**126. The deck (F1)**

- Keys (`ctrl+shift+m`, `alt+tab`) into a scratch window here: pressed in order, all let go (input reaches this box: with the owner).

**129. An air mouse (F4)**

- Remote's "Air mouse" on the A71: held flat, turning moves the pointer across and tilting moves it up and down, a still hand moving nothing; the last choice kept.
- A tap clicks without moving the pointer; a press and turn moves without clicking; nothing moves after letting go.
- "Air mouse speed" at 0.5 and 3; a slow link (moves add up, no replay). (where: owner) (pointer input on this computer.)

**135. A laser pointer for the air mouse (G4)**

- Remote on the A71 with the air mouse on: "Laser pointer" under the pad; on, a press shows a red dot in the middle of the pointer's screen, turning moves it, the pointer stays put.
- A slide show (PowerPoint or a browser's): a tap with the laser on still advances or clicks at the pointer.

**140. Send the clipboard to the phone with a key (H3)**

- Settings "Send the clipboard with a key" = `win+shift+c`, Save: "Registered." under it; with the A71 linked, copy a text on the computer and press it: the text on the phone. (where: owner) (a key pressed on this computer.)
- A combination another app holds: "Another app holds this combination"; freed, registered within 30 s. (where: owner)
- A password manager's copy: not sent (the server's notification says why). (where: owner) (a key pressed on this computer.)

**142. The desktop app, upgrades and UI tweaks (I3)**

- Settings: change a key by hand in `config.toml` with the app open, then change another in the form and Save: the hand edit stays. Ctrl+S saves.
- Keys: Esc on Unpair, Restart and Clear; Ctrl+Page Up and Down; Enter in the Files and Notifications boxes.

**144. Find a setting on the desktop (J2)**

- Ctrl+F with Settings scrolled down: the box in view with the cursor in it. Esc empties it, and a Restart question open at the same time stays open.

**146. Search on Activity and Clipboard (J4)**

- Ctrl+F on each page, scrolled down; both boxes at the minimum width, light and dark.

**151. The phone's volume keys for the computer (K4)**

- Remote open: the volume keys and the panel's slider move the computer's volume, one step a press; a held key stops when let go. (where: owner) (it sets this computer's volume.)
- The Sound sheet moves the volume, then a key press: one step from there, no jump. (where: owner) (it sets this computer's volume.)
- The screen off: the keys still the computer's. A call: the keys are the call's. (where: owner) (it sets this computer's volume.)
- A sound session: the keys stay the phone's. The setting off: the keys stay the phone's. (where: owner)
- `dumpsys media_session`: the session remote, and which session takes the keys on the OPPO and the A71. (where: owner) (a remote needs the Input permission.)

**152. Pause the phones on the computer (K2a)**

- While paused: a link not opened, and a phone ringing this computer silent. (where: owner)

**154. The phone's music in the computer's media controls (K7)**

- The keyboard's media keys (input on this box: with the owner). (where: owner)

**157. Drop files and paste into the phone's window (L2)**

- With control on: Ctrl+V into a scratch field, read back with `uiautomator dump`; a long paste says it was cut.

**168. Image copies without a session (M3)**

- A screenshot copied on Windows (Win+Shift+S) with a phone linked: it goes once, even while the Snipping Tool holds the clipboard. (where: owner) (keys pressed on this computer.)
- A copy marked private on Windows (a password manager): neither pushed nor answered to a session's poll. (where: owner)

**170. No toast while the phone is in hand (M6)**

- With Pocketdesk's Remote open on the phone: a toast. (where: owner) (a remote needs the Input permission.)

**171. Type on the phone from the computer's keyboard (M4)**

- Japanese through Windows' input method: the committed text arrives. (where: owner)

**173. tel: links and Call back (M8)**

- Settings' switch on Windows: Default apps offers Pocketdesk for TEL; a link clicked in a browser opens the dialer with no console window. (where: owner) (it registers a handler on this computer.)
- Off again: the keys are gone and Windows asks for another app. (where: owner)

**180. Files into the phone's folders from the computer's drive (N6)**

- A file's write into the marked folder on the LAN road (or the internet road), its SHA-256 read back through the drive. (where: owner) (the LAN road needs the throwaway's port open.)

**182. Leftovers: typing from the tray and a key, several links, the phone nearly full (N9)**

- "Type on the phone with a key": registered, a press opening the window; another app holding it said in Settings. (where: owner) (a key pressed on this computer.)
- A share with three links sent to "Open on computer": three open on the computer, and the toast says three. (where: owner) (it opens links in this computer's browser.)

**185. A screenshot of the phone and its screen locked from the computer (O2)**

- The screenshot's arrival on the LAN road (over `adb reverse` the computer will not dial back to 127.0.0.1). (where: owner) (the LAN road needs the throwaway's port open.)

**193. The computer's announcement follows its network (P1)**

- A wake from sleep with a new lease. (where: owner)

**195. Quiet hours, and quiet while presenting (P3)**

- Windows' own state: a PowerPoint slideshow, a game in full screen, and a video in a browser's full screen. (where: owner)

**199. Files copied in Explorer sent with the clipboard key (P7)**

- Files off and text copied too: the text sent instead. (where: owner) (the clipboard key on this computer.)

**214. Driving another computer from Windows (shared input)**

- Windows to Linux at the edge: the pointer off the Windows laptop's right edge lands on the Linux screen at the same height, the Windows cursor hidden; back off the Linux screen's left edge, it returns where it crossed and shows again. (where: owner)
- Typing in a terminal on Linux while away: letters, Shift and capitals, Ctrl+C, the arrows, AltGr, and nothing typed on Windows meanwhile. (where: owner)
- The switch key (Ctrl+Alt+Right) goes over and back, from the edge's side and from the middle of a screen; no Ctrl or Alt left down on either side. (where: owner)
- Win+L while away: the keyboard and mouse come home and then Windows locks; no "l" is typed on Windows or on Linux, the Super key is not left down on Linux; after unlocking, the edge works again. Linux gets a lone Super press and release (GNOME's Activities or KDE's launcher may open there; note whether it does), and holding L past the key-repeat delay can type "l" on Windows before the lock shows. (where: owner)
- The network unplugged while typing on Linux: the Windows cursor comes home and shows within about a second, its keys typing on Windows again; Linux lets go of the held key within about a second. Plugged back in, the pointer crosses onto Linux again once the link is back. (where: owner)
- The server killed while away (Task Manager): the Windows cursor shows again when the server starts; Linux lets go of everything within about a second. (where: owner)
- The server stopped while away (`pocketdesk stop`, then again by an uninstall): the Windows cursor shows again at once, Linux lets go of a held key at once (it hears the leave, not a second's silence), and the server exits within a couple of seconds. (where: owner)
- Mixed scales: a monitor at 100% beside the laptop at 125%; the pointer crosses between Windows' own screens as before, and onto Linux from the laptop's edge at the height it left. (where: owner)
- The gaming lock: a full-screen game in front keeps the pointer on Windows at the edge (the log says so, and no hook is installed meanwhile); Alt+Tab out and within a second the edge crosses again; a game started while the pointer is on Linux brings it home; a game that scrolls at the screen's edge, brought to the front with the pointer there, does not cross. (where: owner)
- The switch key mid-game goes to Linux and back, the game getting its mouse again; a program on the always-stay list locks only while it is in front. (where: owner)
- With a kernel anti-cheat running (Vanguard, FACEIT, Easy Anti-Cheat or BattlEye), the switch key mid-game is refused and the log says why; an elevated program in front is refused with the `install --elevated` hint. (where: owner)
- The badge on Windows: crossing onto Linux flashes "Keyboard and mouse: <laptop>" in the middle of each Windows monitor for about a second, and coming back "Keyboard and mouse: <this computer>"; sharp at 100% and 125%; a click on it lands on the window under it, the window typed in keeps the focus, it is in neither the taskbar nor Alt+Tab, and two quick switches show one badge, not two. (where: owner)
- Nothing flashes over a full-screen game in front (the gaming lock's words go to the log only), and the game draws as before. (where: owner)
- Identify on the Keyboard and mouse page: this computer flashes "This is <name>" for about 3 seconds and so does each computer it can drive; a computer not allowed to drive another makes it flash nothing. (where: owner)

**215. The shared keyboard and mouse: the computer driven, the arrangement, the clipboard and the latency (shared input)**

- Copy text on Windows, push the pointer onto Linux, paste there: the text is there, with "Clipboard with my computers" off on both; copy on Linux, come back, paste on Windows. (where: owner)
- A password manager's private copy (KeePassXC, or Windows' marks) does not follow; with "Clipboard follows the mouse" off on either side (with Clipboard with my computers off), nothing crosses that way. (where: owner)
- Take a screenshot to the clipboard on Windows (Win+Shift+S), push the pointer onto Linux, paste into an image editor there: the picture is there, with "Clipboard with my computers" off on both; the server's log says "Clipboard image followed the mouse" with its size and no more. (where: owner)
- Copy an image on Linux (a picture from a browser), come back to Windows, paste into Paint: the picture is there. Cross again with nothing new copied: the log shows no second send of it. (where: owner)
- An image from a program on "Never send copies from these programs", or one KeePassXC marks private, does not follow; with "Files over the internet" off on either side, no image follows (text still does). (where: owner)
- A large image (a full-screen screenshot of a 4K screen) does not hold up the pointer as it crosses; push the pointer across and straight back before it lands: the image does not land late on the computer left, and nothing older comes back. (where: owner)
- The first key after a minute without touching either computer, the pointer resting on the Linux laptop, arrives at once: with "Keep the connection quick while sharing" on (BK10), the laptop sends Windows a byte every 50 ms while it is driven and keys or moves come, so its Wi-Fi does not doze, and after ten seconds without input the driver's own pings five times a second mostly keep it awake; Wi-Fi power saving itself is left on (§221). (where: owner)
- The arrangement page: drag the laptop to the left of the Windows screens, then below them; the pointer crosses where they now meet, within about two seconds; "Put them in a row" puts it back to the right. (where: owner)

**216. Files dragged across the edge (BK4)**

- Drag one file in Explorer across the edge onto Linux and let go: it lands in Linux's Downloads, "Sending 1 file to <laptop>" flashes on Windows, Linux shows its usual received notice, and the file is still in its Windows folder. This is the check of the design: that Windows' drag finds the catcher under the parked cursor and drops on it. (where: owner)
- Several files and a folder at once: all land, the folder as a folder with its files; the sent list shows each. (where: owner)
- Carry a drag across and back before letting go, then let go on a Windows folder: nothing is sent, and Windows copies or moves it there as usual. (where: owner)
- Esc while the drag is on Linux: the drag ends, nothing is sent, no Esc reaches Linux; let go afterwards and nothing is dropped anywhere. (where: owner)
- A window dragged by its title bar across and let go, and text dragged from a browser across: nothing is sent and nothing is dropped into the window under the parked cursor. (where: owner)
- A drag with the right button across: the files land on Linux too. (where: owner)
- Over 100 files at once (select 120 in a folder) dragged across: the first 100 land, "20 more left out: at most 100 go at once" flashes, and the drop is quick (only the first 100 paths are read). (where: owner)
- "Files over the internet" off on Windows: "Not sent" with the reason flashes and nothing goes; off on Linux, or Files not allowed there: the refusal's reason flashes within a few seconds. (where: owner)
- The catcher is never seen or felt: no box in the middle of the screen while away, the window typed in keeps the focus; with "Drag files across the edge" off no catcher is put up, and a drag carried across ends as before BK4. (where: owner)
- Files dragged out of a zip opened in Explorer, carried across and let go on Linux: note whether they land, or the drag is refused (no "Sending" badge, nothing dropped elsewhere). (where: owner)
- Photos dragged from the phone shown in Explorer (MTP, "This PC" > the phone), carried across and let go on Linux: note whether they land, or the drag is refused as above. (where: owner)
- With the server installed `--elevated`, note what a drag across does (Windows is expected to refuse the drop: nothing sent, nothing dropped elsewhere). (where: owner)

**217. The phone as another screen (BK5)**

- The phone shows on the Keyboard and mouse page as a block shaped like a phone, Your phones says "Ready"; drag it beside a screen, push the real mouse off that edge, and a pointer shows on the phone at the same height; push back and the mouse is home where it left. (where: owner)
- With the real mouse on the phone: a click taps, a drag swipes a list, the wheel scrolls where the pointer is, the right button is Back and the middle Home; typed keys go into a field there, Enter sends, Esc is Back. The Windows cursor stays hidden meanwhile, and nothing typed lands on Windows. (where: owner)
- The switch key's cycle reaches the phone as it reaches a computer, landing in the middle of its screen. (where: owner)
- Turn the phone, then push onto it again: its block on the page has turned within a few seconds and the pointer lands on the edge that now faces the computer. (where: owner)
- A drag held while the phone's Wi-Fi is turned off: within about a second the mouse is home and the finger on the phone lifted (no stuck press when the phone comes back). (where: owner)
- Move slowly across the phone and drag a slider, and fast across a list: the pointer and the finger keep up without a visible lag or stall; a press held still for 3 s on an icon opens its menu and a drag after it goes on. (where: owner)
- Hold Backspace and an arrow in a field on the phone: after half a second they repeat until let go of; Enter and Esc never repeat. (where: owner)
- With the phone away from home (its link on the internet road, direct and then through a relay), hold a drag on it for a few seconds and move about: note the lag, and whether the watchdog brings the mouse home by itself (a pong late by a second); the phone is reachable that way, unlike a computer. (where: owner)

**218. Sound follows the mouse (BK6)**

- Play music or a video on Linux, push the pointer onto it: within a moment its sound plays on Windows' speakers; push back home and it stops at once (no tail over a fifth of a second), and it plays on Linux's own speakers throughout. (where: owner)
- By ear with headphones on Windows: no crackle or gaps over a few minutes of music on Linux, and the lag against a video playing on Linux's screen is small enough not to notice for speech (about 40 to 60 ms plus the link's). (where: owner)
- The switch key from Linux to a second computer and back: the sound follows the pointer, never both at once; quick crossings back and forth for ten seconds leave no sound playing at home and nothing captured on Linux once the pointer is home (`pw-top` shows no `pocketdesk-audio-capture` stream). (where: owner)
- A Bluetooth or USB headset as Windows' default output: the sound plays in it. Switch Windows' default output mid-visit: note whether the sound moves at once (unplugging the one playing moves it; one merely no longer the default may keep it until the next crossing, which opens the new default). (where: owner)
- Linux with no sound playing for a minute while the pointer is there, then a sound: it is heard on Windows (an idle sink suspended does not end it), and the keyboard and mouse were never slower meanwhile. (where: owner)
- Linux's sound is the music itself, not noise: the format PipeWire is now offered (16-bit stereo at 48 kHz) is what it sends, on a sink that runs in float (most do). (where: owner)
- With PipeWire stopped on Linux (`systemctl --user stop pipewire`), push onto it: "No sound from <laptop>" flashes on Windows once, and the keyboard and mouse work as before. (where: owner)
- The setting turned off mid-visit: the sound stops within about two seconds (the driver reads its settings every 2 s); turned on again mid-visit, it starts within about two seconds. (where: owner)

**221. No lag while sharing (BK10)**

- At the desk, Windows driving the Linux laptop on Wi-Fi: typing in a terminal on the laptop keeps up with the keys; after a minute's pause with the pointer resting there (the byte stopped after ten seconds), the first key comes within a tenth of a second and the rest at once; with the setting off, the same by comparison. (where: owner)
- Windows' timer given back: with no other program holding a finer one (close browsers playing video), a timer tool such as ClockRes reads 1 ms while the pointer is on the laptop, and 15.6 ms within a second of it coming home; with the setting off it stays at 15.6 ms throughout. (where: owner)

**225. Both computers driving at once, and two pairing papercuts (from the owner's first live test)**

- With Windows' network set to Public: the Pair page and Settings → Other computers say "Windows treats the network "<name>" as Public…" within about 10 s; set back to Private, the line goes within about 10 s. (where: owner)

**230. Look and feel (Desktop UX 3)**

- On Windows at 100 % and 150 % scaling, and on the Linux laptop (Hyprland), the Overview, Devices and Keyboard and mouse pages read well in light and dark: cards, the status dot, buttons not clipped, the pages' list down the left fits without scrolling at the window's usual size. (where: owner)
- Tab through the Overview and Devices: every button, checkbox and group switch shows the blue 2 px focus ring, and Space toggles a group switch. (where: owner)

**231. Keys that reach the other computer (PC4)**

- On a Windows test machine (never the owner's box), run `POCKETDESK_HOOK_TEST_MACHINE=1 cargo test -p pocketdesk-desktop the_real_hook_goes_in_and_comes_out -- --ignored` (without the variable it skips): the hook goes in and the watchdog takes it out within about 1.5 s, the keyboard working throughout. (where: owner)

## A phone with a SIM

**11. WAN over iroh (R1)**

- Phone on LTE (Wi-Fi off): dial from cached identity succeeds → badge shows "WAN · punching…" then settles to "WAN · direct" or "WAN · relayed — trying direct…", video + input + keyframe-on-gap all work through relays (R4 A1). (where: sim)

**25. Desktop sound on the phone**

- A phone call silences it, and it resumes after the call. Another app starting music takes over for good: switching "Desktop sound on this phone" off and on brings it back. (where: sim)

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

**78. Events and hooks (ecosystem 1a)**

- `text_received` at both detail levels. (where: sim)

**98. An Android 17 pass (N4)**

- Settings shows the Android 17 notes beside "Ring from the computer" and "Show texts on my computer".
- A text with a code: does it reach the computer at all after the three hours, and with its own time? (where: sim)

**130. Ringing calls on the computer (F5)**

- A call ringing on the phone: "Call from <name> (<number>)" on the computer within a second or two, in the toast and on the Notifications page; gone when it stops ringing (answered on the phone, declined, missed). (where: sim)
- Silence from the toast: the ring stops (or the refusal, said); the ringer's volume back after the call. (where: sim)
- Decline (with "Let the computer decline calls"): the call ends; with a call waiting, which one ends. (where: sim)
- The permission granted while notification access was already on: the next call shows (no restart). (where: sim)
- Two calls one after another; a call ending while its note is on its way. (where: sim)

**132. The computer's state on the home screen (G1)**

- "State widget refreshes by itself" off: no ask in the logcat at the next period; on mobile data with it on: none either.

**150. Not from this app (K2b)**

- Settings → Notifications and texts → Muted apps: Unmute, and the app's toasts again.

**152. Pause the phones on the computer (K2a)**

- While paused: a text on Messages without a toast. (where: sim)

**156. Cleared on the computer, cleared on the phone (L1)**

- A ringing call not cleared. (where: sim)

**159. Call a number from the computer (L4)**

- A real call (a SIM): the owner presses Call.

**162. The phone's state on the computer (L9)**

- Mobile data with its bars, on a phone with a SIM (two SIMs: the data SIM's).

**164. Notifications to every computer of mine (L8)**

- The second server shows no answer buttons, and no texts.

**168. Image copies without a session (M3)**

- Either switch off, or the phone on mobile data: nothing fetched; back on Wi-Fi, it is fetched then. (2026-10-25, the images switch off: the image told to the OPPO and not fetched. Mobile data needs a SIM.)

**173. tel: links and Call back (M8)**

- Call back from a ringing call's note, and from one whose call ended (a SIM).

**196. Notifications held by their words (P4)**

- A call's note with words in it (a SIM): still shown.

**203. The phone's contacts on the computer (P10)**

- Texts turned off on Devices and the phone unpaired: the list forgotten.

## A second phone

**1. Connect + pairing gate (Tier 1 #5, Tier-2 #11b)**

- With an *unpaired* second phone: control features (clipboard, monitors) must stay locked out.

**24. Desktop app and QR pairing**

- One code, one phone: after a phone pairs, the Pair tab shows "Paired ✓" and no code, `pairing.pin` is gone, and "Pair another phone" shows a new code. Five wrong PINs typed on a phone make the tab say the code no longer works, and no new code appears until it is clicked. Leaving the tab, closing the window to the tray, and Quit each delete `pairing.pin`.

**43. Several phones at once (N7)**

- Two phones in sessions: the first to send input drives, and the other's taps do nothing while its session says "Another phone is driving".
- After 10 s without input from the driver (or its session ending), the other phone can take control. (where: second phone)
- Sessions tab: the driver is marked "Driving"; Give control hands it over within a second. (where: second phone)
- Settings "Several phones": every phone drives; the hand-back time. (where: second phone)
- Two quick Ends in the Sessions tab both end their sessions. (where: second phone)

**80. Named tasks (ecosystem 1c)**

- Linux (`sh -c`), a long output's end on the phone, and two phones at once.

**100. GNOME and KDE hosts that work unattended (N8)**

- Two phones at once, then one leaving: the other's picture and input go on.

**122. The phone as a pen (E3)**

- Control moving to another phone mid-stroke lifts the pen.

**126. The deck (F1)**

- Each refusal on the button: Input, Tasks, Audio or Links off for the phone; "Deck" off; another phone driving.

**145. The Sessions page in blocks, with the recent ones (J3)**

- Two phones: "driving" on one, Give control on the other. A cast with its window closed: Show screen. Music on the phone: Previous, Play/Pause, Next inside its block.

**147. Dropped files ask where they go (J5)**

- Two (a phone and another computer, or two phones): "Send 3 files to:" with a button each; each button sends all three, one notice; Cancel and Esc send nothing.

**173. tel: links and Call back (M8)**

- Two phones that may dial, neither linked: the Overview asks; a choice dials; Cancel, and two minutes, drop it.

**198. Files to several phones or computers at once (P6)**

- Two real targets (the OPPO and a second computer, or two phones): both offered, each its own row in the sent list.

## Away from home: another network, or through a router

**8. Notifications + find-my-device (R3#11c, Tier-2 #11)**

- `notify-send -a Test "Hi" "body"` on desktop → phone notification appears (channel "Desktop notifications"). Tried on the Azure VM (Xvfb, `dunst` as the daemon, 2026-10-24): the server's monitor started (dbus-monitor) and `notify-send` succeeded, but the phone showed nothing: outside a session, as it should (the phone shows the computer's notifications only in a session's screen), and the session tried with it did not start, so the item stays open.

**11. WAN over iroh (R1)**

- Hole punching through a real NAT (relay-free if possible): check `directAddrs` paths get used. (where: away)
- Relayed → direct upgrade while streaming: badge flips to "WAN · direct" on its own (iroh keeps punching) and the bridge logs "upgraded from relay to direct path" (logcat). Relayed session must stay usable the whole time, not stall on the transition. (where: away)
- Relayed session (R4 A3): within ~2 s of the badge reading "relayed", the desktop log shows "Encoder bitrate target changed" (relay_cap engaged) at effective ≤ 2 Mbit/s followed by "Encoder rebuilt for new bitrate", and picture is visibly softer than LAN/direct. (where: away)
- Full-quality override (R4 A3): while relaying, the badge area offers "Full quality: off" — tap → log shows "Relay quality override changed" then a fresh "Encoder bitrate target changed" back to configured (`pocketdesk sessions` kbps over the next session line). Tap again to re-clamp; toggle survives a stream retry (re-arms). (where: away)
- Clamp release on punch-through: start relayed (override off), wait for the direct upgrade → within ~2 s the log shows "Encoder bitrate target changed" back to configured without touching anything. (where: away)
- V8: back home with the link still by the internet road, the session's home dial answers within its 2 s and the session is a LAN one. (where: away)

**12. Roaming (Tier-3 #13 remainder)**

- Mid-stream, toggle Wi-Fi off → phone falls to LTE → within a few seconds status chip shows connecting and a fresh WAN/LAN session comes up (reconnect-on-new-radio, not live migration). (where: away)
- Wi-Fi↔LTE ping-pong with 3 s cooldown: flapping radio must not thrash a healthy stream. (where: away)
- In PiP: network change must NOT tear the float session (pip guard). (where: away)

**18. Login autostart + tailnet access (server-side)**

- Phone on cellular (Wi-Fi off): same connect via tailnet IP works (Tailscale traverses NAT). Then iroh WAN fallback (`## 11`) with the two machines on genuinely different networks — still pending (phone cellular was OUT_OF_SERVICE at test time). (where: away)

**35. Reaching the desktop from outside (N1)**

- A strict-NAT network (or a hotspot behind carrier NAT): "From outside" warns, and the session rides the relay.
- A network with public IPv6: the IPv6 line is ok. (where: away)

**36. Your own relay (N2)**

- HTTPS mode on a cloud VM with a real name: the certificate is issued, and a session connects with `--allow` ids from `relay-ids`. (where: away)
- A phone that is not paired (or an id not listed) is refused by the relay. (where: away)

**37. Found by id on the Mainline DHT (N3)**

- `dht_publish = "addresses"` on a network with public IPv6: the record holds the IPv6 address, and a phone with relays unreachable connects directly. (where: away)
- A network that blocks outgoing UDP: "Found by id" warns that it was not read back. (where: away)

**42. Wake from the phone (N6)**

- Away from home (another Wi-Fi): no broadcast is sent, only the outside packet; with no outside port, Wake says the phone is away and nothing is set up.
- A computer saved by its tailnet address counts as awake once its server answers. (where: away)

**47. Check connection (T1)**

- A VPN that captures home traffic, and Tailscale off for a tailnet address. (where: away)
- Guest Wi-Fi with AP isolation, and another subnet. (where: away)

**49. Protocol audit (what Pocketdesk sends, and to whom)**

- A Wi-Fi with Tailscale up: a phone browsing mDNS sees the LAN address and no 100.x or fd7a: address. (where: away)

**55. A slow or lossy link (fix/slow-link-watchdog)**

- WAN, direct and relayed: frame rate at a 150-300 ms round trip. (where: away)
- The picture's delay on the lossy link (how stale it runs), and a keyframe after a long stall. (where: away)

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

**148. A sound session notices a computer gone quiet (J6)**

- Over a relayed internet path: the same 14 s; a Wi-Fi roam over 12 s reconnects. (where: away)

**159. Call a number from the computer (L4)**

- With the app behind another, or outside a session with a background link: the notification, its tap opening the dialer.

**162. The phone's state on the computer (L9)**

- Behind Tailscale on the phone: Wi-Fi with its bars, not nothing. (where: away)

**168. Image copies without a session (M3)**

- The tile with an image copied on the phone: the seam file holds it; away from home with no link, it opens one and sends.

**186. A paired computer found again at a new address (O3)**

- The computer on the phone's hotspot (or a new DHCP lease): its card moves to the new address with a snackbar, connects, and keeps its name, wake settings and pairing.

**193. The computer's announcement follows its network (P1)**

- A real move: the computer onto another Wi-Fi, or a VPN brought up, found at the new address within a minute. (where: away)

**222. No pacing stall on the computer link (BK11)**

- The phone on the internet road (mobile data) after this build: its control channel and its screen session connect and play as before. (where: away)
- Files between computers on Cubic: a large file (1 GB or more) sent from one computer to the other with 0.8.2 on both, then with this build on both, over a lossy or internet path (another network, or Wi-Fi far from the router), timed both ways; this build is not much slower. If it is, the transfers could ride a connection of their own on BBR. (where: away)

## A second computer

**29. Computers on this network**

- Two computers on the network, both listed.

**42. Wake from the phone (N6)**

- "Through another computer": an always-on computer relays the wake.

**70. The phone's folders on the computer (T10g)**

- A second computer asking: nothing answered.

**86. Two computers: pairing, the link, the clipboard and files (2f–2h)**

- The same from the desktop app: the link pasted in Settings → Other computers, the outcome shown, B's Devices tab. (where: second computer)
- The link comes back after the network drops ("linked" again within a minute). (where: second computer)
- Off on one, nothing crosses either way; a password manager's copy stays. (where: second computer)
- An image both ways with "Clipboard with my computers" on on both (BK3): a screenshot copied on one is on the other's clipboard within a few seconds, pastes there as a picture, and does not come back; with "Files over the internet" off on either, it stays where it was copied. (where: second computer)
- A 500 MB file each way from the Files tab, broken (the network off for 15 s) and continued, MD5 matching; `pocketdesk send <name> <file> --wait`. (where: second computer)
- Unpair on A (Settings → Other computers): the link goes, and B's Devices page still lists A until unpaired there; after that, a pairing again works. (where: second computer)

**105. Choose where received files land (C2)**

- The computer's chosen folder with a phone's share, a KDE device's file and another computer's; a folder that is gone; Settings' Choose folder… and Use Downloads.

**113. Clipboard history and Resend (D3)**

- A KDE Connect device's text, and another computer's, in the history.

**132. The computer's state on the home screen (G1)**

- Connect to another computer, then go Home: the widget follows it, asked once.

**134. The deck on the home screen (G3)**

- Connect to another computer, then press a button still drawn for the first: nothing is sent, the widget redrawn for the new one.

**138. Wake the computer from Quick Settings (G7)**

- A computer asleep (not this box): woken, "It is awake" within a minute. (where: second computer)

**164. Notifications to every computer of mine (L8)**

- Two throwaway servers, both paired, the second picked: a notification reaches both, and its cancel closes it on both. (where: second computer)
- The own server stopped: the second still gets each notification at once. (where: second computer)
- The second server stopped: the own one still gets every notification on time. (where: second computer)
- The same chat posting, being cleared and posting again: the second is cleared on both. (where: second computer)
- The computer in use swapped (a session with the second): the picks still mean the same computers. (where: second computer)

**165. The other computers answer and clear (L8 part 2)**

- Two throwaway servers, both paired, the second picked, answering on: the second's toast offers the buttons, and pressing one runs it on the phone. (where: second computer)
- A toast closed on the second computer is cleared on the phone.
- The second computer's Overview does not show the phone as linked by that link, and its Send file says it cannot reach the phone.
- One computer saved at two addresses and picked: no link to itself, no doubled toasts. (where: second computer)
- The second server stopped: its link is tried again a minute later, and copies fall back to one-offs. (where: second computer)

**167. Both sides' switches, part 2 (M2)**

- The sheet for a computer that is not the phone's own: every row "with its own computer only", the notifications too unless picked under "Other computers of mine". (where: second computer)

**181. Toasts only on the computer in use (N7)**

- Two throwaway servers, both picked on the OPPO (L8), the switch on: a notification while one is in use pops up there only, and the other's Notifications page and Activity have it ("not shown: another computer was in use").
- Neither in use: both pop up. One's server stopped: the other pops up within 30 s of its last answer. (where: second computer)
- "Tell phones when this computer is in use" off on one: it always pops up. (where: second computer)

**186. A paired computer found again at a new address (O3)**

- Another computer announcing the first one's id at a new address (its own certificate): no move.

**219. Another computer's screen in a window (BK7)**

- Windows views Linux: Show its screen opens a window with Linux's screen within a few seconds, at its shape, the window's title and bar naming the Linux laptop; with "Ask before each session" on there, the picture waits for Allow once at the Linux laptop. (where: second computer)
- View-only by default: the window's bar says Linux does not take this computer's clicks and keys and where to turn Control on; clicks and keys in it do nothing there. Turn on "Control the mouse and keyboard" for the Windows laptop on Linux's Devices page, Reconnect: the pointer moves, clicks, drags and scrolls (both ways, a touchpad's too) where they land in the picture, the letterbox bars take nothing. (where: second computer)
- Keys from the window: typing (capitals with Shift, a non-US character), Enter, Tab, Backspace, Escape, the arrows, Home/End/PgUp/PgDn, Delete, F1 to F12, Shift+arrows selecting, Ctrl+C in a Linux text field then Ctrl+V into another there (Linux's own clipboard, not Windows'), Alt+Tab-like chords; nothing stays held on Linux after the window loses focus mid-press, or closes. (where: second computer)
- A monitor switch: with two screens on Linux, the bar lists them by name and size; choosing the second shows it, and the window keeps working; Linux's resolution changed mid-view: the picture comes back at the new size without a restart. (where: second computer)
- Wi-Fi dropped on Linux for a minute mid-view: never a silent black window (since PC2 the window says "Reconnecting… (n)" and comes back by itself, §224); by the internet path when the home network is gone. (where: second computer)
- Quality: Economy, Balanced, Best and Auto from the bar visibly change the picture within a few seconds (and the bitrate in Linux's session record); the Settings choice is what a new window starts at; View only in Settings starts it sending nothing. (where: second computer)
- Linux's session list (its desktop app's Sessions, and `pocketdesk sessions`) shows the Windows laptop's session by its name; End there ends it and the window says why; privacy mode, the away lock and one driver at a time behave there as for a phone. (where: second computer)
- Sound: with "Hear and change this computer's sound" on for the Windows laptop on Linux, Linux's sound plays on Windows while the window is open, and stops with it. (where: second computer)
- Linux views Windows (paired both ways, §223; or, paired one way, unpair on both, then paste Windows' pairing link on Linux): the same from the Linux laptop's desktop app, the Windows host's capture and encoder (and its session list) behaving as for a phone. (where: second computer)
- Screenshot and Record from the window save a PNG and an MKV of Linux's screen; Full screen fills Windows' screen and Exit full screen comes back. (where: second computer)

**220. Linux drives too (BK8)**

- Before anything: `pacman -Q xdg-desktop-portal-hyprland xdg-desktop-portal hyprland` names xdph 1.4.0 or later, and `hyprctl monitors` names each screen's scale; the Keyboard and mouse page on the Linux laptop says "Ready" (or "Fractional scaling stops crossing" with a scale like 1.25), never "doesn't offer input capture". (where: second computer)
- Cross to Windows and back: the pointer pushed off the Linux laptop's edge that faces Windows on the Keyboard and mouse page appears on Windows at the same height, Linux's own cursor hidden and still; moves, clicks, drags, the wheel and a touchpad's two-finger scroll work there; pushed back off Windows' facing edge, the pointer comes home where it crossed, and the other edges are walls. (where: second computer)
- Keys and modifiers: typing, Shift for capitals, Ctrl+C and Ctrl+V in a Windows text field, Alt+Tab there, the arrows, F-keys; Shift or Ctrl held while crossing in either direction is never left held on either computer (xdph bug 436); the switch key (ctrl+alt+right) while on Windows comes home, and pressed at home on Linux does nothing (no keys at home). (where: second computer)
- The safety nets: Wi-Fi turned off on Windows while driving it brings the keyboard and mouse home within about a second; locking the Linux laptop (its lock screen taking them back) brings them home with nothing held on Windows; plugging in or unplugging a monitor on Linux sets the edges again within a few seconds, and doing it while the pointer is on Windows keeps it there until it comes home (then the new edges are set). (where: second computer)
- A fractional-scale wall: with a screen scaled to 1.25 on Hyprland, pushing at the edge either crosses normally or, if the portal flips (bug 435), turns that edge into a wall at the third flip, flashing why once and the page saying "Fractional scaling stops crossing"; back to scale 1, the edge crosses again. (where: second computer)
- A game full screen: with "Stay on this computer while a game runs" on, a full-screen window in front on Hyprland (a game, or mpv with f) keeps the pointer on Linux at the edge; leaving full screen, the edge crosses again within a second. (where: second computer)

**221. No lag while sharing (BK10)**

- The Linux laptop driving Windows (BK8) with the setting on: `share-test <windows>` from the laptop, the network's one way p50 under 5 ms, and a byte every 50 ms from the laptop to Windows while the pointer is there (a capture on Windows' side, counts only). (where: second computer)

**222. No pacing stall on the computer link (BK11)**

- At the desk, Windows driving the Linux laptop on Wi-Fi: after a few seconds' pause the next key comes at once (no tenth-of-a-second catch), and fast typing and a fast mouse keep up. (where: second computer)
- The Linux laptop driving Windows (BK8): `share-test <windows>` from the laptop with a 1 s pause, the network's one way p95 under 5 ms. (where: second computer)
- Mixed versions: a computer on 0.8.2 (or earlier) and one on this build link both ways (each dialing the other), and the keyboard and mouse cross between them. (where: second computer)
- A computer's answers follow its new link: with the throwaway pair of the runs above, restart the Windows server and run `share-test <laptop> --keys 20` as soon as the link is direct again: the timings come back on the first run, not after about 50 s; and restart the laptop's server while Windows' cursor is home: the next crossing from Windows reaches it (its screens answered at once). (where: second computer)

**223. Both are mine (PC1)**

- Pair once: the box ticked on the Windows laptop's Pair page, its link pasted in the Linux laptop's Settings, Other computers, which names the Windows laptop and its addresses (the Windows laptop's own) and says it would see and control the Linux laptop at any time without asking; ticking that it is the owner's own turns the button to "Pair both ways". Within a few seconds each lists the other, Windows saying "Mine once linked" until the link is up and then, like Linux, "Mine: sees and controls without asking", and both say linked. Pasted again unticked, the button says "Pair one way". (where: second computer)
- Each views and drives the other with no prompt: "Show its screen" on Windows shows Linux's screen with control, and on Linux shows Windows' with control, with nobody at the far computer to click Allow or turn Control on; the window's bar says the other takes this computer's clicks and keys. (where: second computer)
- The shared keyboard and mouse both ways with sharing on at both: Windows drives Linux and Linux drives Windows (BK8) over the one link, never "Waiting for <computer> to answer". (where: second computer)
- Turned back and unpaired: on Linux's Devices page, "Ask before each session" on and Control off for the Windows laptop; the next "Show its screen" from Windows waits for Allow once there and is view-only. Unpair under Other computers on Linux: the Windows laptop leaves both Linux pages, and Windows says refused or waiting for it. (where: second computer)

**224. The window says how it's doing, and comes back by itself (PC2)**

- Stats: the bar's Stats shows over the picture the road (home network, or internet direct or by a relay), how long it took to reach, the picture's size and H.264, the frames a second that came and were shown (about equal on a moving screen), a bitrate that rises when the screen moves, a round trip of a few ms at home and more over the internet, a decode time of a few ms, and lost, late and dropped at 0 on a good link; the choice holds when the window is closed and opened again, and Settings → Other computers shows it. (where: second computer)
- The first picture: with the laptop on the same network, the overlay says the home network won; with the laptop's home address unreachable (another network, or its firewall), the internet path's first frame comes sooner than BK7's 3.8 to 4.7 s (note the overlay's "reached in"). (where: second computer)
- The laptop's Pocketdesk server restarted mid-view (`systemctl --user restart pocketdesk`, or Stop and Start in its desktop app): the window dims the last picture and says "Reconnecting to … (1)", then the picture comes back by itself within a few seconds at the same Quality, with no click; the laptop's session list shows one session for the Windows laptop afterwards, not two. (where: second computer)
- The laptop's Wi-Fi off for 30 s mid-view: "Reconnecting… (n)" counting up, then the picture back by itself once the Wi-Fi returns; off for three minutes: the tries stop after about two minutes and the window says it did not come back, with Reconnect, which works once the Wi-Fi is back. (where: second computer)
- Cancel while it reconnects: the window says it stopped, with Reconnect, and the laptop gets no further sessions from it; End on the laptop's session list mid-view: the window says the laptop ended the view and does not reconnect; with "Ask before each session" on there, a reconnect waits for Allow once again, and a no ends it. (where: second computer)

**225. Both computers driving at once, and two pairing papercuts (from the owner's first live test)**

- Windows drives the laptop while the laptop also drives: Windows' cursor arrives on the laptop's left edge and stays there; the laptop's log says it is driven ("driven by another computer (true)") and no "Keyboard and mouse: <Windows>" follows; moving around the laptop's screens, the pointer never bounces home, and pushing back off the left edge comes home to Windows as before. (where: second computer)
- Windows' cursor left resting on the laptop: after about 2.5 s without a key, click or move from Windows, the laptop's own touchpad crosses its edge onto Windows; Windows' cursor comes home there (its badge says the laptop's keyboard and mouse came here), and the laptop's touchpad drives Windows. (where: second computer)
- Then the laptop drives Windows from its own touchpad: once Windows' cursor left, the laptop's edge facing Windows crosses again within a second or two; Windows' cursor arriving on Windows' right edge stays there too. (where: second computer)
- Windows' watchdog: the laptop's Wi-Fi off while Windows drives it: Windows' keyboard and mouse come home within about a second, and once the Wi-Fi is back the laptop's edges lead to Windows again. (where: second computer)
- A pairing link copied with a byte-order mark or a trailing line break (from a text editor) pastes into Settings → Other computers with Pair enabled, and pairs. (where: second computer)

**226. Pairing and disabled controls say why (Desktop UX 1)**

- Paste on Settings → Other computers fills the box from the real clipboard, on Windows and on the Linux laptop, and the line under it names the other computer and its addresses. (where: second computer)
- Start server in the bar at the top of a page starts the stopped server on the Linux laptop, and the bar goes once it runs. (where: second computer)
- A real failed pairing: with Pocketdesk stopped on the other computer, Pair shows its progress, then "The other computer didn't answer…" with Details and Run diagnostics, which opens Diagnostics and runs its checks. (where: second computer)

**227. The clipboard and files follow the window (PC3)**

- Copy a line on Windows, click into the laptop's screen window: within a second the laptop's clipboard holds it (paste it there with Ctrl+V through the window); copy something on the laptop through the window, click back on a Windows app: Windows' clipboard holds the laptop's copy. A quick Alt+Tab through the window and back moves nothing. (where: second computer)
- Copy an image on Windows (a screenshot with Win+Shift+S), focus the window: the laptop's clipboard holds the image; with "Files over the internet" off on either, it does not go. A KeePassXC password copied on the laptop does not come back to Windows as the window loses the focus. (where: second computer)
- With View only on in the window's bar, or Control off for Windows on the laptop's Devices page, focusing the window moves nothing; with "Clipboard follows a computer's screen window" off on either computer, nothing moves; with "Clipboard with my computers" on on both, copies cross as that sync sends them and the window adds no second copy. (where: second computer)
- Drop three files and a folder from Explorer onto the window: the note says "Sending 3 files and 1 folder to <laptop>…", then "Sent 3 files and 1 folder to <laptop>'s Downloads.", and they are there; with Files off for Windows on the laptop's Devices page, the note says Files is off for this computer there; with the laptop's Wi-Fi off, it says the link is down. (where: second computer)

**228. The always-on computer clipboard asks the Clipboard permission**

- With the two laptops paired both ways and "Clipboard with my computers" on both, turn Clipboard off for the Linux laptop on the Windows Devices page: a copy on the laptop no longer lands on Windows; turn it back on and it does. A pairing one way only keeps syncing as before. (where: second computer)
- With Clipboard off for the laptop on Windows' Devices page but Shared input on, cross from the laptop to Windows with something copied: Windows' clipboard does not take it; with Clipboard on, it does. (where: second computer)

**229. Pair a computer nearby, and one place per task (Desktop UX 2)**

- Pair the two laptops from the nearby list: on Windows, Pair → Computer lists the Arch laptop under Nearby within about 10 s, with its address; Pair on it shows a six-digit code with "Same code on both?" and Confirm; the Arch laptop shows "<Windows' name> wants to pair" with the same code, as a notification too; Confirm on Windows (it says "Confirmed. Waiting for … to accept") and Accept on the laptop, and Windows says "Paired with …" with Set up keyboard and mouse, and each lists the other (Windows under Settings → Other computers, the laptop on its Devices page). (where: second computer)
- The same the other way (the Arch laptop asks Windows), with "Both are mine" ticked on both sides: each lists the other as Mine. (where: second computer)
- Both owners must say yes (PR #556 security review): Accept on the asked computer while the asking one still shows "Same code on both?" pairs nothing until Confirm there; Accept first then Confirm, and Confirm first then Accept, each pair. Cancel on the asking computer takes the other's prompt down at once, and a new ask can be made right away. (where: second computer)
- The Arch laptop asks Windows, and its Wi-Fi is turned off while Windows shows the prompt: the prompt goes at once, and the other takes a new ask. (where: second computer)
- Decline on the asked computer, and Escape on its prompt: the asking one says it declined; nothing is paired. A minute without an answer declines too. (where: second computer)
- The asked computer's window hidden to the tray: the notification shows the code, and the window comes forward with the prompt. (where: second computer)
- "Let computers nearby ask to pair" off on the asked computer: the asking one says it isn't taking asks now, and no prompt shows there. (where: second computer)
- Windows set to a Public network: the Arch laptop's Nearby list never shows Windows, and after about 10 s says why (the reasons, with how to make it Private on Windows' own list) and offers Run diagnostics. (where: second computer)
- The Keyboard and mouse page's Setup on both laptops ticks each step as it becomes true (running, paired, sharing on, linked directly, may drive, arranged), and each fix button does what it says. (where: second computer)
- An option changed on the Keyboard and mouse page (Clipboard follows the mouse off) is saved at once and the next crossing follows it; Settings' search for "game" finds the pointer, whose button opens the page. (where: second computer)

**230. Look and feel (Desktop UX 3)**

- With the server running, the Overview's status card shows "The server is running" with Restart server, and Restart works; stopped, its Start server starts it and no bar shows above the Overview (other pages keep the bar). (where: second computer)
- A paired computer shows on the Overview with its link state; Show its screen opens its window; Share keyboard and mouse opens that page. (where: second computer)

**231. Keys that reach the other computer (PC4)**

- Windows viewing the Linux laptop, windowed: Send keys lists Windows key (Super), Alt+Tab, Alt+F4 and Print Screen, and not Ctrl+Alt+Del, Task Manager or Lock; each reaches the laptop (record with `wev` on the headless sway stand-in: key names and counts only), Super opening Hyprland's launcher and Alt+F4 closing the focused window there; with Alt held through the window, Print Screen arrives as Alt up, Print Screen, Alt down (`wev`), never Alt+SysRq. (where: second computer)
- The Linux laptop viewing Windows: Send keys lists Ctrl+Shift+Esc (Task Manager) and Lock (Windows+L) as well; Task Manager opens on Windows, and note whether Lock locks it (Windows may ignore an injected Windows+L; if it does, Lock needs a server action instead). (where: second computer)
- With View only on, or Control off for this computer there, the bar shows no Send keys and no "Send Win and Alt+Tab there". (where: second computer)
- Windows viewing the laptop in full screen with the grab on (never press Windows+L here: it still locks this computer, the owner's box, whatever the grab does): Win alone, Win+1 to Win+4 (Hyprland's workspaces), Win+Enter, Alt+Tab, Alt+Esc and Ctrl+Esc act on the laptop, and Windows' Start menu and task switcher never open, nor does Alt let go after Alt+Tab leave the window's own menu waiting for a key; windowed, or with the setting off, they are Windows' again. (where: second computer)
- In full screen with the grab on, tap Alt+Tab as fast as possible ten times: note how many reach the laptop as a plain Tab (Alt and Tab let go within one pass of the window send the Tab after Alt's release; review M6, left as is unless it shows). (where: second computer)
- In full screen with the grab on, press and let go of Ctrl+Alt+Shift: Win and Alt+Tab are Windows' again; leave full screen and come back, and they go to the laptop again. Ctrl+Alt+Shift+S still switches the stats without giving the keys back. (where: second computer)
- In full screen with Win held through the window, Alt+Tab away by a mouse click elsewhere (a second monitor) or a toast: nothing stays held on the laptop, and Windows' own Win key works at once. (where: second computer)
- With the shared keyboard and mouse across on another computer while the window is in full screen, keys go where the shared keyboard is, once, never to both. With privacy mode turned on here from the phone, Win and Alt+Tab go nowhere (the shield keeps them), not to the laptop. (where: second computer)

**232. Your computers on the Overview and in the tray, and waking one that sleeps (PC6)**

- With the link up, Windows' `computers.json` holds the laptop's entry with `wake` (its adapter's MAC and broadcast); the laptop's own Diagnostics names the same adapter. Paired one way instead, neither computer's entry gets `wake`, and no Wake shows. Turn "Let my computers wake this one" off on the laptop and restart its server: once the link is back, the entry has no `wake`, and Wake no longer shows. (where: second computer)
- Sleep the laptop by hand, on the laptop itself, never this Windows box (wired, if it has a port): its card on Windows' Overview shows Wake with the note on wired and Wi-Fi; Wake wakes it, and its card says linked again within about a minute. Once more on Wi-Fi only: say whether it woke. (where: second computer)
- With the laptop asleep, Show its screen on its card offers "Wake it and show": the countdown runs, the laptop wakes, and its window opens by itself (note how many seconds the countdown had used); Cancel during the countdown stops the wait (the window does not open later). With Wake-on-LAN off on the laptop, two minutes later the notice says it did not come up. (where: second computer)
- The tray on Windows (and the laptop's tray, SNI, for a computer it pasted) lists "Show <name>'s screen" above Quit, the most recently viewed first, and "Wake <name>" only while that computer sleeps; Show opens the window, or with it asleep the Overview with the offer to wake it. (where: second computer)
- Show two computers' screens one after the other: the Overview's cards and the tray list the last one shown first, with "viewed just now". (where: second computer)

**233. Privacy for the far computer from the window (PC8)**

- Windows viewing the laptop with the laptop's "Lock, sleep, privacy and wake others" off for Windows: Hide what I do there stays off and the bar says that switch is off there, by its name. (where: second computer)
- Turn that switch on on the laptop: Hide what I do there turns on within a second, the laptop's keyboard and touchpad stop working there while the window still drives it, and the bar says it is hidden; the laptop's ring shows on its screen (look at the laptop, or a screencopy count of red edge pixels) and is not in the window's picture. (where: second computer)
- Turn it off from the bar, turn View only on, and close the window, each in turn: the laptop's keyboard and touchpad work again within a second each time. (where: second computer)
- With it on, cut the link (stop Windows' server, or turn Wi-Fi off on Windows): the laptop's keyboard and touchpad come back without waiting for the 10-minute TTL (note how long), and the window, once back, says privacy ended. (where: second computer)
- With it on, end the desktop app on Windows from Task Manager (a crash, not a close): the laptop's keyboard and touchpad come back within about 4 s. (where: second computer)
- Both ways of the link: repeat the on and off with the laptop as the computer that keeps the link (its id sorts first) and with Windows keeping it, by pairing the other way round or a throwaway pair of ids. (where: second computer)
- Lock it when I close the window, on (bar or Settings), with the switch on there: closing the window locks the laptop (`loginctl lock-session` or hyprlock); with the switch off there it does not, and a notification here says "Couldn't lock" with that switch's name; with the option off it does not. Never the other way round: Windows here is the owner's box and is never the one locked. (where: second computer)
- With a phone holding privacy on the laptop first, Hide what I do there says another phone or computer holds privacy mode, and closing the window with the lock option still locks the laptop and leaves the phone's grab as it was. (where: second computer)

**234. A faster picture: hardware decode, NV12 and HEVC (PC5)**

- In the real window on Windows viewing the Linux laptop: Stats say `d3d11va`, and the picture's colours match the laptop's screen (a red, a skin tone and a grey side by side; NV12 by the shader against the window's own Screenshot, which converts on the processor); resize, full screen, the dimmed picture while it reconnects and another screen all draw. (where: second computer)
- HEVC both ways: with `allow_hevc` on on the other computer, Stats say `HEVC` and the picture is right; another screen, or a resolution change there, restarts it at the new size; with "Use HEVC when both can" off here (next view) it is H.264 again. Note the bitrate Stats say for each codec on the same screen. (where: second computer)
- Record a view that came as HEVC (Stats say `HEVC`) for ten seconds: the MKV plays in a player and is ten seconds long (`ffprobe` says `codec_name=hevc`). (where: second computer)
- "Decode a computer's screen on the graphics card" off: the next view's Stats say `software`; on again, the card. Compare the ffmpeg sidecar's CPU in Task Manager (or `top` on the laptop) for the card and software at the same screen, 1440p and full motion. (where: second computer)

## A Linux desktop (the Azure VM serves for some)

**2. Session shell + video (Tier 1 #5, R2#2, R2#5)**

- R4 B1 screencopy backend (desktop-observable): starting a stream on (where: linux) Hyprland must NOT raise the portal screen-share grant dialog; server log shows "Capture started via backend Screencopy". If instead it shows "Capture backend Screencopy unavailable (…); trying next" followed by "Capture started via backend Portal", the reason is in the line — that's the fallback working, but note it as a finding. `POCKETDESK_SCREENCOPY=0` forces the portal path.
- R4 B3 capture_backend config override: set `capture_backend = "portal"` in (where: linux) config.toml, restart, start a stream → log shows "Capture started via backend Portal" and NO screencopy attempt line (pipeline pinned). With `capture_backend = "screencopy"` on a NON-wlroots/portal-only Wayland, the server must fail the session with a "Capture backend Screencopy unavailable" error rather than silently falling back to the portal (explicit intent surfaces). Omitting the key (or `"auto"`) restores the screencopy→portal fallback above. An unknown value (e.g. `"v4l2"`) makes the server refuse to start with a TOML parse error.
- R4 B1 idle pacing + freshness: keep the desktop static → `src` fps drops to ≤10 (where: linux) (damage-driven VFR); move a window → back to target fps within a frame or two, no tearing or upside-down frames (YInvert handling).
- R4 B1 multi-monitor: pick monitor N in the picker → log line "Screencopy capture: output at (where: linux) (x,y)" shows that monitor's wl_output origin and frames are its size.
- R4 C2 startup ladder (desktop-observable): on a box where hardware encoding is broken (e.g. force `hardware_encoder = "nvenc"` here — this box's NVIDIA driver predates FFmpeg 9 NVENC, so the sidecar dies), start a session → server log shows "Hardware encoder unavailable (…); falling back to software (C2)" and video still appears within ~2 s on x264.
- R4 C2 mid-session encoder kill (HEVC session): during a live `codec=H.265` session, `pkill -f 'ffmpeg .*vaapi'` (or kill the encoder child seen in `pstree` of the server) → log shows "Rebuilt encoder on software rung (C2)", video resuming within ~2 s and the phone toasts "Desktop switched encoder — now H.264" (MediaCodec swap re-sniffed from the first software IDR).
- R4 C3 NVENC power pin (**NVIDIA hardware only — skip on non-NVIDIA boxes**): start an NVENC session (server log "Video encoder sidecar" on `h264_nvenc`/`hevc_nvenc`) → server log "pinned NVENC GPU 0 graphics clock to N MHz"; `nvidia-smi -q -d CLOCK -i 0` shows the graphics clock held at its max. End the session → log "restored NVENC GPU 0 to default clock management" and `nvidia-smi -q -d CLOCK` shows clocks back under adaptive management. On an unprivileged desktop the pin self-declines (log "clock lock refused ... running unpinned") and no reset is issued. (where: linux)

**2c. In-process VAAPI encode (R4 C4) — desktop/box-observable, no device needed**

- R4 C4 host proof (no phone): `cargo test -p pocketdesk-core --lib encoder_vaapi` →  (where: linux) `test_vaapi_encode_roundtrip` encodes 40 real frames through `h264_vaapi` on this box's Intel iHD node and asserts Annex-B packets + ≥1 keyframe (self-skips green where no VA node exists).
- R4 C4 device-node selection: on the hybrid Intel+NVIDIA box the probe must land on the (where: linux) VA-capable node, NOT the hardcoded NVIDIA `renderD128` — `POCKETDESK_VAAPI_DEVICE=/dev/dri/renderD129` starts a session → server log "Video encoder: in-process VAAPI (h264_vaapi) on /dev/dri/renderD129" and **no `ffmpeg` sidecar child** is spawned for the vaapi path (`pstree` of the server). Point the override at a VA-less node (e.g. `/dev/dri/renderD128`) → it degrades to sidecar-then-software (C2), never hangs.
- R4 C4 quality (needs a phone): a VAAPI-encoded session decodes with correct color (no NV12 chroma (where: linux) shift) and the GOP (fps×2) keyframe lets first-frame render land within ~2 s.

**2d. Rate and link basis (roadmap 2056)**

- The stall check this replaces: freeze the desktop's output (sleep the display, or `SIGSTOP` the (where: linux) server) → HUD `fps` and `kbps` must reach 0 within ~3 s and stay there, with the caption still naming a real span. Under the old lifetime average this was the bug: a dead link read healthy for minutes. Resume → the rates recover within ~3 s, not within a session's worth of averaging.

**3. Input paths (R2#4, Tier 1 #2)**

- ...and a tap is actually a **click** (roadmap 2061, re-verify 2026-09-24). The 2026-09-22 note above (where: linux) claimed "click (BTN_LEFT) lands", but the code that ran sent absolute motion plus a left *release* and never a press — so that half was either an inference or a window reacting to something else. A tap must now put `BTN_LEFT` down (value 1) and up (value 0) at the touched point: check with `evtest` on the abs-pointer device rather than by whether a window happened to react.
- Drag (roadmap 2061, shipped 2026-09-24, device-unverified): start a slow drag-select across desktop text — `evtest` should show the pointer warp to the touched point, `BTN_LEFT 1`, moving `ABS_X/ABS_Y` while held, `BTN_LEFT 0` on lift, and the selection must follow the finger (before this change a drag was motion-with-button-up: nothing began, and the release landed on a button that was never pressed). Then the leak check: begin a one-finger drag and add a second finger mid-drag (pinch) — the button must still come up, and the desktop must not be left dragging. Note there is no server-side safety net for a lost release packet yet (roadmap 2272): a session that dies mid-drag leaves `BTN_LEFT` held on the uinput device until the next release. (where: linux)
- Keyboard: type letters/digits/modifiers via on-screen shortcuts + remote input — check against `evtest`: Super, Alt+Tab, Ctrl+Alt+Del, PrtSc, Esc, Super+1..9 work (ShortcutBar). (Esc tap verified 2026-09-22; this box binds workspace cycling to Super+Tab, so the bar's Alt+Tab is config-dependent, not a pipeline failure.) (where: linux)
- The same keyboard list **run twice, once per injection backend** (roadmap 2057-2060). Under (where: linux) Wayland the server prefers uinput and emits the evdev code the phone sent, so nothing is translated; the enigo/X11 rung is the one that has to name every key, and it is the rung a machine without `/dev/uinput` access lands on. Read which one is live from the server log ("using uinput" vs "using enigo"), then re-run the list with the other by starting the server as a user who cannot open `/dev/uinput`. Symbols matter most here: `- = [ ] ; ' \` , . /` and the dialpad `*`/`+` were refused outright on that rung before the table was completed.

**5. Window + monitor streaming (R3#7, Tier-2 #10)**

- Window picker lists Hyprland windows with title/class/workspace/size + "focused" marker. (where: linux)
- Picking a window: video re-encodes at window size (encoder rebuild), crop tracks the window, taps map to desktop coords via the screen box. (where: linux)
- Moving the window mid-session: crop follows only after re-pick (known limitation — crop is static rect). (where: linux)
- "Whole desktop" clears the crop. (where: linux)
- R4 B2 (Hyprland): pick a window → server log shows "Streaming compositor-cropped window frames"; partly-occluded window streams correctly (occluder NOT in the video), and only the window's pixels are encoded. (where: linux)
- R4 B2: close the streamed window mid-session → stream keeps running on full-output frames with software crop (warn log "did not stage"; no session drop). (where: linux)
- R4 B2 (non-Hyprland wlroots, no export global): window pick still works via software crop exactly as pre-B2. (where: linux)
- Monitor picker lists monitors via xcap; selecting index N streams that monitor's region; reconnect persists the choice per host.

**6. Workspace HUD + battery (R3#8, Tier-2 #11a)**

- Workspace chips appear under StatsHud; open/close/focus a window on desktop → HUD updates within ~4 s. (where: linux)
- Tap a chip ≤9 → Super+N injected → workspace switches. (where: linux)
- R4 E1: a row of window chips (title/class) shows under the workspace chips on a Hyprland session; tap one → video re-encodes cropped to that window (same as the picker), the tapped chip goes solid/highlighted. (where: linux)
- R4 E1: re-tap the highlighted window chip → crop clears back to whole-desktop (monitor) view; a *different* window chip tap switches the crop directly. (where: linux)
- Roadmap 2066 (needs a second output or a hotplug-capable desktop): with the HUD open, unplug/replug a monitor → server log shows `Hyprland output changed, refreshing compositor state` within a second of the event (not up to 4 s) → HUD chips and window-crop geometry re-layout immediately. Never observed here: this box has one built-in output and Hyprland's socket1 write dispatchers are broken upstream, so no fake hotplug exists. (where: linux)
- Battery chip shows desktop percentage + charging state; matches `upower`.

**8. Notifications + find-my-device (R3#11c, Tier-2 #11)**

- Inline reply from the shade → desktop `replies.log` records it, text on desktop clipboard, confirmation popup; originating notification disappears. (where: linux)

**8a. Control-channel versioning (R4 D4)**

- Reply still works end-to-end after the `kdeconnect.notification-reply` → (where: linux) `kdeconnect.pocketdesk.notification_reply` rename (desktop-side check): reply from the phone shade → `replies.log` records it + confirmation popup (the server only matches the new type now, so an old client build's reply would silently no-op — both ends must be from this build).

**9. Privacy mode + lock (Tier-3 #15, R4 E4)**

- "Privacy: on" → physical keyboard/mouse on desktop stop responding (`evtest`: grabs held), while phone input still flows. (where: linux)
- Compositor shield (R4 E4, Hyprland/wlroots): the same "Privacy: on" also paints a full-perimeter red frame on every monitor within ~1 s; the desktop centre stays visible and interactive from the phone (the ring is `KeyboardInteractivity::None`, click-through). Desktop log shows "privacy: layer-shell privacy shield engaged". (where: linux)
- Leave session with privacy on → release happens (phone app exit); also verify 10-min TTL auto-release by force-killing the phone app mid-grab. (where: linux)
- Shield tears down with the grab: "Privacy: off" → the red frame disappears immediately; force-killing the phone app mid-grab → frame gone after the TTL auto-release. (where: linux)
- Non-wlroots desktop (GNOME/KDE/X11, or `POCKETDESK_SCREENCOPY=0` headless): no shield is attempted — grab still works, no crash, no "shield engaged" log line. (where: linux)
- Without `input` group: clean error, no crash. (where: linux)

**10. Audio control (Tier-3 #16)**

- Audio sheet opens with volume %, mute state, sink list (matches `wpctl status`).
- Slider → desktop volume changes on release; mute switch works; selecting a sink routes default output (verify with `wpctl inspect` + real playback).

**10a. Phone mic share (R4 E2)**

- Granting it → button reads "Mic: on" (amber) and the desktop gains a source: `pactl list short sources | grep pocketdesk_mic` (or pavucontrol → Recording). (where: linux)
- Speaking moves the pavucontrol input meter for "Pocketdesk Mic"; `pw-record --target pocketdesk_mic /tmp/mic.wav` captures audible speech (play it back). (where: linux)
- "Mic: on" tap → source disappears immediately; `pgrep -f pocketdesk_mic` empty (no leaked pw-loopback children after repeated toggling). (where: linux)
- View-only latched → mic keeps flowing (it is session media, not injected input); input is still dropped. (where: linux)
- Exit the session while mic is on → source disappears (release-on-dispose); force-killing the phone app → source gone within the QUIC idle timeout (~45 s). (where: linux)
- Screen off / PiP while mic is on (Android 14+): audio keeps flowing — the FGS carries the microphone type (logcat: no SecurityException, no silent AudioRecord). (where: linux)
- WAN (relayed or direct) session: the same checks pass over the iroh path. (where: linux)

**11. WAN over iroh (R1)**

- Loss response (roadmap 2053): degrade the link — `tc qdisc add dev <iface> root netem loss 5%` on the desktop, or walk the phone toward the far edge of the Wi-Fi mid-session — and within a few 2 s ticks the desktop log shows "Encoder bitrate target changed" carrying a `loss_cap=` field with effective below configured, then "Encoder rebuilt for new bitrate" and a visibly softer picture. The cut is 20 % per congested tick down to a 1 Mbit/s floor (a configured rate below the floor is never raised to it). Restore the link → the same line reappears **without** `loss_cap=` as the rate climbs back ~10 % per tick to configured, then stops being a term at all. (where: linux)

**18. Login autostart + tailnet access (server-side)**

- Reboot the desktop → after reaching the Hyprland session, `systemctl --user status pocketdesk` is active without any manual start (WantedBy=graphical-session.target). (where: linux)
- Phone reconnects to the auto-started server WITHOUT re-pairing (QUIC identity persisted in `~/.local/state/pocketdesk/certs/`; a regenerated cert would trip the TOFU pin). (where: linux)
- Log out / session end → service stops with the session (PartOf), never orphaned. (where: linux)
- `install.sh` run → unit lands in `~/.config/systemd/user/`, ExecStart points at the real prefix path, prompt enables it; `--status` reports the user unit; `--uninstall` removes it and any legacy `/etc/systemd/system` unit. (where: linux)

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

- Linux: a `POCKETDESK_STATE_DIR` the user cannot chmod logs a warning and the server still starts. (where: linux)

**50. Resumable transfers (T6a)**

- Android 8–9 (the legacy download file), and the Linux host.

**51. Folders (T6b)**

- Android 8–9, and the Linux host.

**52. Download a folder from the remote browser**

- The Linux host.

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
- No RemoteDesktop portal (or refused): the screen alone.
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

- A real lock, on a computer where locking is allowed (not this box), Windows; on Linux under GNOME, KDE and X11 (R3), and that it never locks on Sway or Hyprland.

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

**140. Send the clipboard to the phone with a key (H3)**

- Linux (R4): the key and the typing key through the GlobalShortcuts portal on KDE Plasma 6 and GNOME 48 (the desktop's question once, then a press sending), and Settings saying "no global shortcuts portal" on Sway.

**143. The Logs page (J1)**

- Copy pastes the lines shown; Open log folder opens the folder at the file (Windows, then Linux).

**150. Not from this app (K2b)**

- Linux: GNOME and KDE show the action; a server without actions shows none.

**152. Pause the phones on the computer (K2a)**

- Linux: the pause in the StatusNotifierItem menu.

**153. Notifications with their app's icon, and pictures (K3)**

- Linux: GNOME and KDE show the icon; a server with body images shows the picture.

**155. The phone's sound with its screen (K5)**

- Linux: the sound through `pw-cat`.

**156. Cleared on the computer, cleared on the phone (L1)**

- Linux (GNOME): closing a notification clears it; pressing its button does not.

**168. Image copies without a session (M3)**

- GNOME (no `wl-paste --watch`): an image on the clipboard is read at most every 30 s.

**173. tel: links and Call back (M8)**

- Linux: the desktop file and `xdg-open tel:5550100`; removed, `mimeapps.list` no longer names it.

**177. Open on the phone (N3)**

- The button by hand on the Notifications page, in a Windows toast, and as a Linux D-Bus action.

**178. The phone's ongoing updates on the computer (N4)**

- A Linux desktop (the `value` hint, the bar, a close).

**179. The computer's music in the phone's media controls (N5)**

- A Linux computer (`playerctl`, `mpris:artUrl`).

**180. Files into the phone's folders from the computer's drive (N6)**

- A Linux file manager (Nautilus, Dolphin) over `dav://`.

**181. Toasts only on the computer in use (N7)**

- A Linux computer: it does not say, and always pops up.

**183. Linux catch-up: the phone's music in MPRIS, the laser dot, the typing and cast windows (N10)**

- The laser dot moved by turning the phone (the phone lay still), and across two outputs.
- GNOME's and KDE's media controls by eye, and the media keys.
- A press on the cast window's picture (the VM has no pointer). (where: linux)
- Orca with the window (AccessKit's Linux backend, fixed here: it had panicked at start).

**190. Received images onto the clipboard (O7)**

- Linux: a JPEG onto the Wayland clipboard (`wl-paste --list-types` says `image/png`). (where: linux)

**191. Choose the computer's player (O6)**

- Linux: two MPRIS players under `playerctl`. (where: linux)

**192. Diagnostics' firewall row on Linux (O9)**

- No ufw: "no firewall found". (where: linux)
- ufw on with the default deny: the row is red, naming 1716/tcp and 4716/udp with `sudo ufw allow …`; after allowing them, green. (where: linux)
- The app run without root: ufw's rules "need root to read", with `sudo ufw status`. (where: linux)
- firewalld with KDE Connect's service: 1716/tcp in, 4716/udp named. (where: linux)

**193. The computer's announcement follows its network (P1)**

- Linux. (where: linux)

**195. Quiet hours, and quiet while presenting (P3)**

- The Settings row by hand (the days, a bad time's warning) and the Overview's line, also on Linux.

**197. A phone's ringing alarm on the computer (P5)**

- Linux's urgent notification.

**199. Files copied in Explorer sent with the clipboard key (P7)**

- Linux: files copied in Nautilus or Dolphin (`text/uri-list`).

**200. Links by site (P8)**

- A phone's link through each rule on a real server: `show`, `browser`, a program (Windows and Linux), a program that is not there.

**204. The desktop's O and P pages after their design pass (Q2)**

- Messages and the apps box: "Asking…", Copied., Ask again, the box coming into view; Narrator or Orca reading a row's buttons.
- Settings on Linux: the presenting row naming Do Not Disturb (Q4), no "Never send copies from these programs" row; a bad link rule, words line or quiet hours keeping Save off.

**206. Linux catch-up for the P cycle (Q4)**

- GNOME: Do Not Disturb on, "While presenting or in full screen" on: a phone's notification logged with `presenting`, no pop-up; off again, shown (within 5 s).
- KDE Plasma: its Do Not Disturb, and while it presents, the same.
- A phone's alarm on KDE: the urgent notification with the alarm sound; on GNOME, silent but urgent.
- X11 (no `WAYLAND_DISPLAY`): files copied in Nautilus or Dolphin sent with the clipboard key or "Paste copied files", through `xclip`; with `xclip` missing, nothing read and nothing hung.

**207. Start beside KDE Connect (T1)**

- Linux with KDE Connect (or GSConnect) itself running: the KDE bridge off, Diagnostics' "Control port moved", and the pairing code naming 1717. (where: linux)

**208. The clipboard on X11 (T2)**

- X11 on a real desktop (Mint, or Xfce): the same in a session, and an image both ways. (where: linux)
- X11 with KeePassXC: a password it copies stays on the computer. (where: linux)
- X11 without `xclip`: nothing read, nothing hung, the server says so once. (where: linux)
- Wayland as before: Sway and GNOME, the watcher still `wl-paste --watch`. (where: linux)

**211. Files to a KDE Connect device (T6)**

- KDE Connect on a Linux desktop (or GSConnect), the same. (where: linux)

**212. The phone's gamepad as a game controller on Linux (T7)**

- Both sticks and the other buttons in `evtest`. (where: linux)
- A game or Steam's controller test seeing an Xbox 360 pad, A/B/X/Y in their places. (where: linux)
- The same with a stick pushed. (where: linux)
- Without uinput access, or with the setting off: keys as before, said once in the log. (where: linux)

**213. Lock when the phone leaves on Sway and Hyprland (T8)**

- Sway: touched meanwhile, it does not lock. (where: linux)
- Hyprland, the same; and a video playing (an idle inhibitor) not keeping it from locking where the compositor offers version 2. (where: linux)
- A compositor without `ext-idle-notify-v1`: never locks, as before. (where: linux)
- V5: the desktop app's Settings shows "Lock after the phone leaves" on Linux, and a change there is taken without a restart. (where: linux)

**214. Driving another computer from Windows (shared input)**

- The badge on Linux (Hyprland or Sway): the cursor arriving flashes "Keyboard and mouse: <laptop>" in the middle of each output, sharp at the output's scale; clicks go through it and the keyboard focus stays; a screenshot of the headless session shows it. (where: linux)
- GNOME (no layer shell), or no font found: an ordinary switch shows nothing, while Identify and a return for a reason (the laptop's sharing turned off, its link pulled) come as a desktop notification. (where: linux)

**215. The shared keyboard and mouse: the computer driven, the arrangement, the clipboard and the latency (shared input)**

- The share-test's result on the desktop app's Diagnostics page. (where: linux)
- Linux kept awake while driven: its screensaver and idle lock do not start while typing goes on from Windows for longer than the idle time. (where: linux)

**220. Linux drives too (BK8)**

- The permission dialog, on GNOME and KDE: with no `input_capture.token` in the state folder, turning sharing on shows the desktop's dialog once; allowed, the page says "Ready"; after a server restart it does not ask again (the token replaced, owner-only); said no, the page says "Needs permission" and it does not ask again until sharing is turned off and on. Hyprland asks nothing: xdph 1.4.1 has no input-capture dialog and gives no restore token, so sharing goes straight to "Ready" (about 18 s after driving was turned on, 2026-10-29) and no token is written. (where: linux)
- The phone's screen sharing still works after driving: with shared input on, turn sharing off and on twice on the Linux laptop, then show its screen on the phone (GNOME or KDE, through the screen-sharing portal; on Hyprland too): it starts within a few seconds, never waiting on a portal call. (where: linux)
- GNOME at scale 2 in its default physical-layout mode: the computer's own screens in `shared_screens.json` (read from xdg-output where Hyprland's IPC is not there) are its logical size, 1920x1080 for a 4K screen at 200%, not 3840x2160 (mutter's xdg-output may report physical-layout boxes); if not, the screens come from somewhere else on GNOME. (where: linux)
- GNOME 45 or later and KDE Plasma 6.1 or later (a VM serves): the dialog, a crossing to a second computer and back, keys, and a lock bringing them home; on GNOME 46 a monitor change may need sharing turned off and on (its portal cannot re-enable a disabled session). (where: linux)

**234. A faster picture: hardware decode, NV12 and HEVC (PC5)**

- The Linux laptop viewing Windows: Stats say `vaapi (renderD129)` (the Intel iHD node, past the NVIDIA renderD128 that has no VA driver); with `POCKETDESK_VAAPI_DEVICE=/dev/dri/renderD128` the window moves on to `auto` or `software` within about 2 s and the picture comes; the shader draws on the laptop's GL (colours as above). (where: linux)

## Other hardware: a foldable, tablet, mouse, game pad, older or newer Android

**13. PiP + multitask (Tier-3 #14, DeX)**

- DeX/docked or split-screen: session is resizeable, letterboxing recomputes, taps still map correctly. (where: other hardware)

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

- A phone that has no room declines an offer; the desktop's Sent list says "the phone declined it: …". (where: other hardware) (a full phone.)
- Download into a phone without enough space: "Not enough space for …", nothing left in Downloads. (where: other hardware) (a full phone.)
- Android 8 or 9: the file lands in the app's own Downloads/Pocketdesk.

**32. Switching screens**

- Three or more monitors: a chip each, in the desktop's order. (where: other hardware)

**33. Shortcuts outside the app (F11)**

- Android 13 or older: the tile opens the session (the older collapse path).

**38. Image clipboard (N4)**

- A 4K screenshot (over 10 MB): it moves, and the phone's memory holds.

**47. Check connection (T1)**

- Android 17's local network permission denied.

**48. Staying connected in the background (T2)**

- Other makers (Xiaomi, OPPO): the guide link and their own battery managers. (where: other hardware)

**49. Protocol audit (what Pocketdesk sends, and to whom)**

- A device-to-device move (Smart Switch) leaves the app unpaired. (where: other hardware)

**59. The phone's media from the computer (T8d)**

- A real player (Spotify, a podcast app): its title, and the app's name (Android 11+ package visibility may give the package name).

**62. A mouse on the phone (T9c)**

- Motion, all five buttons and the wheel (both ways) reach the desktop, and a fast mouse keeps its travel. (where: other hardware) (a mouse.)
- Opening and closing the in-session keyboard: the mouse keeps working. (where: other hardware) (a mouse.)
- The mouse's Back button goes to the desktop and does not end the session. (where: other hardware) (a mouse.)

**65. The clipboard outside a session (T10b)**

- Android 10–12: the tile's window reads the clipboard.

**72. New phone screenshots on the computer (T10h)**

- Android 14+'s "selected photos only" answer, and Android 12 and older.

**76. Hotspot and tethering in Check connection**

- The same computer saved under its home address: the network finding, with "This phone is sharing its connection" beside it. (where: other hardware) (the phone's hotspot.)

**85. iroh 1.3, one endpoint per computer, files over the internet road (2a–2d)**

- The too-old message against a computer from before 2c. (where: other hardware) (a computer running a release from before 2c.)

**87. KDE Connect devices (3a–3c)**

- Cancel after the device's Accept unpairs it on the phone; a second request from the phone while one waits shows nothing new. (where: other hardware) (KDE Connect's app.)
- A device that broadcasts first (this computer dialing it): not built (the receive half waits). (where: other hardware) (KDE Connect's app.)

**98. An Android 17 pass (N4)**

- The app starts, asks for local network access, and a session works once it is granted. (where: other hardware) (Android 17.)
- Ring from the computer with the app in the background and `adb shell cmd audio set-enable-hardening enable`: the notification sounds at the alarm volume, the volume is not raised, and no `AudioHardening` entry names the app. (where: other hardware) (Android 17.)
- A ring that raised the volume on Android 16, cut short, then the phone on 17: the volume is put back when the app is opened. (where: other hardware) (Android 17.)
- A tablet-size window: the session lays out, rotates and resizes (orientation limits are ignored there).

**101. KDE Connect, next steps (N9)**

- A chat app's notification, an update to one (replacing the toast), and a flood held to the limit. (where: other hardware) (KDE Connect's app.)

**104. Control the phone from the computer (C8)**

- Android 13+ sideloaded: the restricted-settings step.
- Advanced Protection (Android 17): the service refused.

**105. Choose where received files land (C2)**

- A picked folder on an SD card or a cloud provider.

**106. Notifications that wait and go later (C3)**

- A computer from before C3: notifications arrive once each, as before. (where: other hardware) (a computer running a release from before C3.)

**110. Recent photos on the computer (C7)**

- HEIC photos: thumbnails shown, Copy saying it cannot. (where: other hardware) (a phone that saves HEIC.)
- Android 8 or 9: the legacy thumbnail path.

**111. Health after updates (D1)**

- A low-RAM phone on Android 10 or older: no "stopped" card.

**117. The phone's folders as a drive (D8)**

- A phone from before D8: the file lands in Downloads, the drive answers 504 after the wait. (where: other hardware) (a phone on a release from before D8.)

**120. Answers that say how they went (E1)**

- A phone from before E1: "sent, but no word from the phone" after a minute. Seen in passing 2026-10-19: an answer sent into a link the phone had just dropped (the app force-stopped) stays "sent", which the page shows as no word back. (where: other hardware) (a phone on a release from before E1.)

**122. The phone as a pen (E3)**

- Pen is not offered on a computer from before E3, and a session that meets one again goes back to Direct touch. (where: other hardware) (a computer on a release from before E3.)

**123. A gamepad on the phone (E4)**

- A real controller connected to the phone (none here): buttons, D-pad or hat, both sticks; Settings off leaves it to Android. (where: other hardware) (a game controller.)

**125. The phone app's design pass (E6)**

- The gamepad in portrait and landscape on the A71 and a 360 dp phone; at font scale 2.0.
- TalkBack on the new rows (the mode chip's action, the gamepad's keys, the folders' Remove).

**129. An air mouse (F4)**

- A phone without a gyroscope keeps the trackpad. (where: other hardware) (a phone without a gyroscope.)

**135. A laser pointer for the air mouse (G4)**

- Two screens: the dot crosses where they meet and stops at an edge, never between them; larger on a 4K screen.

**141. The home-screen widgets, a design pass (I2)**

- Android 11 or older: the pine theme in both modes, no previews, the state widget's layout by its height.

**149. Files keep their dates (K1)**

- Android 10, and a KDE Connect device's share.

**166. Linked with the screen off (M1)**

- A Samsung, a Xiaomi and a Pixel: the service keeps the app linked with the screen off. (where: other hardware)

**167. Both sides' switches, part 2 (M2)**

- A computer from before M2: the sheet says it cannot tell. (where: other hardware) (a computer on a release from before M2.)

**172. The phone's sound without its screen (M7)**

- The microphone not granted on Android 13+: its request shows.

**175. Copies sent by themselves (M10 part 2)**

- Another maker's Android 13+, and Android 12 (the row hidden).

**176. The phone's music without a session, with its cover (N2)**

- Another maker's Android, and Android 10.

**177. Open on the phone (N3)**

- Android 15 and 16 (`ALLOW_ALWAYS`). (where: other hardware) (Android 15 or 16.)

**178. The phone's ongoing updates on the computer (N4)**

- Android 16's Live Updates (`ProgressStyle`) on a phone that has them. (where: other hardware) (Android 16.)

**179. The computer's music in the phone's media controls (N5)**

- A headset's button and a watch's controls. (where: other hardware)

**180. Files into the phone's folders from the computer's drive (N6)**

- A file larger than the phone's free space: refused, said in Explorer. (where: other hardware) (a full phone.)
- A marked folder on an SD card with too little room: the phone checks only its own storage, so the copy fails part way and says so.

**182. Leftovers: typing from the tray and a key, several links, the phone nearly full (N9)**

- A phone with under a gigabyte free: one notification, none again until it has had room. (where: other hardware) (a full phone.)

**185. A screenshot of the phone and its screen locked from the computer (O2)**

- Android 9 to 12, and a phone with a PIN.

**194. Open an app on the phone from the computer (P2)**

- Android 13 and older, and a work profile's apps.

**197. A phone's ringing alarm on the computer (P5)**

- A finished timer, and a Pixel's clock (`CATEGORY_ALARM`, local only). (where: other hardware) (a Pixel.)

**205. The phone's O and P rows after their design pass (Q3)**

- "Let the computer take screenshots and lock this phone" warning while Send new screenshots carries nothing, and gone once it does; hidden on Android 8.

## By hand or eye on this box: hearing, looking, the tray, drag and drop, a screen reader, a real app

**2. Session shell + video (Tier 1 #5, R2#2, R2#5)**

- The session's notification (titled "Pocketdesk", with Disconnect) survives Home and a swipe; its Disconnect action ends the session. (2026-10-23, S1: posted with its Disconnect action; ColorOS's shade does not show it to uiautomator and screenshots are blocked during a session, so the tap is by hand.) (where: by hand)
- Drop phone far from AP / saturate link → frames stall, then recover; the HUD's `lost` rises but no permanent freeze (gap-driven keyframe request, R2#5). (where: by hand) "Drops" means **frames the phone never received** — the holes in the server's per-frame sequence numbers, counted where only the client can count them. So a saturated link must move it (the desktop's transport task trims its backlog before a stall clears), while a frame that arrives late and intact must not: the number can come back down when an out-of-order frame lands, and that is correct, not a bug. A decoder that cannot keep up is a different failure and shows as fps below the desktop's `src` rate, not here.

**2b. Compositor-true latency read (R4 E3)**

- Cross-check (eyes, optional): point the phone camera at the desktop with an on-screen clock/stopwatch — the phone's rendered image lags the physical panel by roughly the HUD `delay` figure (± the phone's own present latency, which the probe can't see).

**8. Notifications + find-my-device (R3#11c, Tier-2 #11)**

- "Ring PC" → desktop plays sound 30 s (pw-play/paplay path; silent no-op is FAIL here since desktop has audio). (where: by hand)
- Yes to `pocketdesk ask` behind a real fingerprint or screen lock ends the command with 0 and `yes`; a cancelled prompt sends nothing; a question turned on its side mid-prompt keeps its Yes (BF3). (where: by hand)

**11. WAN over iroh (R1)**

- `path_chg` counts a walk between Wi-Fi bands or a switch to cellular, and `relayed_s` a relayed session. (where: by hand)

**11b. HUD link-profile presets (R4 E5)**

- A preset change mid-action is seamless: no freeze longer than one keyframe; the post-rebuild IDR reseeds the decoder cleanly (no green/garbled persist). (2026-10-25, the freeze half: the runner's `an1-presets` on the OPPO changed the preset four times mid-session, Economy, Max, Balanced and Auto, each logged by the server, the session up with no redial and its longest rebuild 460 ms. The picture after each is by eye: a screenshot would hold this computer's desktop.)

**14. Blackout / pocket mode (Tier-3 #15 remainder)**

- Recents thumbnail blank during blackout. (where: by hand)
- Pocket test: with overlay on, phone in pocket should not send spurious input (touch consumed by overlay). (where: by hand)

**20. Saved-computers home screen (multi-host store + add-computer form)**

- A real screen's still through Peek (BF2): on Windows, on X11, and on GNOME or KDE under Wayland (their screenshot service may ask at the screen; past 8 s the phone hears it could not take one). (where: by hand)
- The doctor's "Scan code again" for a computer whose key changed forgets all that Remove does: its link, its deck widget list and its "also send to" pick (BC1). Needs a computer whose key changed at a saved address (a second throwaway with fresh certificates on the same port).

**21. UI polish pass C1–C9 (design system, immersive session, error UX, settings)**

- Haptics: tick on the action disc, mode toggle, picker picks and shortcut chips; heavier buzz on Exit; the Settings haptics switch mutes all of them live. (2026-10-25: not readable on the OPPO. Android's vibrator record showed the session's menu tap as Pocketdesk's CLICK at strength 0 and nothing for the mode toggle or Exit: the phone is on silent, which mutes touch vibrations. By hand, with the ringer on.)

**24. Desktop app and QR pairing**

- Scan the desktop app's code with the phone camera (the USB-tethered phone could not see the screen).
- Tray: Open, Pair a phone… and Quit from the tray menu itself.
- Settings → Save on a real `config.toml` keeps comments; the server picks the change up after Restart. (where: by hand)

**25. Desktop sound on the phone**

- Play a video on the desktop during a session: the phone plays its sound, and the lag behind the picture is small (≤ ~150 ms by ear).
- Quick Settings → "Desktop sound on this phone" off: silent at once; on again: sound returns within a second. The setting survives an app restart. (where: by hand)
- Pause the desktop's audio for 10 s, then resume: it comes back cleanly, without a crackle burst. (where: by hand)
- Connecting to a silent desktop does not pause music already playing on the phone; the first desktop sound takes over. "Silent" means no app holds an audio stream open: Windows loopback sends packets of silence while any app has one (seen on the A71 with a browser open: focus was taken 1.3 s after connect), and focus follows packets, not loudness. (where: by hand)
- In PiP the sound continues; Exit stops it. (where: by hand)
- With sound on, the HUD's fps and delay match a session with it off, and taps on a still desktop feel immediate (the bridge lock fixes).

**26. In-session keyboard**

- Landscape: the stream stays visible above the keyboard, and zoom survives opening and closing it. (where: by hand)

**27. Trackpad gestures**

- Two-finger tap opens a context menu; three-finger tap middle-clicks (e.g. opens a link in a new tab). adb cannot make multi-finger touches.
- Two fingers resting without moving do not right-click, and a two-finger tap does not scroll the page first.
- Two-finger scroll is natural (content follows the fingers, like the phone itself) at about one wheel notch per 24 dp, in both axes. Unverified: adb cannot make two-finger touches. (where: by hand)
- Tap, then drag a window by its title bar: it moves without maximising (no double click), and a tap left alone clicks about 0.3 s later.
- A drag survives a second finger landing mid-drag and releases on lift.

**28. Permissions and session approval**

- The desktop app's prompt: it comes to the front (also from the tray) and its three buttons work; Always allow turns the switch off.

**30. Sending files to the desktop**

- Share several photos at once from the gallery (`SEND_MULTIPLE`): "Sending 2 of 3" with a moving progress bar. (where: by hand) (`am` takes no list of URIs, so only the gallery sends one.)
- The desktop shows a notification per file; the app's Files tab lists them, and Show in folder selects each in Explorer. (where: by hand)
- A file from a provider without a size (e.g. some cloud apps) still arrives (copied first). (where: by hand)

**31. Files both ways and the remote browser**

- Tap "Saved … to Downloads"; the file opens. (where: by hand) (ColorOS's shade shows uiautomator no notification of the app.)

**33. Shortcuts outside the app (F11)**

- Place the widget on the home screen: it names the last computer and connects on a tap; with none saved it opens the app. (where: by hand)
- Pin a shortcut to the home screen; it survives the computer being reconnected, and stops working once the computer is removed. (where: by hand)

**34. Desktop diagnostics (F10)**

- Open the Diagnostics tab in the app and click Run checks; Copy debug report pastes the same text.

**38. Image clipboard (N4)**

- Desktop → phone: paste it into a chat app on the phone. (The A71 has no app that pastes images.) (where: by hand)
- Phone → desktop: copy a photo on the phone and paste it into Paint. (The A71 has no app that copies an image: no browser.) (where: by hand)

**39. The phone's notifications on the computer (N5a)**

- A real app's public notification (a chat message) shows its title and text.
- A messaging app that hides its content on the lock screen shows only "New notification". (where: by hand)
- With no computer reachable, the phone's battery and the queue stay calm over an hour. (where: by hand)

**40. Closed on the phone, closed on the computer (N5c, part 1)**

- A chat app that clears its notification when read on the phone. (where: by hand)

**45. Phase R3 and R4: hostile connections, and idle cost**

- The phone's battery over an hour with the app in the background, and while a session is open. (where: by hand)
- Network traffic while idle (both ends). (where: by hand)

**47. Check connection (T1)**

- A changed security key (pin a wrong one), and "Scan code again" from the finding: the computer and its key removed, the scanner opened, and the new code trusted.

**48. Staying connected in the background (T2)**

- Battery use set to Restricted: the card says so, and "Allow" opens the app's page. (where: by hand) (ColorOS keeps no restriction set from the shell.)

**51. Folders (T6b)**

- A folder from the phone shows one "folder arriving" toast, not one per file. (where: by hand) (the phone's folder picker.)
- The phone shows "Not all of … was sent" when it leaves names out, and "Stopped sending …" when the computer goes away mid-folder. (where: by hand) (the phone's folder picker.)
- A folder broken by Wi-Fi mid-way, continued. (where: by hand) (the phone's folder picker.)
- A folder over the limits (10 000 files, 32 levels) says what was left out. (where: by hand) (the phone's folder picker.)
- The desktop app's "Choose folder…" and dropping a folder on the window. (where: by hand)
- Names Windows refuses on the phone (`a:b`) left out and counted. (where: by hand) (the phone's folder picker.)

**52. Download a folder from the remote browser**

- The toast "Asked the computer for …" (the folder arrived, the toast was not caught on screen), and a third folder while two are on the way.
- Closing the sheet before the answer, and the toast still showing. (where: by hand) (a toast uiautomator cannot read.)

**54. Automation apps on the phone (T7b)**

- Tasker itself: the clipboard recipe, a file with "Grant read permission", Connect and Wake from a task.
- From the background, with and without "Display over other apps". (where: by hand) (Tasker.)

**56. Links both ways (T8a)**

- Chrome's and Firefox's share sheets ("Open on computer" listed, the title and link they share). (where: by hand)
- The desktop app's Send link, and its refusal for a phone with Links off. (where: by hand)

**57. The phone's battery on the computer (T8b)**

- The Devices tab's text, and its age after the session ends. (where: by hand)
- Catching up after a Wi-Fi drop. (where: by hand) (the phone's battery reads the hardware, so a report changes only with a real one.)
- The desktop's "Phones' batteries" off: no notification. (where: by hand) (the phone reads its battery from BatteryManager, which `dumpsys battery set` does not move.)
- A real discharge to 15 %, and Android's own low flag. (where: by hand)

**58. The computer's media from the phone (T8c)**

- A real player with a title: a browser video, Spotify; Next and Previous.

**59. The phone's media from the computer (T8d)**

- The Sessions tab itself, and its buttons. (where: by hand)
- The phone's setting turned off clearing the song; a press sent while the app was in the background not acting later. (where: by hand) (a real player on the phone.)
- Turning the setting on opening the notification access page. (where: by hand) (notification access already granted here.)

**60. Pinch-zoom in trackpad mode (T9a)**

- In trackpad mode, two fingers spreading zoom the view, pinching zooms out, back to 1x. (where: by hand) (adb cannot pinch.)
- Two fingers moving together still scroll the desktop, and a scroll that drifts apart does not start a zoom. (where: by hand) (adb cannot pinch.)
- While pinching, moving the fingers pans the view; one finger lifting and landing again does not jump the zoom. (where: by hand) (adb cannot pinch.)
- A two-finger tap is still a right click, a three-finger tap a middle click.

**61. Portrait sessions (T9b)**

- A 16:9 desktop's pane, a monitor switch between shapes, picture-in-picture, and Pane: Single and Dual. (where: by hand)
- Pinch-zoomed upright: the zoomed picture stays inside its area, not under the chrome (adb cannot pinch). (where: by hand) (adb cannot pinch.)

**64. The control channel over the internet (T10a)**

- The phone's battery over an hour with the link up in the background (iroh's 5 s heartbeat).

**66. Answering the phone's notifications from the computer (T10c)**

- "Hide sensitive content" on the lock screen turned on: the probe's notification arrives as "New notification" and offers nothing. (where: by hand)
- The desktop app's Notifications tab itself (its buttons and reply box), and a real chat app (WhatsApp, Signal, Messages).
- An action that opens an activity on Android 14+ (background launches may be blocked). (where: by hand)

**67. Files from the computer without a session (T10d)**

- A phone that refuses background starts: the "The computer sends a file" notification, and Save downloading it. (where: by hand)
- The desktop app's Files tab listing a linked phone with no session. (where: by hand)

**69. "Send to phone" in the file manager (T10e)**

- Send To in Explorer itself, with several files and a folder. (where: by hand)

**70. The phone's folders on the computer (T10g)**

- The desktop app's Phone tab itself (roots, folders, Up, Download). (where: by hand)

**71. Input on wlroots without uinput (T10f)**

- Direct touch (absolute moves), scrolling, right and middle clicks, and a two-output layout.

**72. New phone screenshots on the computer (T10h)**

- A screenshot with the side keys, and a JPEG screenshot (One UI's other format). (where: by hand)

**73. The phone as a remote (T10i)**

- The desktop app's prompt for a remote, clicked. (where: by hand)

**77. This phone's screen on the computer (T10k)**

- The latency against a stopwatch, and over the internet road (WAN, relayed and direct).
- The notification's Stop (the A71 showed only the redacted notification in the shade while locked), and Sessions' "Show screen" after the window was closed. (where: by hand)
- "Share one app" instead of the entire screen; a phone call or a secure screen (banking apps black out); a long cast's heat and battery. (where: by hand)

**78. Events and hooks (ecosystem 1a)**

- The desktop Settings hooks box saving a hook the server then runs. (where: by hand)

**79. The Activity Center (ecosystem 1b)**

- The Activity tab itself on screen (its rows, filters, Show in folder): a click posted to the window did not reach egui, and real input into this box's desktop is not used for tests.
- Pairing and unpairing a phone showing as rows; a folder's Show in folder after it finished arriving. (where: by hand)

**80. Named tasks (ecosystem 1c)**

- The desktop app's Allow / Deny prompt itself (clicked, not written), Deny on the phone, and the Settings task editor. (where: by hand)

**81. Send from the browser (ecosystem 1d)**

- The extension itself loaded in Brave or Chrome and in Firefox (unpacked), and its menus and notifications. (where: by hand)

**82. Home Assistant over MQTT (ecosystem 1e)**

- Home Assistant itself (its Mosquitto add-on): the entities on its device page, the Activity entity's history, a button press from its UI.

**83. What the phone tells other apps (ecosystem 1f)**

- Tasker or MacroDroid receiving them (no automation app on the A71), a remote's events, a send's `FILES_SENT`, a task's `TASK_FINISHED`, a link lost (`REASON=lost`), and the Settings row on screen.

**85. iroh 1.3, one endpoint per computer, files over the internet road (2a–2d)**

- 2e: a screenshot copied on the computer lands on the phone's clipboard over the internet link (and phone → computer, when an app on the phone can copy an image). (where: by hand)

**87. KDE Connect devices (3a–3c)**

- The desktop app's Find button, list, Allow/Deny and Pair on screen.
- 3c, a text or link share refused with its notice, the low-battery warning from a KDE device, and the switches, battery and Ring on screen.

**88. The desktop app's shell (U1)**

- A notice clearing after 6 s, "Copied.", and the two settings' switches while their change runs. (where: by hand)
- A line mixing Arabic or Hebrew with Latin (egui has no bidirectional layout), read by someone who reads it. (where: by hand)

**89. The phone's navigation and feedback (U2)**

- TalkBack reading the snackbars and the checking card.

**90. Names on both sides (U3)**

- Rename on the desktop's Devices page, and the approval prompt's "(calls itself)" (they need clicks).
- A computer's name from a scanned pairing code, the session notification and launcher shortcuts by name, and "Lock <name>?". (where: by hand)

**91. Ring the phone from the computer (N1)**

- The desktop's Ring button on screen (it needs a click), Ring off in Devices refusing it, and once a minute from the button.
- Do Not Disturb with alarms allowed and not; a phone with a secure lock screen (the full-screen ring and Stop); a phone where the full-screen permission is not granted. (where: by hand)

**92. Phone → computer text in one tap (N2)**

- "To computer" in a real app's selection menu, on screen; the Sharesheet's Direct Share row showing the computers (adb opens only the app chooser).
- A copy marked sensitive (a password manager's) asking first through the tile. (where: by hand)

**93. The phone's Home card, Add form, Settings and session menu (U4)**

- TalkBack on a card ("Connect"), a Settings row (one stop) and a heading.
- Landscape: the bottom chrome with 48 dp chips. (where: by hand)

**94. The desktop's Settings and words (U5)**

- Opening and closing sections; a change shown as applying now or after a restart in the save bar, and the restart notice naming it. (where: by hand)
- A narrow window wrapping every row; a section with a problem held open. (where: by hand)
- Unpair's question for a computer. (where: by hand)

**95. The phone app in Spanish and Tamil (U6)**

- Every screen in Spanish and Tamil for text that runs out of its box. (where: by hand)

**96. Answer a phone notification in the toast itself (N3)**

- Typing a reply in the toast and pressing Send: the phone's app gets it (its RemoteInput), at most 10 a minute. (where: by hand)
- Pressing "Mark as read" in the toast, and from the Action Center after the banner has gone. (where: by hand)
- Settings "Answer in the notification" off: the toast is plain. (where: by hand)

**97. The phone as the computer's speaker and mic (N5)**

- By ear: the computer's sound on the phone (lag, quality, across the room).
- Windows with VB-CABLE: the phone's mic heard in a recording or a call on the computer.
- An hour with the screen off: the session holds, and what it costs the phone's battery. (where: by hand)

**98. An Android 17 pass (N4)**

- A screen, remote and sound session with the screen off: the computer's sound keeps playing (the session's foreground service holds while-in-use). (where: by hand) (by ear.)

**102. The phone's background cost (N7)**

- The same hour on battery, without USB: the battery's drop, and the CPU while the phone really sleeps. (where: by hand)
- Over Wi-Fi rather than `adb reverse`: the radio's wakes and bytes for the link's keepalives. (where: by hand)
- A release build (R8): the CPU again. (where: by hand)

**103. Transfers you can watch and stop (C1)**

- A folder cancelled on the phone: the app's notifications sit folded under "Silent" in the shade, so its Cancel was not reached by script (a single 1.5 GB file's was, in C1). (where: by hand)

**104. Control the phone from the computer (C8)**

- The window by hand: a click, a drag, the wheel, the right and middle buttons, typing, the bar's buttons, Watch only, and a drag that leaves the window.
- The phone locked with a PIN: taps and typing refused, Back and Home taken. (where: by hand)
- A password field. (where: by hand)

**105. Choose where received files land (C2)**

- The phone's grant taken back in Android's settings, then a file. (where: by hand)
- A download broken and continued in a picked folder. (where: by hand)

**106. Notifications that wait and go later (C3)**

- Doze on battery without USB, where the phone suspends; a notification waiting through it and arriving marked late. (where: by hand)
- "Wait up to 10 minutes" on screen, and one posted more than 10 minutes before the computer comes back.

**107. The desktop app's headless harness (C4)**

- Clicks through the app's own window that a harness cannot stand in for: the tray, drag and drop, the cast window.

**108. The phone with large fonts, landscape and TalkBack (C5)**

- A TalkBack walk by hand: Home, the Add form, Settings, the session menu (TalkBack's tutorial takes the screen when it is turned on from adb, and adb cannot read which node it reads).
- Split screen: Home and a session in half a screen. (where: by hand)
- Landscape with the keyboard open (§26). (where: by hand)
- Spanish and Tamil at 1.0, 1.3 and 2.0 (a long word such as "Individual" may already make a list at 1.0). (where: by hand)

**110. Recent photos on the computer (C7)**

- The Phone page by hand: Refresh, Save, Open and Copy. (where: by hand)
- Photo access denied: the same with its reason; and the page showing either (by eye).

**111. Health after updates (D1)**

- A real system update (the fingerprint path), and a listener that stays unbound after one.
- The card in Spanish and Tamil, and with TalkBack.

**112. Quiet hours kept in step (D2)**

- The Overview's Mute and Unmute by hand, and the refusal's words. (where: by hand)
- A Do Not Disturb that silences but still lists notifications. (where: by hand)
- A phone with a bedtime mode switching on during a mute. (where: by hand)

**113. Clipboard history and Resend (D3)**

- The Clipboard page by hand: Copy, Send to…, Clear and its confirm, a cut entry's Send greyed. (where: by hand)

**114. Continue reading on phone (D6)**

- Chrome (Windows): "Continue reading on phone" on a long article mid-way; the phone's notification opens it in Chrome scrolled to that paragraph. (where: by hand)
- Firefox: the same, the extension loaded from `about:debugging`.
- A page with a `#section` link: the fragment replaced, the page still scrolled there. (where: by hand)
- The browser's own page (`chrome://extensions`): sent at its top, no error. (where: by hand)

**115. Conversations (D4)**

- A real chat app on the A71: the thread on the Notifications page, oldest first, the owner's own lines set apart, a reply going through.

**116. The tray you act from (D5)**

- Windows, the A71 linked: its submenu titled with its battery, which changes in the menu when the battery does. (where: by hand)
- Ring, Mute for an hour and then Unmute, from the tray with the window hidden: the phone answers, the window stays hidden.
- Send clipboard: the computer's copy on the phone; a password manager's private copy not sent. (where: by hand) (the tray.)
- Send file…: the window opens on Files with the phone chosen. (where: by hand)
- The A71 away: its items greyed out. (where: by hand)
- Send clipboard with an empty clipboard, and with over 256 KiB of text: a notification says it was not sent. (where: by hand) (the tray.)
- A tray pick with the server stopped, and with a server from before D5: nothing sent, and the notice says only that it asked.

**117. The phone's folders as a drive (D8)**

- File Explorer at the Windows address: browse, open a photo, copy one out. (where: by hand)
- A file over 50 MB in Explorer (refused by Windows' limit). (where: by hand)
- Explorer's repeated looks (`desktop.ini`, thumbnails) answered without a new listing each time (the phone's log).
- The setting off: the address gone from the Phone page (by eye).

**118. Lock when the phone leaves (D9)**

- The same set from the desktop app's Devices page and Settings. (where: by hand)
- V1: a phone that walks off on mobile data, on a computer where locking is allowed: locked at the set time. (where: by hand)

**119. Memory at rest (D7)**

- The app with a phone linked and a session running, then at rest again: back to its number. (where: by hand)

**120. Answers that say how they went (E1)**

- The phone away for more than a minute, its notification still kept: "the phone did not connect within a minute". (Run 2026-10-19 with the listener off: the phone never sent the notification, so the answer was refused at once as no longer offered, before any wait.)
- From a toast (N3), the window hidden: a failure's desktop notification. (where: by hand)

**121. The tray on its own (E2)**

- Windows: starting `pocketdesk-app` starts `pocketdesk-tray` once (a second window launch starts no second tray); closing the window leaves the tray.
- Each item from the tray program: Ring, Mute and Unmute, Send clipboard; Open, Pair a phone… and Send file… with the window closed (it starts on that page, Files with the phone chosen) and with it open (it comes to the front on that page).
- With the window closed: a phone that must ask, an ask-first task, and a phone's "Show screen" each start the window (once each), with the prompt or the screen. (where: by hand)
- Quit from the tray program closes the window too; with no window open, an app started just after Quit stays open.
- An install without `pocketdesk-tray`: the app's own tray, as before.

**124. The desktop app's design pass (E5)**

- Each page at the 640 px minimum width, in both themes: no row runs off the page (Devices, KDE Connect, Activity, Notifications, Phone files, Sessions). (where: by hand)
- The Phone page with a long folder: the photos below are in reach, and every photo row shows. (where: by hand)
- The cast window's bar in the light theme: its buttons readable. (where: by hand)
- KDE Connect's Unpair… asks by name; Cancel keeps it. (where: by hand)
- Devices: a paired computer has no Ring, Mute, cast or lock switches; the lock's note shows while it is on. (where: by hand)
- Notices: "Asked the phone…" on the Phone page goes in seconds; an error shows in red. (where: by hand)
- A screen reader (Narrator) names each photo's Save, Open and Copy, the pairing code and the phone's screen.
- The tray: a phone's title says Do Not Disturb or the mute's minutes left; Send the clipboard and Pair a phone.

**125. The phone app's design pass (E6)**

- Spanish and Tamil on the changed screens. (where: by hand)

**126. The deck (F1)**

- A task button, the deck set up in the desktop app's Settings, and the icons by eye. (where: by hand)
- 2, 3 and 4 a row; a long label; Spanish and Tamil; TalkBack reads each button's label.

**127. How the computer is doing (F2)**

- The sheet's asking stops when it closes; Spanish and Tamil; font scale 2.0. (where: by hand)

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

**134. The deck on the home screen (G3)**

- The server stopped: "No answer from the computer yet" after 8 s, never a stuck "Asking…". (where: by hand) (a widget on the home screen.)

**136. The computer's music on the home screen (G5)**

- Place "Pocketdesk: playing" with music playing on the computer: its title and artist, Previous, Pause and Next as the player offers them, "Read at <time>". (where: by hand)
- Pause from the widget: the music stops, and the button turns to Play; Next: the next track shown. (where: by hand)
- The server stopped: "No answer from the computer yet" after 8 s, never a stuck "Asking…". (where: by hand) (a widget on the home screen.)

**137. The computer's music from Quick Settings (G6)**

- Add "Computer's music" to the A71's Quick Settings; with music on the computer, a tap pauses it, the shade still open showing "Asking the computer…", then the track, the tile off; a tap again plays, the tile on. (where: by hand)
- On the lock screen: a tap asks to unlock first. (where: by hand)
- Opened after an hour: "Tap to play or pause", not the old track. (where: by hand)

**138. Wake the computer from Quick Settings (G7)**

- Add "Wake computer" to the A71's Quick Settings: its subtitle names the last computer; with its MAC not known yet, "Connect once while it is awake". (where: by hand)
- A tap with the computer awake: "Sending the wake…", "Sent, waiting for it to answer…", then "It is awake"; a tap again at once is ignored. (where: by hand)
- On the lock screen: a tap asks to unlock first. With no computer saved: a tap opens the app. (where: by hand)

**139. A talk timer in Remote (H2)**

- The screen off, the phone in a pocket: the buzzes still come on time (the session's foreground service). (where: by hand)
- Do Not Disturb on: the buzzes still come; Settings "Talk timer buzzes" off: none. (where: by hand)

**140. Send the clipboard to the phone with a key (H3)**

- The tray's menu (H4): the phone it sends to shows "Send the clipboard (Win+Shift+C)"; with the key cleared, the plain label again.

**141. The home-screen widgets, a design pass (I2)**

- The music widget with a track: title, artist, the buttons at 56 dp and "Read at <time>" in its header; nothing playing. (where: by hand)
- The state widget made one cell tall: the line, no "%" left alone; on its side at two cells: the line where bars do not fit. (where: by hand)
- Spanish and Tamil (the bar labels at 68 dp); font scale 2.0. (where: by hand)

**142. The desktop app, upgrades and UI tweaks (I3)**

- Messages: half-type a reply to one conversation, open another, then New text: each its own draft; Enter sends, Shift+Enter a new line. (where: by hand)
- The notice at the foot of the window, the page not moving; a long one readable; Restart's spinner. (where: by hand)
- The page list: a count on Sessions in a session, on Files while a file moves, a warning dot on Settings while it has changes; readable selected and hovered, light and dark. (where: by hand)
- Files: Send again on a failed send (and disabled with the phone away); Open on a received picture and PDF, none on a program; the rows wrapping at the minimum width. (where: by hand)
- Overview: a phone's battery bar, in the warning colour at 15 % or under not charging; with pairing off, the warning and Open Settings. (where: by hand)
- Diagnostics: the checks running as it opens, the chips in both themes. (where: by hand)
- Pair: "For 15 minutes" makes a new code; a phone paired with it shows its time left on Devices and goes when it runs out. (where: by hand)
- Phone files: the spinner while a folder is asked for. (where: by hand)

**143. The Logs page (J1)**

- A debug-level log of several megabytes open on the page: no hitch while it grows, the newest lines at the foot. (where: by hand)
- The filter and "Warnings and errors only", each alone and together, and the count beside them. (where: by hand)
- Just after midnight, today's log still empty: the page shows the controls and "0 of the last 0 lines". (where: by hand)

**144. Find a setting on the desktop (J2)**

- Searches for "browser", "file manager", "computers" and "kde": each extra shown alone in its section. (where: by hand)
- A setting with a problem, then a search that does not find it: its section still there, whole. (where: by hand)
- The page at the minimum width, light and dark. (where: by hand)

**145. The Sessions page in blocks, with the recent ones (J3)**

- A live screen session: its block with the phone, "on this network" and "screen" chips (the address on hover), how long, End. (where: by hand)
- The page at the minimum width: the chips wrap whole, not as tall slivers. (where: by hand)
- After a session ends: it heads "Recent sessions" with its length, its road and "capture to arrival"; a refused phone reads "A device claiming to be …", its retries one line. (where: by hand)

**146. Search on Activity and Clipboard (J4)**

- Activity with a few hundred events: Search by a phone's name, a file's name and a link's host; with a kind chip; nothing found, then Clear search. (where: by hand)
- Clipboard: Search by a text and by "From …"; with Clear's question open, Esc in the box empties it and leaves the question. (where: by hand)

**147. Dropped files ask where they go (J5)**

- One phone linked: a dropped file goes to it at once, one notice. (where: by hand)
- A second drop while it asks joins the files; a folder reads as a folder. (where: by hand)
- Leaving Files, or hiding the window, while it asks: "Not sent". The phone leaving while it asks: one button, or "Not sent" with none left. (where: by hand)

**148. A sound session notices a computer gone quiet (J6)**

- Nothing playing, the screen off, an hour: never ends on its own. (where: by hand)

**149. Files keep their dates (K1)**

- A folder picked under "Files from computers go to": the file keeps its arrival date; Settings says so. (where: by hand) (the phone's folder picker.)

**150. Not from this app (K2b)**

- A phone notification's toast on Windows: "Not from this app" last among its buttons; pressed, a notification says the app is muted, and the app's next notification shows no toast but is on Activity, "muted here". (where: by hand)
- An answerable notification from a muted app: still on the Notifications page. (where: by hand)

**152. Pause the phones on the computer (K2a)**

- The tray's Pause notifications → For an hour: the menu says "Paused, 60 min left" with Resume, and the Overview says what is held.
- Resume from the tray and from the Overview; `pocketdesk pause --for 1` ends by itself in a minute.

**153. Notifications with their app's icon, and pictures (K3)**

- A phone notification's toast on Windows shows the app's icon as its logo (on screen, not only in the XML).
- With "Pictures in notifications" on in the phone's Settings: a notification with a picture (BigPicture, or a contact's photo) shows it in the toast; one the lock screen hides shows none. (where: by hand)
- The Notifications page shows each answerable notification's app icon. (where: by hand)
- Closing the notification on the phone removes its picture from `notification_icons/pictures/`. (where: by hand) (a notification with a picture.)
- Desktop Settings "Apps' icons and pictures" off: plain toasts. (where: by hand)

**154. The phone's music in the computer's media controls (K7)**

- Settings "The phone's music in this computer's media controls" on; a phone in a session plays music: the volume flyout shows its song as Pocketdesk, with its buttons. (where: by hand)
- Play/Pause, Next and Previous from the flyout reach the phone; the flyout shows the new state at once. (where: by hand)
- The session ends: the song leaves the flyout within a few seconds. Paused five minutes: it leaves. (where: by hand)
- The setting off: it leaves at once. (where: by hand)

**155. The phone's sound with its screen (K5)**

- Phone Settings "The phone's sound with its screen" asks for the microphone permission; desktop Devices "Play its sound here" on. (where: by hand)
- "Show screen" with a throwaway app playing a 1 kHz tone: a throwaway server with `POCKETDESK_PHONE_AUDIO_FILE` writes it; its pitch and level match. (where: by hand) (a tone app on the phone.)
- By ear: a video on the phone plays on the computer's speakers, behind the picture by how much.
- With another app in front on the phone (the use case): the sound still comes. (where: by hand) (by ear.)
- "Play its sound here" off mid-cast: the cast ends. Without it: the phone does not capture. (where: by hand)

**156. Cleared on the computer, cleared on the phone (L1)**

- The toast's ✕ on its banner, and inside the Action Center: cleared on the phone (by hand). (where: by hand)

**157. Drop files and paste into the phone's window (L2)**

- With "Files from the computer in the background" on: a 3 MB file dropped on the phone's window; its MD5 on the phone matches. Without it: the window says why. (where: by hand)

**158. A phone charged to a level (L3)**

- Desktop Settings "Tell me when a phone has charged to" 80 %; with Share battery on, `adb shell dumpsys battery set ac 1`, `set level 79`, then `set level 80`: one notification, "charged to 80 %"; `set level 81`: none. (where: by hand) (the phone reads its battery from the hardware, which `dumpsys battery set` does not move.)
- `dumpsys battery unplug`, then `set ac 1` again at 80: one more. (where: by hand) (a real charge.)
- `adb shell dumpsys battery reset` after. (where: by hand)

**160. Screenshot and record the phone's window (L5)**

- Screenshot: a PNG in Pictures\Pocketdesk at the cast's size, and on the clipboard. (where: by hand)
- Record for about 30 s, then Stop: an MKV in Videos\Pocketdesk; `ffprobe` gives h264 at the cast's size and a duration within 10 % of the wall clock; it plays. (where: by hand)
- Recording while the phone turns: the file so far kept, the bar says why it stopped. (where: by hand)
- The cast ends during a recording: the bar names the file and stops counting. (where: by hand)

**161. Commands to the phone's automation apps (L6)**

- A throwaway receiver in Tasker's package logs `dev.pocketdesk.android.event.COMMAND` with NAME, TEXT and ADDRESS after `pocketdesk phone-command`.
- With "Only to" set to another installed app: nothing reaches the receiver. (where: by hand) (a second automation app.)
- With no automation app installed: nothing is sent, and Settings says so. (where: by hand)
- Tasker itself.

**162. The phone's state on the computer (L9)**

- The Overview's line matches the phone's Settings: free and total storage, Wi-Fi with its bars. (where: by hand)

**163. Features on both sides (L7)**

- The same within 15 s in a session, and on the computer's Devices page. (where: by hand)
- Notification access revoked on the phone: the features the listener runs say "Android allows: no", with the advice.
- Settings "Phone notifications" off on the computer: the notifications rows name it. (where: by hand)

**166. Linked with the screen off (M1)**

- The Health card on screen: "Turn on" with the switch off, and this phone's own setting after a pause with it on; Not now.
- On battery without USB, on Wi-Fi, for an hour (N7's script): a ring from a throwaway server arriving with the screen off, CPU use and battery drain. (where: by hand)
- Renaming the computer changes the notification's words. (where: by hand)

**167. Both sides' switches, part 2 (M2)**

- The same beside the desktop app's Devices page, by eye. (where: by hand)
- Paired as "Someone else's": a file sent from the phone lands; the remote browser and Download this folder are refused; a session waits for Allow; a clipboard push is refused. (where: by hand) (the Pair page.)
- Paired from the phone's "Show PIN on desktop" while the Pair page says "Someone else's": the same narrow permissions. (where: by hand) (the Pair page.)

**169. Photos keep where they were taken (M5)**

- Photo access revoked first, then the switch turned on: Android asks for photo access, and the switch stays on. (where: by hand)
- A JPEG with GPS tags pushed to `DCIM/Camera` by adb, shared from the gallery to a throwaway server with the switch on: the tags arrive (an EXIF reader); with it off, they are gone.
- The same photo fetched through recent photos (C7), and in a folder send: the tags kept. (where: by hand) (photos with GPS tags.)
- Android 14's "selected photos only": the row says access is needed, and photos go as before. (where: by hand)
- A share from the Files app (`com.android.providers.media.documents`), and one from Google Photos. (where: by hand)

**171. Type on the phone from the computer's keyboard (M4)**

- Back, Home and Recents from the window, in turn with text typed before them. (where: by hand) (Pocketdesk control's Accessibility service, turned on by hand.)
- A password field: nothing typed, and the notification says so. (where: by hand) (Pocketdesk control's Accessibility service.)
- Stop: nothing typed for 10 minutes. (where: by hand) (Pocketdesk control's Accessibility service.)
- The switch off, or the app's notifications off: nothing typed. (where: by hand) (Pocketdesk control's Accessibility service.)
- The phone unlinked while the window is open: the window says why and sends nothing. (where: by hand)

**172. The phone's sound without its screen (M7)**

- The OPPO with the throwaway tone app against a throwaway server with the WAV seam (`POCKETDESK_PHONE_AUDIO_FILE`): the tone's frequency and level; no cast window (the relay's log says nothing); the Sessions page says "its sound here". (where: by hand) (a tone app on the phone.)
- "Play its sound here" off for the phone: refused, the phone saying why; turned off mid-session: it ends. (where: by hand) (the casting consent.)
- Stop from the phone's notification, and End on the Sessions page. (where: by hand)
- Started while the screen is shown: refused before Android's consent, saying why. (where: by hand)
- An hour with the screen off: the battery. (where: by hand)
- By ear, and a real music app (the owner).

**174. Home Assistant buttons for phone commands (M9)**

- A local broker as in 1e: the buttons announced under the phone's device; a press published with `mosquitto_pub` reaches the OPPO's throwaway receiver in logcat. (where: by hand) (an MQTT broker.)
- A retained or empty message on the command topic: nothing sent. (where: by hand) (an MQTT broker.)
- The phone's "Send commands to its automation apps" off: refused, in the server's log. (where: by hand) (an MQTT broker.)
- A command removed in Settings: its button goes from the broker; another command keeps its id. (where: by hand) (an MQTT broker.)
- Home Assistant itself, and Tasker (the owner).

**175. Copies sent by themselves (M10 part 2)**

- The switch off: nothing watched, nothing read (`dumpsys accessibility` shows no event types). (where: by hand) (Pocketdesk control's Accessibility service.)
- The keyboard and selection in the app copied from, after the read (focus moves away and back). (where: by hand)
- An hour with the switch on: the battery. (where: by hand)

**176. The phone's music without a session, with its cover (N2)**

- A real player (Spotify, YouTube Music) with the screen off, an hour: its cover, presses, and the battery by N7's script.
- Windows' media controls with the tray program running: the phone's song and cover in the volume flyout, the media keys (with the owner).
- The Overview's music line and buttons, and the tray's menu, by hand.
- Next and Previous from the computer. (where: by hand) (a real player on the phone.)
- Media turned off and on for the phone on the Devices page mid-track: the cover comes back at the next track. (where: by hand)
- Two sessions in one app (a browser's two tabs): the cover of the one reported. (where: by hand)

**177. Open on the phone (N3)**

- Without Pocketdesk control: "Tap to open" on the phone, its tap opening the app, gone after ten minutes or when the notification goes. (where: by hand)
- The switch off on the phone: "opening is off on the phone". (where: by hand)
- A real chat or mail app's notification, and one whose app blocks a background start (another maker's limits). (where: by hand)

**178. The phone's ongoing updates on the computer (N4)**

- A real download or upload (Chrome, Drive), a timer (Clock) and a delivery or ride app, by eye on the toast: no pop-up after the first, the words changing.
- Closing the toast mid-download: it stays closed; the card stays on the page. (where: by hand)
- The phone in hand at the first update: the toast comes once it is put down. (where: by hand)
- The battery with a long download. (where: by hand)

**179. The computer's music in the phone's media controls (N5)**

- The lock screen and the shade's media controls by eye: title, artist, cover, and Previous, Play/Pause, Next as the computer's player offers them.
- A real player on this computer (Spotify, a browser), with the owner: a press from the phone plays, pauses and skips it; never Pocketdesk's own entry (the phone's music) sent back.
- The link dropped: the session gone within 150 s; back by itself when the link returns. (where: by hand)
- Audio turned off for the phone on the Devices page: nothing more comes, and the session ends. (where: by hand)

**180. Files into the phone's folders from the computer's drive (N6)**

- Unmarking: the write grant given back. (where: by hand)
- File Explorer with the drive mapped: copy a file and a folder in (Windows' empty file first, then its content), a large file past 45 s (answered 202, then on the phone), a name taken (the next free one). (where: by hand)
- Desktop Settings "The phone's folders take files" off: the drive read-only again. (where: by hand)
- Explorer's rename, delete and "New folder" (MOVE and DELETE are refused): what Explorer says. (where: by hand)
- A folder that is also where files from computers go: unmarking it leaves downloads there working. (where: by hand)

**181. Toasts only on the computer in use (N7)**

- The battery with the links kept, by N7's background-cost script. (where: by hand)

**182. Leftovers: typing from the tray and a key, several links, the phone nearly full (N9)**

- The tray's "Type on the phone" for a linked phone: the typing window opens (both trays).

**184. Automation apps start the computer's tasks and press its deck (O1)**

- Tasker itself: a Send Intent to the receiver, and Intent Received for the events.
- A task that asks first, allowed at the desk. (where: by hand)
- Over the internet road, cold. (where: by hand) (Tasker.)

**185. A screenshot of the phone and its screen locked from the computer (O2)**

- The Overview's and the tray's buttons by hand, and their words when the phone is away or the permission off.

**186. A paired computer found again at a new address (O3)**

- A card saved by name (`mypc.local`) is left alone. (where: by hand)

**187. Notifications shared by their kind as well as their app (O5)**

- A real chat app's kinds listed under it, by their titles, and one kept on the phone while its messages go.
- TalkBack reads each kind's switch with its kind.

**188. Copies kept on the computer by program or by look (O4)**

- A copy made in KeePassXC (or another listed program) on Windows: not on the phone, not in the history, not sent by the tray.
- A copy, and an image, from a listed program held the same. (where: by hand)
- The two Settings rows by hand, "Add the usual password managers" and "Add keys and tokens". (where: by hand)

**189. Find a setting on the phone (O8)**

- TalkBack reads the box and its Clear; Spanish and Tamil search their own words.

**190. Received images onto the clipboard (O7)**

- A photo from the phone, with "Images sent here onto the clipboard" on: pasted into Paint upright (a portrait photo turned by its EXIF), and the file still in Downloads. (where: by hand)
- A PNG from the computer, with "Images from the computer onto my clipboard" on: pasted into a chat app, and not sent back by the tile or a session's clipboard sync. (where: by hand)
- Either switch off, or a folder of images, or a GIF: the clipboard unchanged. (where: by hand)

**191. Choose the computer's player (O6)**

- Two real players on Windows (a browser tab and Spotify): the one chosen takes Play, Next and Previous from the Sound sheet, the widget and the lock screen. (where: by hand)
- The chosen player closed: the card reads "The computer's choice", and the choice comes back when the player opens again. (where: by hand)

**194. Open an app on the phone from the computer (P2)**

- An open with the control service on: the app in front at once, also with the screen locked (refused) and from the background. (where: by hand) (Pocketdesk control's Accessibility service.)
- The Overview's "Open an app on the phone…" by hand: the search, "No answer from the phone" with the phone locked, Open greyed on an old list. (where: by hand)

**195. Quiet hours, and quiet while presenting (P3)**

- A notification under quiet hours with the phone locked: no pop-up, on Activity and, when answerable, on the Notifications page. (where: by hand)

**196. Notifications held by their words (P4)**

- The Settings box by hand: its warning for a line ending with a colon, and Activity naming the line with "Details in events" on. (where: by hand)
- An update held by its words closing the toast its first showed. (where: by hand)

**197. A phone's ringing alarm on the computer (P5)**

- The Windows alarm toast by ear and eye: its sound, Snooze and Stop pressed from here, Dismiss, its close when the phone stops it, and its end after ten minutes.

**198. Files to several phones or computers at once (P6)**

- The Files page by hand: "Also to…", "Every one", "And 2 more…", a drop's "All of them", the notice when one has left. (where: by hand)

**199. Files copied in Explorer sent with the clipboard key (P7)**

- On the LAN: two files copied in Explorer, the tray's Send the clipboard and the key each sending them, rows in the sent list.
- The Files page's "Paste copied files", one target and several; more than 100 copied. (where: by hand)

**200. Links by site (P8)**

- The Settings box by hand: its warning for a bad line, a .bat refused. (where: by hand)

**201. Permission profiles (P9)**

- Devices page by hand: Save as profile, Replaced, Apply a profile (a narrowed one ending the phone's session once). (where: by hand)
- Pair page: a profile chosen, its words, a new code; a phone paired with it gets those permissions; the profile removed before the scan pairs as someone else's.

**202. A ring that vibrates and flashes (P11)**

- Stop and a dismiss ending both at once. (where: by hand)
- Each switch off: no shaking, no torch; the camera open in an app: the torch left alone, the ring going on.

**203. The phone's contacts on the computer (P10)**

- The Messages page by hand: Ask, the search, Text filling the number, Copy, Call (and with Dial off), "No answer" with the switch off. (where: by hand)

**204. The desktop's O and P pages after their design pass (Q2)**

- Devices by hand: Apply a profile's question (what it turns off, the session ending), Remove a profile…, Save as profile without a name, a broken profiles file said; the Pair page's choice wrapping with a long name. (where: by hand)
- The Overview: the line under a phone's buttons with its "Let the computer take screenshots and lock this phone" off; quiet hours' group and Change quiet hours. (where: by hand)
- V6: with only "Pocketdesk control" off on the phone, the line under its buttons names Accessibility and not notification access. (where: by hand)
- Files: a link's notice with "Also to…" ticked, "And 2 more…"'s hover, Paste copied files while reading and with several phones to choose from. (where: by hand)

**205. The phone's O and P rows after their design pass (Q3)**

- Settings' search with "This phone's notifications" off: "alarms" and "timer" find Alarms on the computer greyed, and the camera's "Also onto its clipboard".
- My contacts on the computer: the permission refused (the line and Android's page), then granted there; without notification access, its warning and link. (where: by hand)
- The Devices page and the phone's "What this computer allows" naming the five new features. (where: by hand)
- A kind of notification's row, Spanish and Tamil on screen, font scale 2.0, TalkBack across the moved and changed rows.

**207. Start beside KDE Connect (T1)**

- A phone with a card saved at the computer's LAN address finding it on the moved port by itself (the toast), and its tailnet card following. (where: by hand)
- Windows: `pocketdesk install` opening TCP 1716-1725, and a phone reaching a server moved to 1717 through the firewall. (where: by hand)
- The desktop app's Overview label and Diagnostics with the port moved. (where: by hand)

**209. New camera photos to the computer (T3)**

- On a LAN (not `adb reverse`): photos and a video landing in the computer's received files, matching their bytes. (where: by hand)
- The phone's own camera: a burst of 20, a 4K video, and photos taken while the computer was off going when it is back. (where: by hand)
- Photo access taken back: the switch off and Home's notice; video access taken back: "Videos too" off. (where: by hand)
- The computer's Devices page listing "Its new camera photos here"; Settings in Spanish and Tamil. (where: by hand)

**210. The whole photo gallery (T4)**

- On a LAN: Save the chosen landing, a video opening in the player, a HEIC photo's PNG on the computer's clipboard. (where: by hand)
- The desktop app's Phone page by hand: More photos, the ticks and their limit of 20, a page of screenshots only. (where: by hand)
- "Its videos too" asking for video access; Settings in Spanish and Tamil. (where: by hand)

**211. Files to a KDE Connect device (T6)**

- KDE Connect on Android, paired: a file from the Files page and from `pocketdesk send <device>` lands in its Downloads, its name and date kept; two files one after the other. (where: by hand)
- The device not connected: the send fails in the Sent list with why; a folder refused with why. (where: by hand)
- Windows: `pocketdesk install` opening TCP 1739-1764, and a device coming for a file through the firewall. (where: by hand)

**214. Driving another computer from Windows (shared input)**

- The tray's menu: "Keyboard and mouse: this computer", then the laptop's name within about a second of crossing; no line with sharing off or the server stopped. (where: by hand)

**215. The shared keyboard and mouse: the computer driven, the arrangement, the clipboard and the latency (shared input)**

- The Keyboard and mouse page's links and permission line, both ways: "Won't take input: sharing off there" when sharing is off on the laptop, and "may drive" switched from the computer that pasted the link. (where: by hand)

**221. No lag while sharing (BK10)**

- The setting in the desktop app's Keyboard and mouse section on Windows and Linux; turned off, the next `share-test` says "the link kept quick: no". (where: by hand)
