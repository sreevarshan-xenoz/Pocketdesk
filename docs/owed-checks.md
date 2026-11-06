# Owed checks, by where they can run

Generated from `docs/device-verification-checklist.md` by `scripts/owed_checks.py` (C6); CI fails when it is stale. Each open item is placed by its own words and its section's, the first place that matches; a line can say `(where: <place>)` itself. Tick items in the checklist, then run the script again.

1436 open items.

| Where | Items |
|---|---|
| Here: this Windows box and the test phone (the OPPO), without touching the owner's power, lock or input | 101 |
| Power, lock or server stop on the owner's Windows box: never run here (the owner's rule) | 19 |
| Needs the owner's OK on this box: input reaching Windows, or its display settings | 205 |
| A phone with a SIM | 40 |
| A second phone | 19 |
| Away from home: another network, or through a router | 43 |
| A second computer | 249 |
| A Linux desktop (the Azure VM serves for some) | 265 |
| Other hardware: a foldable, tablet, mouse, game pad, older or newer Android | 99 |
| By hand or eye on this box: hearing, looking, the tray, drag and drop, a screen reader, a real app | 396 |

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

**236. The rest of the pages (Desktop UX 4)**

- With the OPPO paired and a session or two over: Sessions lists them as cards with a grey dot (amber for a refused one), and a live session's card has a green dot; Files lists a sent and a received file as cards with their dots. (where: OPPO)

**242. One version across my devices**

- The OPPO on this build with a throwaway server on this build: after a connection, the computer's card on Home says "runs <this release>" in the muted colour, and the server's `pocketdesk devices --json` lists the phone's `version` as the app's. Then the phone on 0.8.2 from F-Droid or a release APK: the card says nothing (the server on 0.8.2 sends no release), and a 0.8.2 phone against this build's server leaves the phone's `version` null. (where: here) Half checked 2026-10-31: A's card says "runs 0.8.2" in the muted colour and A's `devices --json` lists the phone's `version` 0.8.2. Not run: a 0.8.2 phone from F-Droid or a release APK (a differently signed APK needs an uninstall, which wipes the phone's pairings). 2026-10-31 second batch: this build's phone against a 0.8.2 throwaway (built from the tag on the laptop): the card shows no release line, and "runs 0.8.3" for the 0.8.3 throwaway. Still not run: a 0.8.2 phone.
- A throwaway server whose `pocketdeskVersion` differs from the phone's (a build from a commit with the workspace version raised, say 0.9.0): the card says "runs 0.9.0, newer than this app: update this app" in the tertiary colour; with Settings → Pairing's switch off it says "runs 0.9.0" only. Its Devices page lists the phone with "older than this one: update it" in the warning colour, and plainly with Settings → Phones' switch off. (where: here) Phone half checked 2026-10-31 against B built at 0.9.0: "runs 0.9.0, newer than this app: update this app" in the tertiary colour, "runs 0.9.0" alone with Settings → Pairing's switch off. B's `devices --json` lists the phone at 0.8.2; its desktop Devices page was not opened.

**252. The phone's camera as this computer's webcam (Linux)**

- Stop on the phone's "Your camera is in use" notification ends the cast and the node goes. (where: here) Partly checked 2026-11-01: the Stop action could not be reached by the script: ColorOS's shade shows the notification as a one-line row inside Pocketdesk's group, without its buttons (also with only two Pocketdesk notifications left), and `CastService` is not exported, so adb cannot fire the action's own intent. Nothing else was learned; Stop by hand on the phone is still owed.

**255. The phone's location for this computer's apps (GeoClue, Linux)**

- The OPPO for real: Settings "Give my computer my location" asks for location and turns on; with a throwaway server and `where-am-i -a 4` (city) the computer logs "fix sent, accuracy 1000 m" or more and `where-am-i` reports a place within about 1 km of the phone's true one (compare by distance only, never print either place); Activity shows "city location, accurate to N m". (where: here) Found, not fixed (2026-11-01, the OPPO, a throwaway on the laptop with avahi-daemon started for the run and a client run as the `geoclue` user standing in for GeoClue, which is not installed there; print nothing of any place): (1) The phone's answer never reaches the computer: `RustCore.sendPhonePacket(..., FIX, ...)` ends in the bridge's `send_phone_packet`, which refuses any type not in `PHONE_PACKETS` (`android/bridge/src/api.rs`), and `kdeconnect.pocketdesk.phone_location` (`phone_location::FIX`) is not in that list. So every answer, a fix or a refusal (`off`, `permission`, `background`, `unavailable`), fails on the phone (log "could not answer the computer's location ask") and the computer logs "no fix (the phone did not answer)" after the full 30 s. Fix: add `phone_location::FIX` to `PHONE_PACKETS` (the array's length changes to 18) and a test that every packet type the app sends is in the list. (2) Probably: a fix that does not come hangs the phone's answers for good. `fetch` cancels `getCurrentLocation` after 20 s, and cancelling a `LocationManagerCompat.getCurrentLocation` does not call its consumer, so `done` never runs and `inFlight` stays true: from then on `answer` returns silently at `if (!inFlight.compareAndSet(false, true)) return` (seen: after one ask with location services on and no fix to be had, three later asks over six minutes left no line in the phone's log, and the computer logged "no fix (the phone did not answer)" each time). Fix: finish with `done(null)` on the timeout path itself (and reset `inFlight` there), not through the consumer. Not run past these: no fix could be had (the phone has no SIM and was indoors), so "fix sent, accuracy N m" and "Activity shows city location" were not seen.
- The same with the computer on Exact and the phone's "Exact location" on (accuracy of tens of metres, GPS), and with only one of the two on (the phone still gives the city). (where: here) Not run 2026-11-01: blocked by the two bugs above (no answer reaches the computer, and no fix was to be had).
- The phone in the pocket with the screen off: with "Allow all the time" the ask is answered; without it the phone answers `background` and the computer logs "no fix" and tries again a minute later; with Pocketdesk open it is answered. (where: here) Partly checked 2026-11-01 (the phone's own decisions, from its log; the answers themselves did not arrive, see the first item): with "Give my computer my location" on, only the approximate-location permission (no "Allow all the time") and location services off, an ask with Pocketdesk not on screen was decided `background`; with Pocketdesk open the same ask was decided `unavailable`. A real fix with the screen off, and with "Allow all the time", was not seen.
- The switch off on the phone answers `off` at once (no wait of 30 s); location permission taken away in Android settings answers `permission`; location turned off for the phone answers `unavailable`. (where: here) Partly checked 2026-11-01: the switch off was decided `off` within 1.0 s of the computer's ask (the phone's log), and location services off was decided `unavailable` (Pocketdesk open); but neither answer reached the computer (the first item's bug 1), so the computer waited the full 30 s. `permission` was not tried: revoking a runtime permission by adb ends the app's process, and Pocketdesk's control service must not be cut off.

**259. Hands-free apps: a paused share, the screen-sharing question accepted, a lock swiped away**

- The window-state event of the question names the activity Pocketdesk's intent names (`ConsentPress` presses nothing before it): the log shows "pressed" only after it; a second system dialog raised by another app at the same time is not pressed; a button whose words are not a known confirm word (set the phone to a language not in the list) is not pressed and the page says a tap is needed. (where: OPPO)
- The question was raised for the entire screen on Android 14 and later, with no one-app choice (the question has no chooser); and if the OEM's question has a chooser, entire screen was picked before Start, and a chooser reading as one app was never confirmed. (where: OPPO)
- Nothing else is ever pressed: with the opt-in on, open a second system dialog (a permission question) during the five seconds, and with no open asked at all, raise Android's screen-sharing question from another app; neither is touched, and with the opt-in off nothing is pressed even for an open. (where: OPPO)
- The pause is kept only with the apps switch and the opt-in on and for the phone's own computer: turn either off while paused and the notification goes and the projection is given back within about two seconds (the status bar's sharing indicator goes); unpairing the computer does the same. Whether the OPPO also ends a paused projection when the phone locks (the guide and SECURITY do not rely on it). (where: OPPO)
- A swipe-only lock (set the lock to "Swipe" in Android's settings), screen off: an open from the Phone apps page wakes the screen, swipes the lock away and opens the app; the log (`KeyguardDismiss`) says "the lock screen was dismissed". With the lock set to a PIN: the page says the phone has a lock Pocketdesk never bypasses and names Extend Unlock / Smart Lock / Trusted devices, the phone stays locked, and the screen is not woken by the ask; a lock set to a PIN a moment after an open is asked is given up on within about 2.5 seconds. (where: OPPO)
- Extend Unlock: with a trusted device or place keeping the phone unlocked behind a lock screen that still shows, an open swipes it away and opens the app; with the trust gone (PIN needed), it is refused as above. (where: OPPO)

**260. Phone folders kept on the computer, by themselves (Settings, Folders kept on my computer)**

- Add a folder: "Add a folder" opens Android's folder picker; no permission dialog about all files or storage appears, and the app lists the folder with "Not run yet". Adding the same folder twice makes one row. (where: OPPO)
- "Run now": the files land under `<root>/<phone>/<folder>/` with the same paths; the Files page's Kept folders row shows the same count and total bytes as the test folder, and a hash of each file there equals the phone's (generate the files from a seed so both sides can be recomputed). (where: OPPO)
- Run again with nothing changed: nothing is sent (no transfer notification stays in the shade, the phone's log says 0 sent). Change one file, add one, run: only those two go, the changed one replacing its older copy (no `(1)` copy), and its date is the phone's. (where: OPPO)
- Kill mid-file: with the 200 MB file part way (turn Wi-Fi off, or force-stop the app), then run again: the transfer continues from where it stopped (the server's log says "Continued ... from N of M bytes") and the final file's hash is the phone's. (where: OPPO)
- Mirror deletions off (the default): delete a file on the phone, run: it stays on the computer. With the phone's switch on and the computer's `kept_folders_allow_delete` off: it stays, and the Files page says one file was kept although deleted. With both on: it is deleted on the computer, and the folder it leaves empty goes too. (where: OPPO)
- The guards: with both switches on, hide the phone's storage from the app (revoke the folder's access in Android's settings) and run: nothing is deleted on the computer, and the row says the access is gone; remove 30 of 40 files at once, or empty the whole folder, and run: the deletions are held and the row says so; with the phone's guard bypassed by hand (a computer-side check: a run that names 5 of 6 files, sent by a stub), the computer refuses it and the Files page says so. (where: OPPO)
- An emptied file: empty one file on the phone (keep its name) and run: the computer's copy is emptied (size 0), not left with its old content, and a file that is empty on the phone from the start is made empty there. (where: OPPO)
- The size cap: set "Size cap for kept folders" to 1 GB and keep more than that: the run stops with the notice on the phone ("the computer's kept folders are full"), the Files page row shows the problem, and one desktop notification came, not one per file. Raising the cap and running again finishes it. (where: OPPO)
- A phone that is not one of this computer's owner phones (a guest's pairing) is refused ("Kept folders take only the owner's own phones") and nothing is made on the computer. With "Phones may keep folders here" off, every run is refused with its reason on the phone. (where: OPPO)

**261. Phone apps on the computer's Start menu, from notifications, and dropped files shared into the app in front**

- The pin with the phone away or the phone's switch off: the shortcut says why on the main window's notice line, never opening anything; with the apps list never fetched it says to open the Phone apps page once. After the phone app is restarted (its list is forgotten), the pinned shortcut still opens the app: the list is asked for again and the open follows once. (where: OPPO)
- A notification with a reply or button (a chat) on the Notifications page: "Open app on the phone" opens that chat app and brings up the screen window; with the phone off or unlinked the button is grey and says why; a notification of an app the phone does not list has no such button. (where: OPPO)
- Drop a PNG on the phone's screen window with a notes or messaging app in front: the file arrives, Android's share screen for that app opens (never sent by itself), and the window's line says "Shared ... into <App>". With an app in front that takes no images (a calculator): nothing opens, the file is in Downloads/Pocketdesk, and the line says so. With the launcher in front: "could not tell" and Downloads. Two files dropped at once: both go to Downloads, none is shared. (where: OPPO)
- Names: a file named with a colon, a Windows device name (`NUL.txt`), a hidden file and a trailing-dot name on the phone are left out (the phone's log counts them) and the run goes on. A folder renamed on the phone keeps its place on the computer. (where: OPPO)
- Remove a folder in Settings: its row goes, the computer's copy stays, and Android no longer lists the folder in its granted folders for the app (`adb shell dumpsys package` shows no persisted grant for it) unless Folders the computer may open still shares it. (where: OPPO)

**262. A spare phone at home as the wake helper (Settings, Wake helper for this home)**

- Removing: Remove on the desktop's Settings, Wake: the helper phone's switch goes off at its next link with the computer (within a minute) and a wake through it fails. It does not come back by itself: after 20 minutes it is still off. Turning the switch on again on the phone makes it the helper again. (where: OPPO)
- Unpairing the helper phone from the computer: it is no longer the computer's helper at the next check (the desktop's row says none) and its duty is gone. (where: OPPO)

**263. Phone apps in windows of their own, several at once (Settings, Separate windows (Shizuku))**

- Shizuku not running: the phone's Settings has no "Separate windows" row at all, and the computer's Phone apps page says apps open on the phone's screen; the Phone apps page opens an app as before. (where: OPPO)
- Shizuku running, switch off: the row is shown and off; the page still opens apps on the phone's screen. Turn it on: Shizuku's own permission dialog appears once and nothing else; after Allow the row says on; after Deny the switch stays off and nothing is asked again at once. (where: OPPO)
- Open a second, third and fourth app: four windows at once, each its own app and stream; a fifth says the phone already shows four and opens nowhere; closing one lets it open. The page says "N of 4 open". (where: OPPO)
- Close a window on the computer: the app's display goes (the phone's `dumpsys display` no longer lists it) and the encoder stops within a second or two; the last window closing ends the phone's notification after a moment. Quit the desktop app with windows open: the phone closes them about 30 seconds later. (where: OPPO)
- Turn the switch off with windows open, then separately stop Shizuku, then turn off "Let the computer open my apps", then unpair: each gives every window back within a second and the computer's windows close. (where: OPPO)
- A foreground service start from the background: with Pocketdesk not in front and the screen off or on another app, a window ask starts the cast service; if Android refuses, the page falls back to opening the app on the phone's screen (`windows_unavailable`) and the log says why. The notification says "Showing app windows on <computer>". (where: OPPO)
- The deny-list: Settings, the package installer, the permission screens, Play Store, the Shizuku manager and Pocketdesk, asked for in a window from the Phone apps page, are not opened in a window (the page opens them on the phone's screen under the usual rules, or says so); in a session of windows alone a screen-control batch from the computer does nothing to the phone's screen. (where: OPPO)

**265. Keys to taps: a phone game played from the keyboard and mouse (Settings, Keymaps for phone apps)**

- A stick: the Stick tool, WASD: W puts a finger down in the stick's middle and moves it up by its reach, W with D moves it on a diagonal of the same reach, letting go lifts it from where it was; opposite keys together leave the finger in the middle; the reach slider changes how far it goes. The stick is round on a tall phone window. (where: OPPO)
- Without Shizuku nothing changes: with the phone's "Separate windows" off or Shizuku stopped, apps open on the phone's screen, its window shows no keys, and the bar says keys to taps works in an app window only; stopping Shizuku with keys held gives the windows back within a second and nothing stays down. (where: OPPO)

**266. The computer turns on the phone's hotspot (Settings, Let my computers turn on my hotspot; Overview, Use my phone's hotspot)**

- Off by default and quiet: with the switch off (or on a phone without Shizuku running) the phone's Settings shows no hotspot row (or the row off), the Overview shows no hotspot buttons for it, and an `hotspot_on` ask written by hand into `phone_actions_out/` is logged by the phone as "not done: OFF" and starts nothing. A phone on Android 10 or older shows no row. (where: here)
- Turning the switch on asks Shizuku's own dialog once; with Shizuku running and Pocketdesk allowed, notification access on, the row says "On" and, within a minute, the Overview shows both buttons for the phone (the phone's switches report says `hotspot` on with android true). Stopping Shizuku greys the buttons with the reason in a minute. (where: here)
- The computer connects: another device or this computer joins the phone's network with its own saved password and has internet through the phone's mobile data (open a page, or `ping` an address by number). (where: here)
- Turning it off: "Turn its hotspot off" stops it within about 10 seconds and the indicator goes; the same with the switch off (an `off` ask is taken with the switch off); stopping the tethering manager's call by making it fail (the fallback) runs `/system/bin/cmd wifi stop-softap` only (see the helper's log), and nothing else. (where: here)
- The phone's own switch turned off while the computer's hotspot is on turns that hotspot off at once; a hotspot the owner turned on themselves is not turned off by the timer or by the switch. (where: here)
- The auto-off: with 15 minutes chosen (tap the row to change it; the values go round 15, 30, 60, 120, never), a hotspot the computer turned on is turned off by the phone about 15 minutes after the last ask to turn it on, a second ask meanwhile renews the time, and "never" leaves it on until asked, turned off, or the 4-hour cap. The phone's process killed in between (swipe the app away) still turns it off at the next tick after it restarts. (where: here)
- A locked phone with a PIN or fingerprint lock still turns the hotspot on and off when asked, with the screen off; nothing appears on the phone's screen but Android's own indicator. (where: here)
- The rate limit: nine asks (on and off together) in ten minutes: the first eight go, the ninth is logged "TOO_MANY" and does nothing; ten minutes later it goes again. (where: here)
- No other command: while turning it on and off, `ps` and logcat on the phone show no `cmd wifi start-softap`, no shell and no hotspot name or password anywhere in the helper's command lines; the computer's Activity and the phone's log never show the hotspot's name or password. (where: here)
- A stop that fails is never forgotten: with the computer's hotspot on, stop Shizuku and press "Turn its hotspot off" (or turn the phone's switch off): the hotspot stays on, nothing is cleared, the log says the stop did not go through and tries again about every 15 seconds; start Shizuku again and allow Pocketdesk: it goes off within about 15 seconds. (where: here)
- The hard cap: with "never" chosen for the auto-off, a hotspot the computer turned on goes off 4 hours after the first ask of that run, and asking again at 3 hours does not move it. (where: here)
- The owner's own hotspot: turn the hotspot on yourself in Android, then press "Use my phone's hotspot": nothing is started, and neither the timer, the cap nor the switch ever turns it off; only "Turn its hotspot off" from the computer ends it. A USB tether you started does not stop the hotspot from being started. (where: here)
- A helper that hangs: with Shizuku stopped half way (or its service unresponsive) an ask ends within about 45 seconds and the next ask works (nothing stays "busy"). (where: here)
- The Settings rows by hand: the phone's switch, summary and auto-off row (light, dark, narrow, Spanish and Tamil), and the desktop's Phones row on Linux (the field limits at 32 characters; the row is not drawn on Windows). (where: here)

**268. A still screen sends nothing to another computer's window (the window's stats, a still desktop)**

- The phone is unchanged: the OPPO against a throwaway server on a still screen still gets its frame every second (the phone's decoder reads 10 silent seconds as a dead link), a session that stays up for a minute, and the server's log says `no_idle_repeats=false` for it. (where: here)

**269. VA-API encodes each frame as it comes (phase 1b of the window handoff: `async_depth` 1, a yield after each packet send)**

- The phone on a real device is unchanged or better: the OPPO against a throwaway server on this laptop's VA-API encoder at 1080p60 and 1080p30, H.264 and HEVC (`allow_hevc`): a smooth picture for a minute, the HUD's delay no higher than before, no drop in the frames a second, a keyframe after a pause and a rejoin comes within about 2 seconds, and the server's log has no "ignored async_depth" warning. (where: here)

**271. VA-API converts BGRA to NV12 on the card (phase 1c of the window handoff: `scale_vaapi`, no processor conversion)**

- The phone on a real device: the OPPO against a throwaway server on this laptop's VA-API encoder at 1080p60 and 1080p30, H.264 and HEVC (`allow_hevc`): a smooth picture for a minute, colours as before (compare a known test picture on the phone's screen with the old build's), the HUD's delay no higher, and a keyframe after a pause and a rejoin within about 2 seconds. (where: here)

**278. A borrowed window takes the size of its window (resize follows the viewer, window handoff)**

- The setting off with a window already borrowed: the open window stops following at once and keeps its size; turned back on, it follows the window's size again without reopening. (where: here)
- The pointer locked, or the mic sent, in a borrowed window that is then resized (a tiling window manager, a keyboard shortcut): the lock stays and the mic carries on across the redial, with no click and no word on the bar. (where: here)

**276. Smooth keyframes (low-latency window handoff, phase 1: no whole keyframe on a two second beat)**

- The OPPO as the viewer of a computer with the setting on (VA-API and the software encoder in turn): the picture starts at once, plays as before, and a stalled link that comes back shows a picture again within a few seconds (the computer's keyframe on request, `RequestKeyframe`); say how long a "decoder took no input" skip waits for the next whole keyframe (10 s on VA-API, 30 s on the software encoder: no phone asks for one then yet). (where: here)
- A phone that joins mid-session, and one that reconnects after a drop, with the setting on: each gets a picture as fast as with it off (every connection opens its own encoder, which starts on a keyframe). (where: here)
- A phone's own screen cast to the computer (the phone is the encoder): unchanged by this setting (its keyframe every five seconds and on request). (where: here)

**277. Zero-copy capture (low-latency window handoff, phase 2: the screen goes from the compositor to VA-API as a DMA-BUF)**

- The Settings row "Copy the screen through memory" on this Windows box's app: it shows under Phones, saves with the rest, and says the server reads it at its start. (where: here)

**279. Dragging a window past the edge (phase 4 of the window handoff)**

- The window following the pointer while the button is held (the viewing window moving with the pointer until it is let go): not built. The window appears when the button is let go. Wayland gives a program no way to move its own window; X11 and Windows could. (where: here)
- The Settings rows "Drag windows to my other computers" and "What starts a window drag across the edge" under Other computers on a Linux computer: they save, apply within a couple of seconds without a restart, and are not drawn on Windows. (where: here)

**280. A Windows computer lends one of its windows (phase 5 of the window handoff: Windows as the source)**

- The Settings row "Let my computers borrow my windows" on a Windows computer: drawn, saves, takes effect without a restart, and its note says what happens on Windows (the window stays where it is, is raised and used with the real pointer and keyboard, may be resized, and is put back as it was). (where: here)

**282. Zero-copy capture on the PipeWire path (low-latency window handoff, phase 2b: GNOME's and KDE's screen goes from the compositor to VA-API as a DMA-BUF)**

- The Settings row "Copy the screen through memory" on this Windows box's app: its help now names GNOME and KDE. (where: here)

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

**257. A direct link on Windows: the Mobile Hotspot**

- The server killed with a link on (`taskkill /F` on the server only, and only when the owner says so) and started again: its first start's log says it swept one, the hotspot is off, and the owner's settings are back. A record that could not be put back (the internet unplugged during the kill) stays and the next start retries it. (where: power)

**262. A spare phone at home as the wake helper (Settings, Wake helper for this home)**

- Survives: reboot the helper with the switch on and on the charger: the service starts after boot (Android may refuse a start from the background: it starts the next time the app is opened, and the notification says why it waits); on ColorOS 14 the helper keeps answering with the screen off for an hour on the charger. (where: OPPO)

**263. Phone apps in windows of their own, several at once (Settings, Separate windows (Shizuku))**

- Lock rules: with a PIN lock on, an open for a window is refused like any open (the page says locked and to unlock or use Extend Unlock) and no display is made; after unlocking and opening a window, lock the phone: the window goes on. (where: OPPO)

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

**235. Pointer lock and relative mouse (PC7)**

- On a test machine (never the owner's box), run `POCKETDESK_POINTER_TEST_MACHINE=1 cargo test -p pocketdesk-desktop the_real_grab_locks_and_frees -- --ignored` (without the variable it skips) and move the mouse during the five seconds: the pointer stays hidden and inside the window, the test reports relative moves above zero, and the pointer comes back free as the window closes. On Windows, and on the laptop under Hyprland (Wayland, locked in place) and under an X11 session if one is at hand (confined). (where: owner)

**236. The rest of the pages (Desktop UX 4)**

- On Windows at 100 % and 150 % scaling, and on the Linux laptop (Hyprland), with nothing paired: Sessions, Files, Messages, Notifications, Phone files, Clipboard, Activity, Diagnostics and Logs each show a card saying what will show there and how, light and dark, nothing clipped at the smallest window; each button on them (Pair a phone, Open Devices, Turn it on in Settings, Clipboard settings) opens what it says. (where: owner)

**237. The window's mic to the other computer (PC10)**

- On a test machine (never the owner's box), run `POCKETDESK_MIC_TEST_MACHINE=1 cargo test -p pocketdesk-desktop the_real_microphone_gives_frames -- --ignored` (without the variable it skips): it reports about 50 frames in a second. On Windows, on the Linux laptop (PipeWire 1.6: `pw-record --raw`), and once there with `pw-record` hidden from `PATH` so `parec` is used. With `POCKETDESK_MIC_TEST_DEVICE` set to part of a second microphone's name, that one opens. (where: owner)
- Windows viewing the laptop, Control on: Send my mic on; the bar says "Mic on"; on the laptop `pactl list short sources` lists `pocketdesk_mic` within a second, and `pw-record --target pocketdesk_mic` for 5 s gives a file of about 5 s with samples above silence while someone speaks at Windows (count them; never play it on the owner's box). (where: owner)
- Turn it off from the bar, turn View only on, close the window, and cut the link (stop Windows' server), each in turn with it on: `pocketdesk_mic` is gone from the laptop's `pactl list short sources` within about 2 s each time, and the bar no longer says "Mic on". (where: owner)
- The laptop's Devices page for Windows with "Use the phone as a microphone" off: Send my mic on at Windows makes no `pocketdesk_mic` there; on again there, from the next session, it does. Its hint on a computer's page names the screen window's Send my mic. (where: owner)
- The laptop viewing Windows (Windows with VB-CABLE installed, its speakers muted): Send my mic on at the laptop plays into "CABLE Input" on Windows, counted by a recorder on "CABLE Output" for 5 s (samples above silence, counts only). (where: owner)
- Windows (a test machine) with Privacy & security, Microphone, "Let desktop apps access your microphone" off: Send my mic says "Couldn't use this computer's microphone: Windows keeps the microphone from desktop apps (...)" and turns off. "Microphone for computer windows" set to a name no microphone has: it says no microphone is named like it. (where: owner)
- With Send my mic on, Alt+Tab to another window here, and minimise the window: the window's title (and its taskbar button) starts with "Mic on", the main window says "Mic on: this computer's microphone goes to …" on every page, and the laptop's `pocketdesk_mic` stays; the main window's Stop stops it. Minimised, turn Control off for Windows on the laptop: the mic stops within a second (the laptop's `pocketdesk_mic` goes) with no look at the window. The window dropping into Reconnecting turns it off, and it stays off as the picture comes back until turned on again. (where: owner)
- Task Manager (or `top` on the laptop): the desktop app's CPU with Send my mic on is a few percent at most, and the Stats' bitrate barely moves. On a slow internet road (a relay), typing while it sends: keys arrive as without it, and the laptop's mic lags well under a second. (where: owner)

**239. The range review after 0.8.2**

- Two computers paired both ways after this build (fresh pairing): the laptop's Devices page for Windows shows "Use the phone as a microphone" on. At Windows, Send my mic on with Control on: `pocketdesk_mic` appears on the laptop. Turn the window's View only on, then back off and Send my mic on again: no frames play on the laptop while view-only (counts of samples above silence only). With the switch off there, the bar says the laptop does not play this computer's microphone. (where: owner)

**240. Keys by position in a computer's screen window**

- An input method here (Windows' Japanese IME, or Linux fcitx): note that it does not compose in the window in either mode (the picture asks for no input method), and that with Keys by position on, its keys go by place with nothing typed twice. (where: owner)

**250. Approve with my phone: sudo, polkit and the lock screen**

- A real fingerprint on the owner's phone approves `sudo true` on the owner's desk, and the phone shows sudo's command; No, no answer and a phone out of reach each give the password prompt (at once with no phone linked, after the wait otherwise). (where: owner)
- polkit on the owner's desk: a `pkexec true` (or a GUI admin prompt) with the line in `/etc/pam.d/polkit-1` is approved on the phone. (where: owner)
- The lock screen on the owner's desk (hyprlock): with "The lock screen too" on and the line in `/etc/pam.d/hyprlock`, the phone unlocks it; with the switch off, only the password does. (where: owner)
- Pinning on a real phone with Key Attestation: `sudo pocketdesk-pam-ask --pin <user>` asks the phone, the fingerprint makes an attested key, the phone says it replaces the current key and shows a code, root asks for that code and pins only when it is typed right, reports the key attested (TEE or StrongBox), and the old key keeps approving until a request names the new one. (where: owner)
- "Fingerprint only, not the PIN" on the phone: the prompt offers no PIN, and a fingerprint enrolled afterwards makes the phone say its key no longer works until it is made and pinned again. (where: owner)
- `sudo` over ssh to the owner's desk with "Only from this desk" on asks for the password at once, and the phone is not asked. (where: owner)

**253. Fit the screen to this window, on a Linux computer**

- The desktop app on Windows with a window onto the laptop's real Hyprland session (the laptop paired from it, Input on): the bar's switch turns on, a new screen of the window's size appears in the laptop's `hyprctl monitors`, the picture follows a drag of the window's edge about half a second after it stops, and closing the window removes the screen (`hyprctl monitors` as before). (where: owner)
- The same on a laptop with a real second monitor, and with two windows fitted (two `PD-` screens) while one is resized: the other's place does not move onto the laptop's own monitors (Hyprland's `auto-right` is set again for the resized one). (where: owner)
- A host with "Phones may add a screen" off, or with Input off for the asking computer, says why on the window and shows the screen asked for. (where: owner)
- A program started on the fitted screen by its own keys (the window drives the empty desktop: a workspace of it, a launcher) works as on any screen. (where: owner)

**255. The phone's location for this computer's apps (GeoClue, Linux)**

- A real session on the owner's desk (GNOME or KDE): night light or the time zone panel takes the phone's city; the server's log and `events.jsonl` hold no coordinates (search them for the first digits of the phone's true latitude: none). (where: owner)

**257. A direct link on Windows: the Mobile Hotspot**

- Read-only, on a Windows computer (`cargo test -p pocketdesk-core --features local direct_link::tests::this_computer -- --ignored --nocapture`): "PLAN mode=SecondInterface" with an internet connection; with the network cable and Wi-Fi off, "NO PLAN: NoConnectionProfile" and the Pair page says Mobile Hotspot needs an internet connection; with the owner's own hotspot on, "NO PLAN: HotspotInUse". Print nothing but the plan or the reason. (where: owner)
- The Pair page's "Make a direct link" on a Windows computer with an internet connection and its own hotspot off: the status says `on` with an address in 192.168.137.0/24, Windows' Settings (Network & internet, Mobile hotspot) shows a name starting `pocketdesk-` and the hotspot on, and the Pair page shows the code. (where: owner)
- The owner's settings come back: note the hotspot's name, passphrase and band in Windows' Settings first (outside the repo, never pasted anywhere); after "Turn off" the hotspot is off and its name, passphrase and band are as they were; with the hotspot on first, a link is refused (HotspotInUse), and after turning it off in Settings, made, ended, and turned back on by hand, the settings are the owner's own. (where: owner)
- The OPPO scanning the Windows computer's code (a screenshot pushed by adb): Android asks once to join `pocketdesk-…`, the phone joins (WPA2: if Windows' hotspot is set to WPA3 only the phone cannot join, which the first run should report), pairs, and starts the session over it; `direct_link.json` says in use while the phone's session runs. (where: owner)
- The idle timer and the longest life on a real Windows link (a 1-minute idle setting is enough): with no phone it ends after the setting, the hotspot off and the owner's settings back; with a phone's session it stays on; and turning the hotspot off in Windows' Settings while a link is on is not noticed (a known gap: the page keeps showing the code until the idle time ends the link, and the owner's settings then go back). (where: owner)
- The band row: 5 GHz on a card that allows a hotspot on it makes a 5 GHz network; on one that does not, the link comes up on the card's own band and does not fail. A card that cannot host beside its own Wi-Fi connection (Windows drops the Wi-Fi uplink) is reported in the first run's words. (where: owner)
- A put-back that fails is retried: with a link on, unplug the internet and turn the link off (the owner's settings may not go back at once); the log says the hotspot was not fully put back, the record stays, and within a minute of the internet returning the settings are back and the record is gone. The hotspot does not come up with the link's name in the meantime. (where: owner)
- A hotspot still named `pocketdesk-…` (left by a killed server whose record was lost): "Make a direct link" says to give it a name of your own and starts nothing. (where: owner)
- The Pair page's card follows the server's plan, not the desktop's own look: with the internet unplugged the card says Mobile Hotspot needs a connection within about 30 s, and after plugging it in the button comes back within about 30 s; the desktop app itself never calls WinRT. (where: owner)

**263. Phone apps in windows of their own, several at once (Settings, Separate windows (Shizuku))**

- Touch, scroll, Back and typing in a window (a notes app, a browser with a made-up page): each reaches only that window's app, even with two windows open; Home and Recents are not offered; a letter outside the keyboard's map is skipped, not typed wrong. Watch only (the window's bar) stops input. (where: OPPO)

**265. Keys to taps: a phone game played from the keyboard and mouse (Settings, Keymaps for phone apps)**

- Off by default: with the setting off an app window's bar has no "Keys to taps" and typing, touch and scroll work as in section 263; the phone's own screen window says nothing about keys. Turn the setting on: each app window's bar shows "Keys to taps" and the warning that some online games flag injected input; the phone's own screen window says keys to taps works in an app window only. (where: OPPO)
- A tap: Edit keys, Tap, click a point on the picture, press a key, Done, turn the keys on: holding the key shows one touch at that point in "Pointer location" and the app reacts as to a finger; letting go lifts it. The mark was drawn over the picture while editing and not afterwards. (where: OPPO)
- Several fingers at once: hold W (stick), a tap key and the aim together: "Pointer location" shows three pointers at once, each moving on its own, and a drawing app or a map pinch (two taps either side of a point moving apart) responds as to a real multi-touch; releasing them in any order leaves nothing stuck. (where: OPPO)
- Nothing stuck: with a key, the stick and the aim held, switch to another window (Alt+Tab), turn "Keys to taps" off, open the editor, turn control off, close the window, kill the desktop app: each lifts every finger within a moment (no touch stays in "Pointer location"); the helper's display going with the window ends them anyway. (where: OPPO)
- The injection stays on the window: with keys held in a window, the phone's own screen shows no touch of them (its own touches in "Show taps" are none), on every Android at hand (14 first; 10 to 13 owed as for section 263); two windows with two different keymaps play only their own app. (where: OPPO)
- Held fingers survive nothing: hold a mapped key, then pull the desktop's network cable or kill the desktop app, stop the control stream (stop the relay), turn the phone's "Separate windows" off, stop Shizuku, and just stay still with a key held for longer than two seconds (it stays down, kept by the half-second keep-alive in "Pointer location"): after a dropped stream, the switch off or two seconds of silence the touch is gone; with a helper killed (`kill` its process) the next open window works without a phantom touch. (where: OPPO)
- The limits: a desktop test loop sending 1000 finger downs a second to one window lets about 100 through a second (the rest are not seen in "Pointer location"), finger lifts of fingers that were down always arrive, no finger stays down afterwards, and ordinary aiming at 144 Hz is not cut. (where: OPPO)
- The typing rule: with keys on, letters typed in the window go nowhere (not to the phone's text fields), Backspace and Enter and Escape (Back) still work when not mapped, and Ctrl+key shortcuts are not taken as keys. (where: OPPO)

**278. A borrowed window takes the size of its window (resize follows the viewer, window handoff)**

- The viewing window moved between a 100 % and a 150 % or 200 % scale monitor: the host's window takes the new physical size (not the point size) and the picture is sharp on both. (where: owner)

**273. The window's wait for the display (phase 1d of the window handoff: a setting to draw without vsync, and the window's stage trace)**

- Windows: the setting on tears at most, never freezes the window, and an animating page (a spinner) does not pin a core for long; off is as before. (where: owner)

**274. A window the compositor does not show no longer stops the app (the swap never waits on Wayland, and the windows are paced)**

- Windows: the swap still waits by default; with the Settings switch on, an animating page (a spinner) does not pin a core and the pictures show no worse than before (the pacing sleeps in ticks of the Windows timer, up to 15 ms, only for a window that asks for frames closer than half a refresh). (where: owner)

**280. A Windows computer lends one of its windows (phase 5 of the window handoff: Windows as the source)**

- The host's "Let my computers borrow my windows" turned off while a window is lent: the window is given back within about two seconds, the viewer's window ends, the list is refused. The viewer's computer going away (network cable out, app killed): the window is back within a few seconds to a minute (the sweep and the 60 s lease). The host's server stopped from the tray or by `pocketdesk stop` (the owner does this, never the build): every window is put back as it was. (where: owner)
- A server crash or kill while a window is lent (the owner does this): the window stays at the size the viewer last gave it and is not touched again at the next start (handles are not kept), and nothing is lent after the restart. (where: owner)

**281. A GNOME computer lends a window to look at (phase 6 of the window handoff: GNOME as the source)**

- GNOME's screen-sharing indicator shows in the top bar while a window is lent and goes when it ends; the host's own pointer and keyboard are not affected at any moment. (where: owner)
- A Shell extension, once one exists (not built), that answers the list with places, stacking and focus: whether input can then be confined as it is on Windows; the window guard's questions it would answer. (where: owner)
- KDE Plasma (not built): the measured facts to confirm on a real Plasma 6 desk: an ordinary program can load a KWin script and read the windows, stacking and active window; whether `zkde_screencast_unstable_v1` and `org_kde_kwin_fake_input` are offered to a program with no desktop-file entry. (where: owner)

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

**260. Phone folders kept on the computer, by themselves (Settings, Folders kept on my computer)**

- The schedule: the job is queued with its constraints (`adb shell dumpsys jobscheduler` lists the app's WorkManager job needing an unmetered network and charging; with "Every few hours" the charging need goes); on mobile data with Wi-Fi off, nothing runs; plugged in on Wi-Fi it runs within the period, and on ColorOS 14 the transfer service's notification shows during the run and goes when it ends (or the run waits for the app to be opened or the notification listener's next look, which the log says). (where: OPPO)

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

**262. A spare phone at home as the wake helper (Settings, Wake helper for this home)**

- Introduction: with the helper phone paired with the throwaway computer's own code and connected once while the computer is awake, the desktop's Settings, Wake shows "<phone> is this computer's wake helper" and the computer showed one notification "<phone> is now this computer's wake helper". A phone paired as a guest (a time-limited grant, or another owner's code) that sends the offer is not listed (the server's log says an offer from a phone that is not an owner phone was dropped). (where: second phone)
- Owner phones learn of it: another owner phone, connected once while the computer is awake, shows "Wake through my helper" in the computer's Wake settings with the helper's name (off by default); a guest phone shows "No wake helper yet". (where: second phone)
- The limit: two Wakes in a row work; a third within 30 seconds says "the helper was asked a moment ago: try again in half a minute", and one half a minute later works. (where: second phone)
- Revoking: remove an owner phone from the computer's Devices page: within a minute the helper's list drops it and that phone's Wake now fails ("the helper could not send it" or "could not be reached"); the packet capture shows nothing for it. (where: second phone)

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

**262. A spare phone at home as the wake helper (Settings, Wake helper for this home)**

- The wake: the computer asleep (suspend it by hand on the throwaway machine), the owner phone on mobile data with "Wake through my helper" on: Wake sends the request, the computer wakes, and the helper's capture shows the magic packet matched the computer's own MAC (matched or not only). With the same phone at home on the same Wi-Fi, Wake sends the packet from the phone itself, as before. (where: away)
- Another network: take the helper to a different Wi-Fi network (the computer elsewhere) and ask for a wake: the answer is that the helper could not send it, and the packet capture there shows nothing. A helper on a VPN-only network is told its duty is withdrawn: an owner phone revoked meanwhile can no longer ask once the helper hears it (within a minute). (where: away)
- A helper whose relay changed (move the helper to another Wi-Fi network, with the computer awake): it offers again and the owner phone's next connection gets the new relay; an owner phone that has not connected since asks the old one and is told it could not be reached. (where: away)

**263. Phone apps in windows of their own, several at once (Settings, Separate windows (Shizuku))**

- The display flags: the display is neither public nor a presentation, so check on each Android at hand that `am start --display <id>` from the helper still launches another app on it (Android 14 first; on 10 to 12, where it may refuse a private display, the page then opens the app on the phone's screen and the log says the launch failed), and that the helper's touch, scroll, Back, Enter and Delete reach that app and no key outside those three is ever pressed. (where: OPPO)

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
- The first link after pairing: Other computers says "linked" on both within about a minute of pairing, never "refused" for five minutes (a refusal in the first 2 minutes after pairing is retried on the backoff, link follow-ups 2026-10-30). (where: second computer)
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
- With it on, end the desktop app on Windows from Task Manager (a crash, not a close): the laptop's keyboard and touchpad come back within about 8 s (the relay waits 4 s for the window to come back first). (where: second computer)
- Both ways of the link: repeat the on and off with the laptop as the computer that keeps the link (its id sorts first) and with Windows keeping it, by pairing the other way round or a throwaway pair of ids. (where: second computer)
- Lock it when I close the window, on (bar or Settings), with the switch on there: closing the window locks the laptop (`loginctl lock-session` or hyprlock); with the switch off there it does not, and a notification here says "Couldn't lock" with that switch's name; with the option off it does not. Never the other way round: Windows here is the owner's box and is never the one locked. (where: second computer)
- With a phone holding privacy on the laptop first, Hide what I do there says another phone or computer holds privacy mode, and closing the window with the lock option still locks the laptop and leaves the phone's grab as it was. (where: second computer)
- Lock it when I close the window on, the switch on there: quit Pocketdesk here from the tray with the window open (showing the laptop): the laptop locks, the app is gone within about a second, and the laptop's session list no longer shows the view. With the window reconnecting as you quit, it does not lock. Signing out of Windows (or shutting it down) with the window open quits the app the same way and locks the laptop too. (where: second computer)
- With Hide what I do there on, stall the window's writes (on a test machine: slow the link with `clumsy` on Windows or `tc qdisc ... netem delay 2000ms` on the laptop while dragging the mouse fast, so the relay's input queue fills): the window's connection ends and it joins again within a second, the bar stays “hidden” and the laptop's keyboard and touchpad stay blocked throughout (no “Privacy ended: the window closed”). (where: second computer)
- Lock it when I close the window on, the pointer locked, the link slowed as above: close the window during a stall: the laptop locks (the lock goes before the pointer's free, and one that cannot be written goes with the stop), or a “Couldn't lock” notification here says why; never neither. (where: second computer)

**234. A faster picture: hardware decode, NV12 and HEVC (PC5)**

- In the real window on Windows viewing the Linux laptop: Stats say `d3d11va`, and the picture's colours match the laptop's screen (a red, a skin tone and a grey side by side; NV12 by the shader against the window's own Screenshot, which converts on the processor); resize, full screen, the dimmed picture while it reconnects and another screen all draw. (where: second computer)
- HEVC both ways: with `allow_hevc` on on the other computer, Stats say `HEVC` and the picture is right; another screen, or a resolution change there, restarts it at the new size; with "Use HEVC when both can" off here (next view) it is H.264 again. Note the bitrate Stats say for each codec on the same screen. (where: second computer)
- Record a view that came as HEVC (Stats say `HEVC`) for ten seconds: the MKV plays in a player and is ten seconds long (`ffprobe` says `codec_name=hevc`). (where: second computer)
- "Decode a computer's screen on the graphics card" off: the next view's Stats say `software`; on again, the card. Compare the ffmpeg sidecar's CPU in Task Manager (or `top` on the laptop) for the card and software at the same screen, 1440p and full motion. (where: second computer)

**235. Pointer lock and relative mouse (PC7)**

- The laptop viewing Windows, a 3D or mouse-look program in front on Windows (Blender's viewport fly mode, or a game in a window): Lock the pointer turns it as the mouse moves, with no edge stop; "Ctrl+Alt+Shift+M frees the pointer" shows for about 3 s; the chord frees it. Never the other way round with the owner's box grabbed. (where: second computer)
- Windows (a test machine) viewing the laptop's headless sway stand-in, locked: `wev` on the stand-in counts relative motion events while the mouse moves (counts only), and none once freed (the moves go absolute again). (where: second computer)
- Locked, each of these frees it at once and the pointer shows again: Ctrl+Alt+Shift pressed and let go alone (in full screen with “Send Win and Alt+Tab there” on, note that it also gives Win and Alt+Tab back to this computer until the window leaves full screen; Ctrl+Alt+Shift+M frees only the pointer); Alt+Tab or a click elsewhere (focus loss); View only; closing the window; the view dropping into Reconnecting. Coming back to the window does not lock again by itself. (where: second computer)
- With the shared keyboard and mouse on and the laptop placed beside the viewing computer: locked, pushing against that edge (and pressing the switch key) never crosses, the badge says a screen window holds the pointer; freed, the edge crosses again. With a game in front holding the edges already, locking and freeing says nothing more. (where: second computer)
- A long, slow drag of the mouse while locked (a few pixels a second) moves the far pointer the same distance as a quick one, with no drift lost to rounding; and a fast circle on a slow internet road comes back about where it started (a relative move the relay's full queue drops is owed and goes with the next, up to 256 counts each way; past that, after a long stall, the rest is dropped so the view never spins, PR #563 review M5 and re-review N4). (where: second computer)
- Locked from the bar (on Wayland, where the hidden pointer stays on the bar's switch), click in the picture several times: each click reaches the far computer and the lock stays on; the bar looks greyed out until it is freed. (where: second computer)
- Locked, stop the window answering (suspend the desktop app with Resource Monitor's Suspend Process on Windows or `kill -STOP` on Linux, on a test machine): within about 3 s the shared keyboard and mouse's edges lead on again, and the badge says so. (where: second computer)
- Locked, on a slow far link (`clumsy` or `netem delay 2000ms` as above), move the mouse hard for 30 s: the window drops and joins again several times (the picture stays), the moves resume after each, and the view does not end with “kept dropping this window”; only a link that stays stalled past about 20 rejoins in a minute ends it, saying the link stayed too slow. (where: second computer)

**236. The rest of the pages (Desktop UX 4)**

- Keyboard and mouse with the Linux laptop paired: Your computers shows it as a card with its dot (green linked directly); with the server stopped its "may drive" box is greyed with the line and Start the server, which starts it. (where: second computer)

**238. Every screen of the other computer (PC11)**

- The Linux laptop with two screens (or Windows with two) shown from the other computer: the bar's buttons sit as its screens do (one above the other or side by side as in its display settings), the one shown highlighted; with a computer on 0.8.2 or older they are a row. (where: second computer)
- Switch with a button, then with Ctrl+Alt+Shift+2 and +1 (the far computer gets no keys from them: `wev` on a Linux far side counts none): the last picture dims with "Switching to …" and the new one comes; note the Stats' "Last switch" over five switches each way, home network and internet path (before PC11 the window's own part alone was about 0.3 s more). (where: second computer)
- A switch while locked (Lock the pointer), hiding (Hide what I do there, the far side's Power switch on) and recording: the pointer is free and shows after it; privacy stays on there throughout (the far computer's keyboard stays blocked; its ring stays); the recording's first file ends and a second starts for the new screen, both play. (where: second computer)
- Right-click screen 2's button, Open in a new window: a second window shows screen 2 beside the first's screen 1, both moving (a video playing on each); the far computer's session list shows two sessions; the Stats in each say "Views on this server: 2 of 4". With control: on a Windows far side, and on a Linux far side on a wlroots compositor or Hyprland from this build on (checked in Sway and Hyprland only; not verified elsewhere), a click in each window lands on its own screen there (each window's corners on its screen's corners; note the far side's scales and where its screens sit), and still does after moving or rescaling a screen there mid-session; on a Linux far side on GNOME, KDE or X, or on a build before it, the second window opens View only and its bar says that computer takes clicks over its first screen (the first window says so too when switched to screen 2) (turn View only off there and note where a click lands). (where: second computer) *2026-10-31, the Linux per-screen remap (no real second monitor): two windows of a throwaway pair on the laptop's loopback, into a headless Sway and a nested Hyprland with two outputs each (scale 1 side by side; 1.5 beside 1.25 with the second at -2048,-300; scale 2 above the first with Xwayland off; Hyprland 1 beside 1.33; Hyprland 1.5 beside 1.5 at -1280,-300 with Xwayland off), each run moving and rescaling the second output mid-session and clicking again: `wev` on each output saw 100 of 100 clicks (corners and middle of each window, both before and after the change) on that window's own output, the corners on its corner pixels; the live test asserts it. Owed: the same with real monitors, and the pen (a phone's stylus on one screen of such a computer goes through the same remap onto uinput's tablet, not checked).*
- Close the second window: the first goes on untouched, the far computer lists one session; open it again, then stop the first window: the second goes on. Quit Pocketdesk with both open and "Lock it when I close the window" on: the far computer locks once, after both stopped. (where: second computer)
- Open four windows, then a fifth from any bar: "At most 4 screen windows at once" shows and nothing else opens; the server's memory and the far computer's CPU with four views (counts only). (where: second computer)
- With "Hide what I do there" on in one window, ask it in the second window of the same computer: it says another window already hides it, and nothing changes there. (where: second computer)

**239. The range review after 0.8.2**

- Windows and the Linux laptop sharing the keyboard and mouse (Power on for the laptop on Windows' Devices page): at the laptop, open Windows' screen window and turn on "Hide what I do there", once with Windows' keyboard on the laptop and once with it home: both times the bar says it was refused (it shares its keyboard and mouse), and Windows' keys and clicks still work. With shared input off on Windows, the same ask is taken (keyboard blocked, ring shown); then Ctrl+Alt+Del, Lock, and unlock Windows (and again with Windows+L): within about a second of the unlock the grab ends and Windows' keyboard and mouse work. On the laptop as the far side (a grab from Windows' window, shared input off there): `loginctl lock-session`, then `loginctl unlock-session` over ssh, ends the grab only with a locker that sets the locked hint (note which: hyprlock, swaylock, GNOME); and unplugging and plugging the laptop's USB keyboard back in gives a keyboard that types while the grab holds. (where: second computer)
- Two windows on the laptop's two screens: copy a line in window 1, click window 2 and paste: the line pastes. With a window on the laptop and one on a third computer: copy in the laptop's window, click the other window and paste: the copy from the laptop arrives. (where: second computer)

**240. Keys by position in a computer's screen window**

- The Linux laptop viewing Windows, Keys by position on, Windows' layout US: in Notepad, the laptop's keys type as on Windows' own keyboard; switch Windows to French (AZERTY): the key labelled Q on the laptop types "a" there. Off again: typing gives what the laptop's keys say, whatever Windows' layout. (where: second computer)
- Windows viewing the laptop (Hyprland, uinput), Keys by position on: `wev` on the laptop shows each key's code as pressed at Windows (W is 17, the ISO key beside Shift 86 on a 105-key keyboard), Shift going down as pressed; holding W for a second shows one press and repeats, and one release. A game or `wev` sees WASD and Shift held together. (where: second computer)
- With W held, switch Keys by position off, Alt+Tab away, turn View only on, and close the window, each in turn: `wev` shows W let go each time, nothing stuck. The view reconnecting (stop the laptop's server for a moment) leaves nothing held either. (where: second computer)
- Lock the pointer with Keys by position off: the bar says "Games usually want Keys by position" with its buttons; free the pointer and click Turn on: it is on, and closing and opening the window for the same computer starts it on (another computer starts as Settings say). "Not now" in a fresh window: locking again does not offer it. (where: second computer)
- A Linux far side on X11 (enigo, no uinput) with a German layout: Keys by position on, the laptop's Y key types "z" there (by its place, as on uinput and Windows hosts). The phone's own keys on that desktop now go by place too: note what the phone's shortcut sheet's Ctrl+Z does there with a German or AZERTY layout (before this, enigo looked the letter up in the layout). (where: second computer)
- Keys by position on, Windows far side with two layouts (Alt+Shift switches them): press Ctrl+Alt+Shift+M and Ctrl+Alt+Shift+S at the laptop: a key logger there sees only the first two modifiers pressed and let go, never the third (Alt+Shift alone does switch the layout there, as it should). In a game there: W held, then Ctrl (crouch), then Shift (run): all three are held at once. Ctrl held then C held: C is held as long as it is, not tapped at its release. On a Dvorak layout here, Ctrl and the key printed C copies there and nothing keeps repeating after it is let go. (where: second computer)
- A Japanese (JIS) or Brazilian (ABNT) keyboard at the laptop, Keys by position on, a Windows far side with that layout: the Ro, Yen, Henkan, Muhenkan and Katakana/Hiragana keys and the keypad's Enter and slash do there what they do on its own keyboard; with a US layout there, note what each does (the scancodes are written from Microsoft's table, not seen on a device). (where: second computer)

**241. Remove a lost device from another of yours (remote revoke)**

- Two computers paired both ways (Windows and the Linux laptop): at the laptop, Settings → Other computers → Windows → Devices there… lists Windows' devices, the laptop marked "this computer"; Remove… a throwaway device there: it goes from Windows' Devices page, and Windows notifies. Do it once with the laptop keeping the link and once with Windows keeping it (pair again from the other side). (where: second computer) Partly checked 2026-10-31 through B's `computers_out/` asks (what the desktop page sends), with B keeping the link: `devices_there` listed A's devices with B marked as the asker, `remove_there` took a seeded device off A (`removed: <name>`), and A logged "Dchk B (computer <id>) removed …". Not run: the desktop page itself, the Windows toast, and the direction with Windows keeping the link. 2026-10-31 second batch: the other direction too, A keeping the link (A pasted B's link): B's `devices_there` listed A's devices (B `you`, the phone yours, C a computer not yours), `remove_there` took C off A (`removed: Dchk C`) and A notified "Dchk B (computer <id>) removed Dchk C from this computer". Still not run: the desktop page itself (the headless app's Settings opened, but the headless compositor took no clicks to open Other computers).
- A computer paired one way (no "Mine" tick): its Other computers card has no Devices there…; a scripted `remote_devices` ask from it on its link is answered `refused: not_owner`. (where: second computer) Half checked 2026-10-31: C paired one way with A, a scripted `devices_there` ask from C answered `refused: not_owner` and A logged the refusal. Not seen: C's desktop card.

**242. One version across my devices**

- Windows and the Linux laptop paired both ways, one on this build and one a release behind: each one's Settings → Other computers shows the other's release, with the advice on the newer one's row only pointing at the older; on the computer that dials the link it comes from `computers.json`, on the other from the trust store. (where: second computer) Data checked 2026-10-31 (A 0.8.2, B 0.9.0, B dials): B's `computers.json` `versions` has A at 0.8.2, A's trust store has B at 0.9.0. The desktop Settings rows were not opened.

**247. Shared input A: the receiving end, measured end to end**

- Windows driving the Linux laptop for real (the owner's mouse and keyboard, not the script): `share-test --keys 500 --moves 2000` from Windows to the laptop, before and after this build, and a feel check that the pointer keeps up and clicks land where it is. (where: second computer)

**249. Fine scrolling end to end**

- Windows host: a slow two-finger scroll from the phone or another computer's window scrolls a browser page a little at a time (`SendInput` wheel data below 120). (where: second computer)
- Shared input between two computers on this build: a touchpad's slow scroll on the driving one scrolls finely on the other; with one on an older build, by notches. (where: second computer)

**251. Shared input B: the driver loop, batched and kept light**

- Windows driving the Linux laptop for real with this build (its hook loop has the same changes and builds on CI only): a 1 kHz mouse keeps up and feels smooth, clicks and drags land where the pointer is, keys typed during a fast move arrive in order, and the gaming lock still takes the hooks out within about half a second of a game coming to the front. (where: second computer)
- The Linux portal driver for real (the owner's Hyprland driving another computer): the same feel check, and the edge still crosses after a screen is plugged in or rescaled. (where: second computer)

**253. Fit the screen to this window, on a Linux computer**

- A Windows computer as the other end: the window's switch is greyed with the reason once it says it runs Windows, and the switch turned on before that (Settings) is answered "only Sway and Hyprland" and shows the screen asked for. (where: second computer)

**254. Unattended on GNOME (mutter's own D-Bus, no dialog)**

- The "Allow Remote Interaction" off case through the portal on a real GNOME: the server logs "Portal input not taken" once, and another computer's window onto this one (Settings → Other computers → Show its screen) goes view-only without a reconnect; the phone is no longer offered the pen. (where: second computer)

**255. The phone's location for this computer's apps (GeoClue, Linux)**

- Two computers of the owner's, both with the setting on: each asks only its own phone, and a computer the phone is not paired to gets no answer. (where: second computer)

**258. Open any phone app from the computer (Phone apps page, Apps part 1)**

- "Let the computer open my apps" off on the phone: the page says the phone keeps its apps to itself and names the setting; turned on, "Ask again" lists them. A computer that is not the phone's own gets no word from it at all. (where: OPPO) Partly checked 2026-11-01 (the OPPO): with the switch off an ask for the list and an open were each answered `off`; turned on, the next ask for the list gave the 77 apps again. "A computer that is not the phone's own gets no word" needs a second computer and was not tried; the page's words are desktop UI.

**261. Phone apps on the computer's Start menu, from notifications, and dropped files shared into the app in front**

- The guards: "Let the computer open my apps" off, then the control service off, then a secure lock, then the screen shown to another computer only: each drop leaves the file in Downloads and the line names the reason; the foreground package is read once per drop (the phone's log names no package of any other window), and Pocketdesk's own screen in front is refused. Whether a share started from the service with another app in front appears on ColorOS 14 and Android 15 or 16, and whether Android's chooser fallback shows when the direct share is refused. (where: OPPO)

**263. Phone apps in windows of their own, several at once (Settings, Separate windows (Shizuku))**

- A screen cast running at the same time (the phone's screen shown to the same computer): a window opens beside it and each has its own window here; stopping the screen cast keeps no window open unless the session is still up. With the screen shown to another computer, a window ask is answered `windows_unavailable`. (where: OPPO)

**264. Wear OS tile, complication and list (Settings, Watch controls; `android/wear`)**

- Wake: with the laptop asleep by hand (never by Pocketdesk's tests), the tile's Wake wakes it as the phone's Wake tile does, and says "Sent"; with wake off for that computer it says why. (where: second computer)
- Music: play/pause and next on the tile change the computer's player as the phone's music tile does; with Audio off for the phone on the computer the tile says the computer refused. (where: second computer)
- A question: with a `pocketdesk ask` waiting, the tile shows "No: ..." and the list shows its words; No on the watch answers No on the computer and takes the phone's notification away; with the watch's lock set it works unlocked and says "Unlock the watch first" when locked; with no watch lock it says "Set a watch lock to use this". (where: second computer)
- Deck: with Deck switched on the list shows the computer's buttons (at most eight) and a press runs it (a button the computer's own rules refuse says so); with it off the list shows none. Two presses within 1.5 s: the second says "Too soon". (where: second computer)

**266. The computer turns on the phone's hotspot (Settings, Let my computers turn on my hotspot; Overview, Use my phone's hotspot)**

- Only its own computer: another paired computer's `hotspot_on` (sent by hand) is logged "NOT_OWN" and does nothing, and a paired computer with "Screenshot, lock and open its apps" turned off in Devices on the computer cannot ask at all. (where: second computer)
- The asking computer gone: with the hotspot on, turn off the computer's Wi-Fi or stop its server for over 2 minutes: the phone turns the hotspot off (and not before 2 minutes); unpair the phone on the computer, or remove the computer on the phone: it goes off at once. (where: second computer)

**267. Input to picture on a real link (the latency probe, phase 0 of the window handoff)**

- Across 5 GHz Wi-Fi: a throwaway server on one computer with `POCKETDESK_TEST_PATTERN=probe` (its trust entry for the viewing computer allows Input; a throwaway state folder), the other computer a viewer paired with it, then `PD_LIVE_A_STATE=<the viewer's state folder> PD_LIVE_B_ID=<the shown computer's id> cargo test -p pocketdesk-desktop input_to_picture_on_a_running_pair -- --ignored --nocapture` on the viewer: p50, p95 and max for H.264 and, with `PD_LIVE_HEVC=1`, HEVC, with no more than 2% misses; the numbers go in `docs/status-history.md` beside the loopback ones. Never against a server that shows a real screen. (where: second computer)
- Probe mode injects nothing: while that run goes, the pointer, windows and typing of the computer running the probe-mode server are not touched at all (watch it for the minute), and its log says "Latency probe: no input is injected". (where: second computer)

**268. A still screen sends nothing to another computer's window (the window's stats, a still desktop)**

- Before and after, numbers only: with a Windows computer as the host (a mostly still screen: nothing moving, the cursor left alone) and a Linux computer showing it in a window, once with a build from before this change on the host and once with one from after, read the window's stats (Ctrl+Alt+Shift+S) or the view relay's status for a minute of stillness each way: frames a second in and the bitrate. Record both pairs in the status history. After: within about 4 seconds of stillness the line reads "Frames: none (a still screen sends nothing)" with "Decode: idle" and "Pipe: idle", the bitrate near 0, and "Lost 0 \· late 0 \· dropped 0" stays 0; before: about 1 frame a second from the repeats (more only where something on the screen really moves). (where: second computer)
- A change shows at once: from the still state, move the mouse on the host or type into a text editor there; the window shows it with no more delay than before (compare against the old build's feel and the stats' delay), and a second later the picture is as sharp as the old build's (the two settling repeats let the encoder refine it); no blur stays on text typed and left alone. (where: second computer)
- A keyframe on a still screen: leave the host still for a minute, close the window and open it again (a rejoin asks for a keyframe), and press "Fit" or switch screens: the picture is back within about 2 seconds each time and never stays grey or empty. (where: second computer)
- The link is not taken for dead: leave a window on a still screen for 10 minutes: it does not say "Reconnecting", the host's log (`pocketdesk logs`) shows the session unbroken, and "Encoding task complete" at the end names `repeats_skipped` above 0. (where: second computer)
- The stats overlay on a still screen says the screen is at rest, not that frames are missing, and returns to the numbers ("Frames: 24 a second in, 24 shown") as soon as the host moves. (where: second computer)
- Both directions of the mix: a window from after this change on a host from before it still gets its repeats (about 1 frame a second on a still screen; nothing breaks), and a window from before on a host from after still works (it sends no bit, so repeats go on). (where: second computer)
- The switch: Settings → Other computers → "Send nothing while the other screen is still" (`computer_view_no_idle_repeats`, on) turned off, then a new window on the same still host: from the next view the stats read about 1 frame a second again ("Frames: 1 a second in", no "the screen is still") and `repeats_skipped` stays 0 in the host's "Encoding task complete"; turned on again, the next new window is quiet as above. The switch is read as each view dials, so an open window keeps what it started with. (where: second computer)
- A keyframe on a still screen, with the FFmpeg sidecar encoder (a Windows host on its default NVENC, AMF, QSV or Media Foundation rung, not the opt-in `inproc-windows`): the sidecar cannot be told to emit a keyframe, so after one is asked for (the window's gap recovery, or a crop moved at the same size) the host keeps sending the capture's once-a-second repeats until its next scheduled keyframe goes out (the log's "Keyframe encoded" line), then they stop again ("Encoding task complete" names `repeats_skipped` above 0 for the whole run); the window never stays grey. (where: second computer)
- An encoder that holds frames, on a still screen: a Windows host on an AMD card (AMF holds one frame) and one on Media Foundation (holds several; the debug log's "holds N frame(s) until the next input" names the lag). At session start on a still desktop the first picture comes out (never a blank window), and after a change the last picture is whole and sharp, not the one before it: the repeats go on for about N + 2 more after a change, then stop. (where: second computer)

**269. VA-API encodes each frame as it comes (phase 1b of the window handoff: `async_depth` 1, a yield after each packet send)**

- The other hardware rungs for a held frame, with the stage probe or a stopwatch on the host's side: NVENC in-process and sidecar on a Windows host (`delay 0`), AMF and QSV (`-async_depth 1`) and Media Foundation: the frame's packet comes out at its own frame, not the next (the debug log's "holds N frame(s) until the next input" absent). None of these can run on the laptop. (where: second computer)

**272. Borrowing a window of another computer (the viewing side, phase 3b of the window handoff)**

- The Windows app against the Arch laptop's real session (Hyprland or Sway), a computer paired both ways with Input allowed and "Let my computers borrow my windows" on there: Borrow a window... lists the laptop's windows with their titles, Borrow on a terminal opens a window here titled with the terminal's title, its picture is the window's size, typing and clicking there reach the terminal, and closing the window puts the terminal back on its old workspace. (where: second computer)
- The same from a Linux desktop app (the laptop as viewer, its Sway as host in a second session or a second computer): a borrowed window, then a second one in a window of its own, then a fifth refused with "At most 4 screen windows at once". (where: second computer)
- The host's switch turned off while a window is borrowed: it goes back within a couple of seconds, and the viewing window says the other computer closed that window or stopped lending. (where: second computer)
- The borrowing computer's link dropped (its network pulled, or its server stopped) with a window borrowed: the window is back on the host within a couple of seconds, not after a minute. (where: second computer)
- A borrowed window closed on the host (its program quit): its empty screen is removed within a couple of seconds and the viewing window ends with a note, never a frozen picture. (where: second computer)
- "Lock it when I close the window" on, a borrowed window closed: the other computer is not locked (only a screen window locks it). (where: second computer)
- A short network gap (the viewing computer's Wi-Fi off for about 10 s, then on) with a window borrowed: the view dims and reconnects and the window is still lent, not ended; the window is back after 60 s if the viewer stays gone, and the host lends it again if the viewer comes back after that. (where: second computer)
- A borrowed window's `return` lost (the link up but the packet not delivered, for instance the viewing server killed at the moment the window closes): within about a minute the host takes the window back without the viewer. (where: second computer)
- Two windows cannot borrow the same window: its Borrow is greyed in the picker once lent, and a second view asked for by the local API says it is already shown. (where: second computer)
- The app quit while the picker is open (or killed): the windows' titles are gone from the state folder's `computers.json` within a minute (and at the server's next start). (where: second computer)
- A borrowed window resized on the host (dragged, or its program resizing itself) keeps showing its first size: following the host's own resize is still later (its screen stays the size lent); following the viewer's window is section 278. (where: second computer)
- A window with a very long or odd title (emoji, right-to-left text) in the picker and in the viewing window's title bar: shown cut and without control characters, never garbled past the picker. (where: second computer)

**278. A borrowed window takes the size of its window (resize follows the viewer, window handoff)**

- A real second computer (the Windows app or the laptop's Linux app as the viewer, a Sway or Hyprland host): borrow a window, drag the viewing window's edge, maximise it, restore it, and the real window on the host follows each time with the text sharp (look at small text 1:1), the old picture scaled and undimmed meanwhile with no black frame, and the new picture in a moment (say how long it took on the home network). (where: second computer)
- A real program with a minimum size (a browser, a terminal with its own steps of a character cell): asked for a size smaller than it takes, the host's window ends larger than the screen or at its own steps, the viewing picture shows what is there (scaled, letterboxed) and nothing loops asking again. (where: second computer)
- The host on an older Pocketdesk (it ignores `width` and `height`): the borrowed window keeps its size, the viewing picture is scaled, and after two asks nothing is asked again for that view (a reconnect carries no size). (where: second computer)
- A short network gap or a drop while a resize is in flight: the view dials again, the host's screen keeps the last size asked, and the window there is not left at a size that does not match its screen. (where: second computer)
- The viewer's server cannot reach the host just as a resize is asked (the link dropped for a moment): the view does not end; it tries again on the reconnect backoff and the window comes back at the new size. (where: second computer)

**275. The other computer's pointer drawn here (local cursor, phase 2 of the window handoff)**

- A real Hyprland host (the owner's laptop) shown in a window on another computer: the picture has no pointer in it (move the pointer over a plain wallpaper: nothing is drawn in the picture while the window's own pointer follows the mouse at once), the shape follows the host's pointer (an arrow, a text cursor over a text field, a hand over a link, a resize arrow on a window edge), and it hides when the host hides its own (a full-screen video, typing). If the cursor session never answers on Hyprland 0.56, the log says "The pointer stays in the picture" and nothing is lost. (where: second computer)
- A real Sway host on a real output (a cursor plane): the same; and a Sway session that draws its pointer in software (a GPU or driver with hardware cursors off) shows both pointers, the window's own and the picture's. (where: second computer)
- A GNOME host (the portal, with Allow Remote Interaction, and Unattended on GNOME's mutter path) and a KDE host: the portal offers cursor metadata, the picture has no pointer in it and the shape arrives; with a portal that offers none, the log says so and the picture is as it was. (where: second computer)
- A Windows host shown on Linux, and a Linux X11 host: the pointer stays in the picture as before, the window's own pointer is the system arrow, and nothing is hidden or doubled. (where: second computer)
- An older computer (before this build) as the host, and an older one as the viewer: neither hides or draws anything extra; the view works as before. (where: second computer)
- "Lock the pointer" on the window (Ctrl+Alt+Shift+M) over a host that sends its pointer apart: the other computer's pointer is painted over the picture where that computer says it is and follows the relative moves; freed, the window's own pointer takes the shape again. (where: second computer)
- Over the internet and over Wi-Fi: a move of the free pointer is as quick as the mouse (it waits for nothing), and a locked pointer's painted copy follows the host's within about the link's round trip. (where: second computer)
- A lossy link (Wi-Fi with loss, or `tc netem loss 10%` on the host): a pointer message that is lost does not leave the window with a wrong or no pointer for more than about two seconds (the host says the whole pointer again every 1.5 s), and a shape that changed during the loss shows after the next refresh. (where: second computer)
- A Hyprland host shown in a window on another computer, a borrowed window of that host (its picture comes from the compositor's toplevel export): the window's picture shows the host's pointer (drawn into it) and the window draws none of its own, so there are never two or none, and showing the whole screen again leaves it out of the picture and draws it in the window. (where: second computer)
- The host's pointer capture ending mid-view (kill the host's compositor-side client or restart the compositor's capture): the window goes back to its own pointer within a moment and the picture shows the host's pointer again. (where: second computer)
- A GNOME or KDE host (portal or mutter) with a window of that host cropped in software, with this window's pointer locked: the painted pointer is placed over the whole screen's coordinates, not the crop's, so it is off by the crop's offset (a known gap; the free pointer is not affected). (where: second computer)

**276. Smooth keyframes (low-latency window handoff, phase 1: no whole keyframe on a two second beat)**

- The OPPO over Wi-Fi with some loss (`tc netem loss 5%` on the host's interface), the setting on, the software encoder: the picture heals within about two seconds of a loss on its own (the refresh wave), with no freeze, and no longer than with the setting off; with VA-API, a loss is healed by the keyframe the phone asks for. (where: second computer)
- A slow link, the setting on, the software encoder: the queue that waits for a link trims to the newest whole keyframe as before, and with the keyframes far apart the wait is the queue's depth (8 frames), not the keyframe's distance. (where: second computer)

**279. Dragging a window past the edge (phase 4 of the window handoff)**

- Hyprland 0.56 with a real second computer (Linux, the app running): hold Super, drag a floating window with the left button to the edge and over it, let go on the other computer: the window leaves this screen within about a second and opens as a borrowed window there, its picture sharp, and the pointer is where it was. Say how long from letting go to the first picture. (where: second computer)
- A Windows computer as the other side: the pointer crosses as before, no window moves, nothing is said. Windows source is phase 5. (where: second computer)
- The other computer's "Let my computers borrow my windows" off, or the computer paired one way, or no link: the pointer crosses, the window stays on its screen, and the borrowed window never opens. (where: second computer)
- The other computer's app not running when the window is dropped: no window opens after ten seconds, the window is not lent, nothing is stuck. (where: second computer)
- The dropped window's place on a Linux desktop that honours it (X11, or a Wayland compositor that lets a program place its window) and on one that does not (Sway, Hyprland: the compositor floats it where it likes; say where); its size is the window's own, then follows the viewing window as in section 278. (where: second computer)
- Drag the borrowed window by its bar back over the edge with Super and the left button held: the window comes home within about a second, put where the pointer comes in with the pointer where it held it, and the viewing window says "The window went back to ..." (it is not closed by itself: say if that is a nuisance). Without the key held, the pointer comes home and nothing returns. (where: second computer)
- The window dragged out and back twice in a row, and out to one computer and back from it with a second computer in the row. (where: second computer)
- Review fixes (PR #641): with a window handed over and its view open on the other computer, drag a text selection or a file, then another floating window with Super and the left button, back over the edge: nothing comes home, the view stays. Then drag the view itself back: it comes home. (where: second computer)
- The window dragged back almost at once after it was let go, before its view opened (within a second): the offer is withdrawn, the window stays on this screen and the other computer's view says "That window was dragged back before it was lent" (or never opens). (where: second computer)
- "Drag windows to my other computers" off on the other computer, on here: the window stays, nothing opens there. A window narrower or shorter than 64 px at the edge is not offered. (where: second computer)
- Two windows dropped on the other computer in the same second (from two computers, or two drags): both open. (where: second computer)
- A view that is view-only is not dragged back by its window (it reports no focus): say if that is a nuisance. (where: second computer)

**280. A Windows computer lends one of its windows (phase 5 of the window handoff: Windows as the source)**

- The Windows host with "Let my computers borrow my windows" on, the viewer's Settings, Other computers, "Borrow a window…": the list shows the host's windows by title and program (Notepad, a browser, Explorer), none of Pocketdesk's own (the app, the server), none minimised, none on another virtual desktop (cloaked), none that are tool windows or dialogs owned by another window. Say what a UWP app (Calculator, Settings) and a window of the Microsoft Store do. (where: second computer)
- Borrow Notepad: a window opens on the viewer within about two seconds and shows that one window, title bar and borders included, not the shadow around it, and not the host's monitor at any moment. Say how long from Borrow to the first picture, and whether Windows draws its yellow capture border around the window (it should not). (where: second computer)
- The picture's size: the viewer's window says the size the host answered (compare it with the window's size on the host with a ruler or a screenshot); on a 150 % and a 200 % monitor, and for a window on the second monitor and one on a monitor left of the primary (negative coordinates). (where: second computer)
- The pointer: moving over the viewer's window moves the host's real pointer to the same place in the window (check the four corners and the middle, a menu, the title bar's buttons: do not click Close); on the 150 % monitor and on the monitor left of the primary; a move never lands outside the window's frame. (where: second computer)
- Click and type: a click in the viewer's window places the caret in the host's Notepad and typing arrives there; the mouse wheel scrolls it. Say whether the window is on top of every other window after the first click. (where: second computer)
- The window covered by another (maximise a browser over it, then click in the viewer's window): the lent window comes to the front and the click lands in it, not in what covered it; say whether Windows refused the focus (the title bar not going active, keys going to the browser) and how often. A move alone must not raise it. (where: second computer)
- The host's owner moving the real mouse or typing while the viewer drives: the two compete (said in the guide); nothing hangs, and when the owner stops the viewer carries on. (where: second computer)
- A window minimised on the host before it is lent cannot be picked (the list leaves it out, and a lend of it by id is refused as not there); one minimised after the lend: the capture brings it back out of the taskbar within about a second (and on the next ask), so the viewer's picture does not stay frozen. Say whether the restored window takes the focus, and whether it is a nuisance that a window the owner minimised on purpose comes back (the owner returns the window to keep it minimised). (where: second computer)
- Resize follows the viewer: drag the viewer's window larger and smaller, maximise it, move it to a screen of another scale (the existing "Borrowed windows take the size of their window here" setting on); the host's Notepad takes the size within about a second of the picture settling and the picture is sharp, one pixel to a pixel. Say what a window with a smallest size does (Calculator, a browser) and whether the viewer's picture then scales rather than breaks. (where: second computer)
- A maximised window on the host is lent as it is and is never sized or moved (the picture is scaled in the viewer, the answer says the size it has, no error); on return it is still maximised. A normal window near the right or bottom edge of its screen that the viewer's window makes larger: it grows and is moved back so that all of it is on the screen (not under the taskbar); on return it is back at its first place and size. (where: second computer)
- Return, by closing the viewer's window: the host's window is as it was lent: a minimised one (restored during the lend) minimised again, a maximised one maximised, a normal one at its old place and size even after the viewer resized it and after the host's owner moved it. Say whether it takes the focus on the way back. (where: second computer)
- The host window closed while lent: the viewer's window says it closed within about two seconds and ends; the host shows no leftover; borrowing another window works. A viewer that redials just after the return does not show the host's monitor, whatever happens (the index of a window no longer lent captures nothing). (where: second computer)
- Two windows lent at once (Notepad and a browser) to one viewer: each its own window, no mix-up of pictures or clicks; a ninth lend at once is refused with "Too many windows are lent at once" and the first eight carry on. (where: second computer)
- An elevated window (a program run as administrator): the picture shows but the input does not reach it (Windows blocks input into a higher integrity window); say what the viewer sees and whether anything is sent to another window by mistake. A window showing protected or hardware-overlay content (a video player, a game): say whether the picture is black. (where: second computer)
- The Windows desktop app as the viewer of a Windows host's window (two Windows computers), and a Linux viewer: both behave the same; frame rate and latency of a window capture against the same window's monitor capture (say the numbers). (where: second computer)
- The host's whole flow with the viewer's view-only switch on: the picture shows, no input moves the host's pointer and the window is not raised. (where: second computer)
- The window dragged across the edge (phase 4) with a Windows computer as the other side still does nothing there (a plain crossing); Windows as the offering side of a drag is not built. (where: second computer)
- **Confinement, a third computer:** a second computer of the owner's, paired both ways with Input, that is not the one the window was lent to, dials the lent window's index (1000 to 1007, found by trying them: the app's own picker never offers one) as a screen: the session is refused at once (no picture, no input, its connection closed with "that window is not lent to this device"), while the first computer's view carries on. (where: second computer, a third)
- **Confinement, the pointer:** with the lent window partly covered by another window, moving over the viewer's picture where the cover is does not move the host's pointer into the cover (the pointer stays where it was); a click there raises the lent window first and then lands in it; a click in the viewer's window with the pointer never moved in it (a fresh view) is dropped. The pointer is never seen outside the lent window's frame on the host, at any scale, on the left monitor and on the 150 % monitor. (where: second computer)
- **Confinement, the keys:** the viewer types into the lent Notepad while the host's browser is in front and Windows refuses to give the focus back: nothing is typed into the browser (the keys are dropped, none sticks: Shift or Ctrl held in the viewer is let go on the host), and typing works again as soon as the lent window is in front. Pressing the Windows key, Win+D, Win+L, Alt+Tab, Alt+Esc, Ctrl+Esc, Ctrl+Shift+Esc, Print Screen and the volume and power keys in the viewer does nothing on the host (never run Win+L or a power key against the owner's own computer: watch for nothing happening). Alt+F4 in the viewer closes the lent window and only it. A dialog of the lent window (Notepad's Save As, a menu) takes the keys and the clicks without the window being raised over it. (where: second computer)
- **Confinement, the end of a lend:** the viewer keeps clicking and typing in its window while the host's owner gives the window back (from the host's side: the switch turned off, the window closed, or a return sent from the viewer's own second view): the session ends within about a second and nothing more reaches the host (no stray click, no key left pressed); lending another window to the same viewer is a new session. Lending, returning and lending the same window again in quick succession: the first session never drives the second. (where: second computer)
- **Confinement, what may be lent:** the desktop (Progman), the taskbar, Task Manager or any program run as administrator, and the app and server's own windows are not in the list, and a lend of one of their handles sent by hand (`0x` and hex) is refused as "No such window on this computer"; a window closed and another program started at once (a handle that may be reused) is never moved, sized or driven by the old lend (not forceable; unit-tested only). A borrowed window carries no sound of the host and no microphone. (where: second computer)

**281. A GNOME computer lends a window to look at (phase 6 of the window handoff: GNOME as the source)**

- A real GNOME desk (stock, no extension), "Let my computers borrow my windows" on, "Unattended on GNOME" on: the other computer's "Borrow a window…" says GNOME Shell does not give its window list and that it would take a Shell extension, in the picker's words; no window is lent and nothing is registered. With "Unattended on GNOME" off it says that instead. (where: second computer)
- A real GNOME desk where the list is given (a Shell started in its unsafe mode, from a throwaway session): the picker lists the windows by title and app, none of Pocketdesk's own and no hidden one; Borrow opens a window on the viewer within about two seconds showing that one window alone, at its own size, with no pointer of the host in it and none of the rest of the screen. Say how long to the first picture and how it looks scaled. (where: second computer)
- View only: clicking, scrolling and typing in the viewer's window do nothing on the host (no pointer movement, no focus change, no key typed into any window); the viewer's window says view-only (the ready stream clears input) and its screen buttons are the view-only ones. Say what the viewer shows. (where: second computer)
- The lent window covered by another (open a browser over it), and on another workspace: the viewer's picture is still the lent window's own, not the cover. Say what a covered window and a window on another workspace give (a frame, nothing, the last frame). (where: second computer)
- The lent window resized by its owner, maximised, and moved to another monitor while lent: the viewer's picture follows the window's size (the crop record changes; the encoder is rebuilt at the new size) with no frame of the whole screen in between; say how long it takes and whether any frame is the stage-sized one. (where: second computer)
- A window minimised while lent stays lent (not ended) and its picture stops or stays; restored, the picture comes back. A window closed ends the viewer's view within about two seconds, with its borrower told it closed and no leftover on the host. (where: second computer)
- Return by closing the viewer's window, the other computer going away (cable out, app killed), the switch turned off, the server stopped: each ends the capture within a second or two (the host's screen-sharing indicator goes), and a restart of the server lends nothing (no file is written). (where: second computer)
- A third paired computer dialling the lent index (1000 to 1007, found by trying them) is refused at once with the words of a window not lent; the window is not shown to it. (where: second computer)
- The phone's apps and the computer's other captures run beside a lent window with no change (a screen session of the host's monitor while a window is lent; the index range does not collide). (where: second computer)

**284. A KDE Plasma computer lends a window to look at (phase 6 follow-up of the window handoff: KDE as the source)**

- The first lend of a window shows the portal's dialog at the host and the viewer waits for it (up to a minute, then the session says no one answered). Choosing the lent window shows its picture alone (not the screen) within about two seconds, with no pointer of the host in it; say how it looks and how long the first picture took. (where: second computer)
- The kept grant: lend the same window again (return, then borrow): no dialog, the picture within about two seconds; the server's state folder has one owner-only file for that window and nothing else new. Restart the server with the window still open and lend again: no dialog. Close the window: its file is forgotten at the next lend or list. (where: second computer)
- The picture follows the window: the lent window resized, maximised, moved to another monitor and put on another virtual desktop while lent: say what the stream does (follows the size, stops, shows black) and whether a window covering it shows in the picture. (where: second computer)
- View only: clicking, scrolling and typing in the viewer's window do nothing on the host (no pointer movement, no focus change, no key typed into any window); the viewer's window says view-only. (where: second computer)
- A lent window minimised stays lent; one closed ends the viewer's view within about two seconds with the borrower told it closed; return, the other computer going away, the switch turned off and the server stopped each end the capture within a second or two, and the script is unloaded when nothing is lent (and a minute after a list nobody lent from): `org.kde.KWin /Scripting isScriptLoaded pocketdesk-windows` says false; a server killed with a window lent, or after only a list, leaves a script that the next first list or lend replaces, not doubles; a server stopped cleanly after only a list unloads it within two seconds. System Settings, Shortcuts, KWin lists "Pocketdesk: report the windows" while the script is loaded; after the unload say whether the entry stays. (where: second computer)

## A Linux desktop (the Azure VM serves for some)

**2. Session shell + video (Tier 1 #5, R2#2, R2#5)**

- R4 B3 capture_backend config override: set `capture_backend = "portal"` in (where: linux) config.toml, restart, start a stream → log shows "Capture started via backend Portal" and NO screencopy attempt line (pipeline pinned). With `capture_backend = "screencopy"` on a NON-wlroots/portal-only Wayland, the server must fail the session with a "Capture backend Screencopy unavailable" error rather than silently falling back to the portal (explicit intent surfaces). Omitting the key (or `"auto"`) restores the screencopy→portal fallback above. An unknown value (e.g. `"v4l2"`) makes the server refuse to start with a TOML parse error. (The unknown value seen 2026-10-30 on the Linux laptop with a throwaway server: `capture_backend = "v4l2"` stopped it at start, exit 1, "TOML parse error at line 7, column 19 ... unknown variant `v4l2`, expected one of `auto`, `screencopy`, `portal`, `x11`, `wgc`, `dxgi`". The portal pin and the non-wlroots case not run: the pin raises the share dialog at the laptop's desk.)
- R4 B1 idle pacing + freshness: keep the desktop static → `src` fps drops to ≤10 (where: linux) (damage-driven VFR); move a window → back to target fps within a frame or two, no tearing or upside-down frames (YInvert handling). (The static half seen 2026-10-30 on the Linux laptop, no phone: with its screen still, the screencopy backend logged "screen static, repeating the last frame each second" and the client received 1.7 to 2.0 fps in every 10 s window of four 40 s sessions. Moving a window, and the picture itself, not checked.)
- R4 B1 multi-monitor: pick monitor N in the picker → log line "Screencopy capture: output at (where: linux) (x,y)" shows that monitor's wl_output origin and frames are its size.
- R4 C2 mid-session encoder kill (HEVC session): during a live `codec=H.265` session, `pkill -f 'ffmpeg .*vaapi'` (or kill the encoder child seen in `pstree` of the server) → log shows "Rebuilt encoder on software rung (C2)", video resuming within ~2 s and the phone toasts "Desktop switched encoder — now H.264" (MediaCodec swap re-sniffed from the first software IDR).
- R4 C3 NVENC power pin (**NVIDIA hardware only — skip on non-NVIDIA boxes**): start an NVENC session (server log "Video encoder sidecar" on `h264_nvenc`/`hevc_nvenc`) → server log "pinned NVENC GPU 0 graphics clock to N MHz"; `nvidia-smi -q -d CLOCK -i 0` shows the graphics clock held at its max. End the session → log "restored NVENC GPU 0 to default clock management" and `nvidia-smi -q -d CLOCK` shows clocks back under adaptive management. On an unprivileged desktop the pin self-declines (log "clock lock refused ... running unpinned") and no reset is issued. (where: linux) (Not runnable on the Linux laptop as of 2026-10-30: its NVIDIA card is on nouveau, so there is no NVENC and no `nvidia-smi`.)

**2c. In-process VAAPI encode (R4 C4) — desktop/box-observable, no device needed**

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

- Linux: the app builds and opens on X11 and on Wayland (Hyprland), closing it quits (no tray on Linux yet), and the single-instance socket works. (Partly seen 2026-10-30 on the Linux laptop with throwaway folders: the release build opened a "Pocketdesk" window on Hyprland, and a second launch exited at once (exit 0) with the first still running and one app process. X11 and closing the window not run.)
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

- "Phones may add a screen" off: refused with its reason; on Windows, GNOME or KDE: "only Sway and Hyprland can for now" (Hyprland: §245).

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

- "Remember the screen-sharing grant" on: a session from the phone asks once at the desk; a second session and a reconnect start with no dialog. (KDE and GNOME headless above, by the probe; a phone session at a real desk owed.)
- Without uinput access: the phone's taps, typing (accents, emoji), keys and the wheel (its direction, one click a notch) reach GNOME through the RemoteDesktop portal; a remote without the screen is told it gets no input there. (Headless 2026-11-01: taps, keys, ASCII typing and the wheel arrived, accents not on a US layout, and a grant with "Allow Remote Interaction" off gave no input and no word to the phone; owed: a phone, a layout with accents, emoji.)
- Revoking in GNOME's settings brings the dialog back; Forget the grant and turning the setting off make the next session ask.
- No RemoteDesktop portal (or refused): the screen alone.
- The server started as a systemd user service: does the portal keep its restore data with no desktop app id? (Unnamed, KDE kept and restored both tokens under the empty id; the server now names itself `dev.pocketdesk.Pocketdesk` where the package's desktop file is installed. The unit itself is still owed on a live desk. Answered 2026-11-01 from the unit and systemd's rules: a user unit gets only the user manager's environment, never the session's, and Plasma's `startplasma-wayland` sets `XDG_CURRENT_DESKTOP`, `KDE_FULL_SESSION` and `KDE_SESSION_VERSION` itself and carries KDE's `KUpdateLaunchEnvironmentJob`, the job that pushes them to the user manager and D-Bus activation (read from the 6.8.2 binary's strings, not run), while `plasma-workspace.target` is `Before=graphical-session.target`, which the unit starts after; the same rig showed the converse on GNOME, where the manager had no `XDG_CURRENT_DESKTOP` until the session set it, and the portal then offered no ScreenCast. Still to see on a real Plasma desk: `systemctl --user show-environment | grep -E 'XDG_CURRENT_DESKTOP|KDE_FULL_SESSION'`.)

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

**121. The tray on its own (E2)**

- Linux: on a desktop with no tray host, the window quits on close.

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

**245. A second screen at the phone's shape on Hyprland**

- From a phone on the laptop: "Add a screen for this phone" in the picker, the session switches to it and a window dragged onto it shows on the phone; the screen goes about 10 s after the phone leaves. (where: linux)
- A Hyprland whose config has a monitor rule for every output (`output = ""` with a fixed position): the phone's screen still lands right of the others. (where: linux)

**246. Linux firewall: nftables and iptables, said at pairing**

- The desktop app on the laptop: the Pair page (Phone and Computer) and Settings → Other computers show the firewall line, and it goes within 30 s of the ports being opened. (where: linux) Half checked 2026-10-31: the desktop app for a throwaway in a headless sway on the laptop, its Pair page's Phone tab: "This computer's firewall (nftables) does not let other devices in on 1736/tcp and 4736/udp, so pairing will time out…" (its own ports). Not seen: the Computer tab and Settings (no clicks in the headless rig), and the line going (the laptop's firewall is never changed).
- The hint's `nft insert` lines run by hand: the row turns green and a phone pairs. (where: linux)
- iptables (`iptables.service` with a DROP policy): the row is red with the `iptables -I INPUT` lines. (where: linux)

**247. Shared input A: the receiving end, measured end to end**

- The uinput path on a Linux computer where `/dev/uinput` is writable: the same run, the receiver's write calls per move a quarter of before. (where: linux)

**248. Software encoding through OpenH264 on stock Fedora**

- A phone session against the tarball's server on a real stock Fedora machine with no working hardware encoder and `ffmpeg-free` plus `openh264`: the picture appears and stays, Diagnostics' last session names `ffmpeg-sidecar-openh264`, and the picture quality at the default bitrate is acceptable. (where: linux)
- install.sh on a fresh stock Fedora: it lists `ffmpeg-free` and `openh264` and, once they are installed, `pocketdesk capabilities` names libopenh264; with `fedora-cisco-openh264` disabled it says how to enable it and enables nothing. (where: linux)

**249. Fine scrolling end to end**

- Firefox and a GTK app under Sway or Hyprland without uinput: a slow two-finger scroll moves the page about as far as the same scroll on a laptop's own touchpad, with no jump at notch boundaries. (where: linux)
- X11 (an Xorg session without uinput, through XTEST): the wheel scrolls up and down the right way, a click a notch. (where: linux)
- KDE Plasma through the portal: a slow touchpad scroll from the window or the phone moves a few pixels at a time (`NotifyPointerAxis` pixels, 3.75 a quarter notch), the right way. (where: linux)
- uinput on a Linux computer where `/dev/uinput` is writable: `libinput debug-events` shows `v120` values below 120 for a slow scroll, and a mouse wheel's notch from an older phone app still scrolls one notch. (where: linux)
- The phone's trackpad: a slow two-finger scroll moves the page smoothly; with Settings' Smooth scrolling off it moves a notch at a time as before. (where: linux)

**251. Shared input B: the driver loop, batched and kept light**

- A running server at rest with driving on: the `pd-shared*` threads wake about 7 times a second at home (`/proc/<pid>/task/*/status`), and its receive loop about once a second while nobody drives this computer. (where: linux)

**252. The phone's camera as this computer's webcam (Linux)**

- "Offer phones as a camera" turned off and "Stop camera" on the Overview each end a running camera, and the "in use" notification shows plainly (default importance, silent) on the phone. (where: linux) Partly checked 2026-11-01 (the OPPO, a laptop throwaway on a private PipeWire, counts only): with the setting turned off the running camera ended and the node was gone in 0.7 s, the phone's "in use" notification with it; the Overview's Stop camera, as its seam (a device kick, what `end_sessions_of` writes), took the node away in 0.6 s and the notification too; the notification's channel is of default importance (3) with no sound and no vibration. The desktop button itself was not clicked.
- A real video call on the owner's desk: Firefox (and Zoom or Meet in the browser) picks "Pocketdesk camera (<phone>)" in its camera list, the other side sees the phone's picture, and it stops when the phone's notification's Stop is pressed. (where: linux)
- OBS 30+ and GNOME Snapshot list and show the camera; `pw-v4l2` lets a V4L2-only app (e.g. `pw-v4l2 ffplay /dev/video0` or Cheese) open it. (where: linux) 2026-11-01: `pw-v4l2` is not installed on the laptop (and nothing was installed for it); not run.
- The ask refused: Webcam off in Devices (the button is greyed and says why), the phone's switch off (no notification), and the setting off (a camera cast from an older ask is closed with "does not offer phones as a camera"). (where: linux) Partly checked 2026-11-01 (the OPPO): the phone's switch off left no notification on the phone; the setting off dropped the ask ("does not offer phones as a camera", no notification) and ended a running camera ("phones are no longer offered as a camera", 0.7 s); Webcam off for the phone dropped the ask ("its permission is off", no notification). The Devices button's greying and its reason are desktop UI, not looked at.

**254. Unattended on GNOME (mutter's own D-Bus, no dialog)**

- The first-input race on a real session: a pointer move sent before an app has bound the seat's pointer is lost to that app until the next time the pointer enters it (seen with `wev` started before the session); check a click right after a session starts lands in a window the pointer was already over. (where: linux)
- A real GNOME desk (Ubuntu 24.04 or newer, Fedora 41 or newer) with the switch on and no one at the desk: a paired phone starts a session with no dialog, the picture and a tap and a typed word arrive, the screen-sharing indicator shows while it runs and goes when the phone leaves, and nothing is left in `~/.local/state/pocketdesk` for it (no restore token). (where: linux)
- The same with two monitors: `monitor_index` 1 records the second one (the logical monitor at its corner), and an absolute click lands on it. (where: linux)
- With the switch off, or on KDE or Sway with it on, the capture goes through the portal as before (no mutter call in `busctl --user monitor org.gnome.Mutter.ScreenCast`). (where: linux)
- An older GNOME whose mutter has the ScreenCast API but no `org.gnome.Mutter.RemoteDesktop` (or refuses it): the same single log line and the portal's dialog. (where: linux)
- The desktop app's Settings → Connection shows "Unattended on GNOME" on Linux only, off by default, and a session started after turning it on takes the new path with no server restart. (where: linux)

**256. A direct link (no shared network): this computer's own Wi-Fi, joined by the phone for Pocketdesk only**

- A throwaway server (not the laptop's own ssh Wi-Fi: a USB Wi-Fi adapter as the card, the built-in one keeping ssh) with a request left in `direct_link_out/` (or the Pair page's button): the hotspot comes up (`nmcli -t -f NAME,TYPE connection show --active` names "Pocketdesk direct link"; print nothing else), `direct_link.json` says `on` with an address, mode 0600, and `nmcli connection show` lists no saved "Pocketdesk direct link" (volatile). (where: linux)
- The second-interface road: the server run with `AmbientCapabilities=CAP_NET_ADMIN` on a card whose `iw list` allows AP and managed together: the plan is "second interface", `iw dev` shows `pdlink0` of type AP beside the card's managed interface on the same channel, the laptop's own connection never drops, and `iw dev pdlink0 del` has run when the link ends. (where: linux)
- The takes-over road, on the USB adapter only: the Pair page says "This computer's Wi-Fi will disconnect while the direct link is on" before the first click, and asks once more ("Disconnect Wi-Fi and make the link"); a request without `accept_takeover` is refused with a status of `failed` and nothing changes. (where: linux)
- The OPPO, in range, scanning the Pair page's code from a screenshot pushed by adb: Pocketdesk asks "Pair with the computer at 10.42.x.1?" with the direct-link note, Android's own prompt to join `pocketdesk-…` comes up once, the phone joins, pairs, and a session starts; a file moves both ways (counts and a checksum only), and a 30 s session counts frames. `dumpsys connectivity` meanwhile shows the phone's mobile data still the default network. (where: linux)
- The OPPO refusing Android's prompt (or the code scanned out of range): "The phone did not join the computer's Wi-Fi", nothing paired, nothing left bound, Cancel and back both leave. (where: linux)
- After the session ends, and after Cancel, the phone is back on its own Wi-Fi (`dumpsys connectivity | grep -c WifiNetworkSpecifier` is 0) and Pocketdesk's sockets are no longer bound. (where: linux)
- "Join computers' direct links" off on the phone: scanning a direct link says joining is turned off, joins nothing. (where: linux)
- The idle timer on a real link: with no phone it turns off after the setting (a 1-minute setting is enough), the Pair page counting down; with a phone connected it does not, and starts counting again once the phone leaves. (where: linux)
- The server stopped (`pocketdesk stop`) with a link on: the hotspot is gone at once; the server killed with `kill -9` and started again: the first start's log says it swept one, and no "Pocketdesk direct link" is left. (where: linux)
- A firewall on the computer (firewalld, then ufw with its default deny): the phone reaches 1716 and 4716 on the hotspot's address under firewalld's `nm-shared` zone, and what ufw needs is what the guide says. (where: linux)
- The Settings rows: "Offer a direct link" off hides the card and a running link goes off within 5 s; the band row's 5 GHz on a card that allows it makes a 5 GHz network, and on one that does not says why the link failed. (where: linux)
- The takeover's restore, on the USB adapter only: with the adapter joined to a network, make a link (taking over), then turn it off: the adapter's own connection is active again within seconds (`nmcli -t -f NAME,DEVICE connection show --active`, names not pasted), and the same after the server is stopped with a link on, and after a link that fails to come up (for example `dnsmasq` missing). (where: linux)
- The sweep is this folder's: with the main server's link on and a second throwaway server (its own state folder) started, the main link stays up and the throwaway sweeps nothing; with the main server killed and started again, its first start removes its own left-over link (and `pdlink0` if it made one) and restores the card's connection. (where: linux)
- The longest life: with a phone connected (so never idle), the link goes off at eight hours (a test build with the constant cut to a minute is enough). (where: linux)
- The OPPO opening a direct link from another app (an `adb shell am start -d "pocketdesk://pair?..."` link, not scanned): Pair first, then a second screen naming the Wi-Fi network and asking again; "Join the network and pair" is the only way on, and Cancel and back both leave with nothing joined. After `adb shell am kill` or a rotation mid-pairing, the restored link has no Wi-Fi keys. (where: linux)

**266. The computer turns on the phone's hotspot (Settings, Let my computers turn on my hotspot; Overview, Use my phone's hotspot)**

- Joining on Linux with NetworkManager: with "Join my phone's hotspot network" set to the phone's saved Wi-Fi name, "Use my phone's hotspot" says "Joining ..." and the computer joins it once it appears (the notice says "Joined"); with a name that is not saved here it says so and joins nothing and asks for no password; with the setting empty it only tells you to connect. No password prompt appears and none is stored by Pocketdesk. (where: linux)
- Two saved networks of one name on Linux: with the phone's name saved twice in NetworkManager, nothing is joined and the notice says so; with one, it is joined by UUID (`ps` while it joins shows `nmcli ... connection up uuid ...`, no name or password). (where: linux)

**268. A still screen sends nothing to another computer's window (the window's stats, a still desktop)**

- A Linux host on Hyprland or another wlroots desktop (screencopy's heartbeat), viewed from another computer: the same stillness numbers as the first item and a change shown at once. (where: linux)
- Hyprland, a change the compositor did not report: Hyprland says "no damage" at once, so the capture's once-a-second copy is the only way some changes get out. Change the screen (type, play a video, drag a window) and let it rest: the window shows each new picture within about a second, none is dropped as a repeat, and a screen that then stays the same goes silent after the settling repeats. (where: linux)

**269. VA-API encodes each frame as it comes (phase 1b of the window handoff: `async_depth` 1, a yield after each packet send)**

- An AMD host with VA-API (Mesa radeonsi, `pocketdesk bench --target vaapi --frames 300` before and after, and the input-to-picture probe with `PD_LIVE_HEVC=1` too): the stage table's "enc: sent > packet out" near 0 and "frames in before its out" 0 for H.264 and HEVC, the encode call's p50 within the frame period at 1080p60 (the bench's p50 well under 16 ms), and no more dropped frames than before (the pictures count of the probe's run). Record the numbers in the status history. (where: linux)
- An Intel host newer than Ice Lake, and a slower one (an Atom or Celeron iGPU): the same at 1080p60 and at 1440p or 4K at 60, where the sum of the copy, conversion and encode may pass the frame period; if it does, the session's picture rate falls below the asked rate (the stats overlay's frames a second in). (where: linux)
- A host whose FFmpeg lacks `async_depth` on the VA-API encoders (an older FFmpeg, or a vendor build without it): the open logs "ignored async_depth: it may hold a frame until the next" and the stream works as before. (where: linux)
- The sidecar VA-API rung (a host where the in-process VA-API open fails and the ladder falls to the sidecar, or a build without the `inproc` feature): the stream starts and the FFmpeg child accepts `-async_depth 1` with both `h264_vaapi` and `hevc_vaapi`. (where: linux)

**270. Lending a window to another computer (the host side, phase 3a of the window handoff)**

- On a real desktop session: the Arch laptop's own Hyprland, "Let my computers borrow my windows" on in Settings, Other computers, a computer paired both ways with Input allowed asking for an unimportant window (a terminal): the window leaves the laptop's screen, its picture is on the asking computer at its size, and typing and clicking there reach it; returned, it is on its old workspace, tiled or floating as it was, and the extra screen is gone. (where: linux)
- The same on a real Sway session, with a window that has a title bar and border (the default `border normal`): put back with the same title bar, border and size. (where: linux)
- The switch off (the default): the same ask is answered that the computer does not lend its windows, and nothing moves; a phone and a computer paired one way are told only a computer paired both ways may. (where: linux)
- The borrowing computer's session, connection and link all gone (close its window, then stop its server): within a couple of seconds (the watcher's look a second; the 5-second sweeps are the backstop) the lent window is back and its screen gone. (where: linux)
- The server stopped (`pocketdesk stop`) with a window lent: the window is back on its workspace before the server exits. A server killed (`kill -9`) with one lent, then started again in the same compositor: the window is back and the extra screen gone at start. (where: linux)
- A browser (or any program that hides its toolbars when it is told it is full screen) lent on Hyprland keeps its toolbars: the program is not told it fills the screen. (where: linux)
- A window closed while lent has its empty screen removed within a couple of seconds, and the window list no longer shows the window. (where: linux)
- A tiled window lent from a workspace with others comes back to that workspace; it may not come back to its old slot among them (the limit the guide names). (where: linux)
- A window that will not go back stays lent and is retried: on a throwaway Sway or Hyprland make the compositor refuse the move back (a workspace name the record cannot use, or the compositor socket briefly unreachable), then ask `return`: the answer is an error, the window and its screen are still there, and once the compositor works again the next `return`, or the sweep after the borrower is gone, puts it back and takes the screen away. (where: linux)
- An X11 (Xwayland) program's window on Sway lent and returned: its title and class are in the window list, and it moves and returns like a Wayland one. (where: linux)

**271. VA-API converts BGRA to NV12 on the card (phase 1c of the window handoff: `scale_vaapi`, no processor conversion)**

- An AMD host with VA-API (Mesa radeonsi): the encoder's open logs "colour conversion on the card" (not the "unavailable ... converting on the processor" warning); `pocketdesk bench --target vaapi --frames 300` with and without `POCKETDESK_VAAPI_CPU_CONVERT=1`: the card path's encode call p50 no higher, the CPU use of the server lower; the probe (`POCKETDESK_TEST_PATTERN=probe`, `POCKETDESK_LATENCY_TRACE`) shows "enc: upload to card" and "enc: BGRA > NV12 (card)" and no copy or processor-conversion rows. Colours as before: a picture of a colour test card (or the probe's pattern) next to the old path's, no shift in skin tones, greys or saturated reds and blues (the card path is asked for BT.601, limited range, as the processor path always produced). Record both in the status history. (where: linux)
- An Intel host newer than Ice Lake (Tiger Lake, Alder Lake, Arc) and an older or slower one (Skylake to Comet Lake, an Atom or Celeron iGPU): the same, at 1080p60, and at 1440p or 4K where the processor path could not keep the rate; on a very old driver (i965) the log may say the card's conversion is unavailable and the stream must still work on the processor path. (where: linux)
- A driver or FFmpeg that cannot do it: the open logs "conversion on the card unavailable (...); converting on the processor" once (later opens, for a resolution change or a new session, log it at debug only) and the stream works as before. Try an FFmpeg without `scale_vaapi` (a vendor build) or a VA-API driver without BGR0 surfaces. (where: linux)
- A host with two GPUs (Intel and NVIDIA, as the laptop): the encoder opens on the render node whose driver has VA-API, and the card path works there; with `POCKETDESK_VAAPI_DEVICE` pointing at a node with no VA driver the open fails as before and the ladder goes on to the sidecar. (where: linux)
- A resolution change in a live session (the host's display changes mode, or the viewer asks for another size): the encoder is rebuilt at the new size and the picture continues on the card path, no restart of the session and no "does not hold" error in the log. (where: linux)

**272. Borrowing a window of another computer (the viewing side, phase 3b of the window handoff)**

- The same on Hyprland 0.56 as the host (a headless Hyprland, or a real session): `list` names the windows, a click aimed at the lent screen lands on the window (the live test's check, which has only run on Sway). (where: linux)

**278. A borrowed window takes the size of its window (resize follows the viewer, window handoff)**

- Sway: several outputs placed right of the lent screen (a second lent window's screen) when the first is enlarged: their pictures stay their own (Sway's outputs keep their places, so a larger one overlaps the next one's; the capture takes each by its own origin). (where: linux)

**273. The window's wait for the display (phase 1d of the window handoff: a setting to draw without vsync, and the window's stage trace)**

- On a real Hyprland desktop (and a real GNOME and KDE session), the setting off then on, the window showing a moving screen at the display's refresh: the picture is a frame sooner (a high-speed camera or the probe's marker against a stopwatch shows it), and Wayland shows no tearing. (where: linux)
- X11 (an X11 session, or XWayland with the window forced onto it): the setting on can tear, and the setting says so; off never tears. (where: linux)
- The app's other windows with the setting on: the main window's animations do not run at hundreds of frames a second for long, and the tray, the other computer windows and the notices are as before. (where: linux)
- The same measurement between two real computers over Wi-Fi: `view_present_live` cannot cross a network, so read the window's own Stats (frames in and shown, decode time) with the setting off and on. (where: linux)

**274. A window the compositor does not show no longer stops the app (the swap never waits on Wayland, and the windows are paced)**

- On the owner's Hyprland desktop (and a real GNOME and KDE Wayland session): a computer's screen window moved to a workspace that is not shown, and one covered by another window, leave the main window and the other windows drawing (the app's window animates, its tray works), and the window draws again when shown. (where: linux)
- Hyprland, GNOME and KDE: no tearing and no visible judder in a computer's screen at the display's refresh with the swap not waiting (a moving screen at 60 and 120 Hz), and the main window's animations are smooth. (where: linux)
- A computer's screen window that is not shown on Wayland keeps decoding and drawing its pictures (the compositor shows no signal of it): its cost on the processor and the card is about the same as shown, so say what a long hidden window costs on a battery. (where: linux)
- X11 (an X11 session, or XWayland with the window forced onto it): the swap still waits by default (no tearing), a window covered or on another workspace does not stop the app (X11 reports it covered, and eframe stops drawing it), and the Settings switch on makes pictures sooner and tears at most. (where: linux)

**275. The other computer's pointer drawn here (local cursor, phase 2 of the window handoff)**

- The setting off: the pointer is in the picture as before (and, from a GNOME or KDE host, as the portal gave it) and the window's pointer is the system arrow. A change applies from the next view. (where: linux)
- A very large pointer (an accessibility size, over 256 pixels) shows shrunk, not cut; a pointer that changes often (a program animating it) costs no more than the shapes it sends, each sent once. (where: linux)

**277. Zero-copy capture (low-latency window handoff, phase 2: the screen goes from the compositor to VA-API as a DMA-BUF)**

- A real Sway on its DRM backend (not headless), a monitor at its native size: "Zero-copy capture: ..." in the log, the picture right, no tearing, a still desktop sends one frame a second, and the frames a second under a moving window are the monitor's rate. (where: linux)
- The owner's Hyprland session, whole screen: the capture takes DMA-BUFs (a Hyprland screencopy that reports no damage at once: a still screen sends its heartbeat each second, as repeats only while the picture is the same, read back by the card), the picture is right, and a window crop from a phone (Hyprland's toplevel export) still works and still copies. (where: linux)
- A window crop on a host without the toplevel export (Sway), set mid-session from a phone: the log says "Capture back to copies through memory", the stream goes on at the crop's size, and a second session starts on DMA-BUFs again. (where: linux)
- An output whose size changes mid-session (a monitor's mode, a window's fitted screen resized): new buffers are made and checked, the encoder is rebuilt at the new size, and the stream goes on. (where: linux)
- X11 hosts: unchanged, the capture copies as before, and the new Settings row is shown and harmless (GNOME and KDE are §282). (where: linux)

**279. Dragging a window past the edge (phase 4 of the window handoff)**

- Sway as the offering computer: it has no input-capture portal, so no edge is watched; the pick of the window in front and its return were live-tested in a headless Sway (a floating window at the edge picked, a tiled and a full-screen one left, a drag back put the container's top left where asked, 1.0 s). Whether Sway will ever drive the shared input is a separate question. (where: linux)

**282. Zero-copy capture on the PipeWire path (low-latency window handoff, phase 2b: GNOME's and KDE's screen goes from the compositor to VA-API as a DMA-BUF)**

- KDE Plasma 6 (KWin) through the portal, in a throwaway container (KWin as `kwin_wayland --virtual`, the rig of §100's KDE check): the log says "The PipeWire stream settled on DMA-BUF buffers", the picture is right, and the frame rate and the host's processor use against `capture_copy` on. Say which modifier KWin fixed and whether it fixed one itself or left it open. (where: linux)
- GNOME through the portal (the dialog path, "Remote Desktop"/"Share Screen") on a real or headless session: the same as above, and the tearing check: a fast-moving window (a video, the fishbowl) over several minutes with no garbled or half-drawn frame in the viewer (the buffers' implicit fences are all the capture waits on; the rig cannot show a tear). (where: linux)
- The local cursor with DMA-BUF buffers (a viewer that draws the pointer itself): the pointer's metadata arrives on every buffer, the picture has no pointer in it, and a buffer that only moved the pointer is queued back at once (`chunk.size` 0). (where: linux)
- The screen's size changes mid-session (a virtual monitor's mode, a window's fitted screen): the stream renegotiates, the new buffers are imported once each, the encoder is rebuilt at the new size, and the stream goes on on the card. (where: linux)
- A phone's window crop (the window pick), set mid-session on a GNOME or KDE host: the log says "Capture back to copies through memory", the stream asks for memory buffers and goes on at the crop's size. A lent GNOME window (`RecordWindow`) copies from the start, as before. (where: linux)
- A session pinned to NVENC or the software encoder on a host with VA-API: no DMA-BUF format is offered (the log shows no "offers DMA-BUF buffers" line) and the capture is as before. (where: linux)
- Cropping on the card (`scale_vaapi`'s crop, so a window pick or a lent window need not copy): not built. (where: linux)

**284. A KDE Plasma computer lends a window to look at (phase 6 follow-up of the window handoff: KDE as the source)**

- The script loads: `list` on a KDE host loads the script (`org.kde.KWin /Scripting loadScript` then `run` on `/Scripting/Script<id>`; if that object path is not right on this KWin, the fallback `start`), the first report arrives within three seconds, and `list` names the ordinary windows of programs by title and app, none of Pocketdesk's own, no panel, desktop, menu, notification or dialog. Say which KWin version and whether the script's object path or the fallback worked. (where: linux) Partly checked 2026-11-06 (a GitHub runner's throwaway Arch container, KWin 6.8.2 as `kwin_wayland --virtual`, kglobalacceld 6.8.2, `.github/workflows/rig-kde.yml` on a `rig/` branch): the script loaded, the first report came at once (`LIST_SECS 0.0`) and `list` named the one `wev` window by title and class with its size; the rig has no panel, desktop, menu, notification or dialog, so leaving those out is not shown.
- The report's facts are right: a window's frame, minimised flag, "shown" (a window on another virtual desktop is not shown), transient-for parent (a dialog), the stacking order (a window raised or lowered), the active window, and `normalWindow` (a panel, the desktop, a menu and a notification are not ordinary). Say which of `normalWindow`, `skipTaskbar`, `desktops`, `transientFor`, `stackingOrder` and `activeWindow` did not behave as the script assumes. (where: linux)
- Only KWin's report is taken: a second program on the session bus calling `dev.pocketdesk.Windows.Update` with a table that names a made-up window changes nothing (no new window listed, the table's age unchanged). (where: linux)
- Refusals in words: a KDE session on X11 says it needs Wayland; the KWin scripting interface unavailable (or scripts disabled in `kwinrc`) says KWin did not take the script; xdg-desktop-portal stopped says the portal is missing; a portal whose `AvailableSourceTypes` lacks windows says it cannot share a single window; a non-KDE desktop never picks this path. (where: linux)
- The match: choosing another window in the dialog (of a different size) ends the session saying the window cannot be shown to be the lent one, shows nothing, and keeps no grant; choosing a monitor says it was a screen; two windows of the same size lent or chosen: the session refuses (cannot tell which), and say whether the portal names the window (its stream id or mapping id) so that twins can be told apart. Say what size KWin reports for a window's stream against its frame and client geometry at scale 1, 1.25 and 1.5 (the match compares device pixels, the client area or the frame times the output's scale, within 2 pixels): which of the two it is, whether it includes a shadow, and whether a window with a server-side title bar differs; and that the script's `output.scale` and `clientGeometry` read right. A window on a fractional-scale output is lent without being refused, and a 533x400 window at 1.5 and an 800x600 one at 1 are refused as twins. (where: linux)
- Input, once it is wired (not built): a click, scroll and key through the portal's RemoteDesktop on a window source land in the lent window only, with another window over it and with another window active; the guard's questions over the table (`window_kde::KdeOs`) agree with what KWin does. (where: linux)

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

**258. Open any phone app from the computer (Phone apps page, Apps part 1)**

- Whether a background start by the control service itself works on the OPPO (ColorOS 14) and, if it can, on a phone with Android 15 or 16: the app starts from the service's own context with the phone's screen on another app. If it only appears behind, the fallback (N3's opt-in) is the next thing to try. (where: other hardware) Partly checked 2026-11-01 on the OPPO (ColorOS 14, Android 14): yes — with Calculator in front and the cast shown, an open of Settings from the control service's own context brought Settings to the front (`opened`). Android 15 or 16 hardware was not available.

**261. Phone apps on the computer's Start menu, from notifications, and dropped files shared into the app in front**

- A computer that is too old (a throwaway built from before this change): the phone says the computer cannot keep folders yet and sends nothing; no copies appear in that computer's Downloads. (where: other hardware)

**262. A spare phone at home as the wake helper (Settings, Wake helper for this home)**

- A stranger: a phone paired with nothing, or another owner's phone, dialing the helper by its key is closed (the helper's counts show a dropped request, no packet sent). A request line with a MAC in it sends nothing (the packet capture shows none). (where: other hardware)

**263. Phone apps in windows of their own, several at once (Settings, Separate windows (Shizuku))**

- An older computer (a throwaway built before this change) answers no such ask; a computer without the setting (`phone_app_windows` off in the desktop's Settings) opens apps on the phone's screen as before. (where: other hardware)

**264. Wear OS tile, complication and list (Settings, Watch controls; `android/wear`)**

- **The one assumption the design rests on:** a secure RFCOMM socket from the watch to the phone's Pocketdesk service connects on a real Wear OS 3, 4 and 5 watch (a Pixel Watch and a Galaxy Watch, if both can be had). On the phone, Settings → Watch controls on, then on the watch Pair: the connect succeeds and the phone logs "a watch was paired" (no address). If `connect()` fails on a model ("read failed", "service discovery failed") note the model and Wear OS version: the fallback is an L2CAP channel (Android 10 and later) or a BLE GATT service behind the same `watchlink` frames, which only changes the socket in `PhoneLink` and `WatchLinkServer`. (where: other hardware)
- Pairing: Settings → Watch controls → Pair a watch, then on the watch Pocketdesk → the phone → six digits; typing them on the phone pairs ("Paired with ..."). A wrong code: the watch says "Not paired: the phone refused"; three wrong codes from the watch shut it out until the window is opened again. A second bonded device (earbuds, a car) that connects during the window and fails does not use up the watch's tries. Closing the dialog, or two minutes, closes it: a later watch connect gets nothing. With the window closed, a bonded device that connects is dropped at once. (where: other hardware)
- The permission: turning "Watch controls" on the first time shows Android's nearby-devices question once; declining leaves the switch off; the phone's Settings never asks for location. On the watch, Pocketdesk asks for the same once. (where: other hardware)
- The tile: add it on the watch; it shows the buttons the phone has on (and no Deck chip by default), the computer's name, and "Nothing is on in the phone's Watch controls" when every switch is off. Turning a switch off on the phone removes its chip the next time the tile is shown. (where: other hardware)
- An "Approve with my phone" request (sudo): the list shows its command and "Yes needs your phone's fingerprint"; there is no Yes on the watch; No on the watch denies it; Yes on the phone still takes the fingerprint and signs. (where: other hardware)
- The phone not near: switch the phone's Bluetooth off, or walk out of range: the tile says "The phone is not reachable" within about twelve seconds, and a wake pressed then does not go out when the phone comes back. Phone's Pocketdesk "Watch controls" off: the watch says the buttons are off or the phone is not reachable; "Forget" on the phone: the tile says "Pair with the phone first". (where: other hardware)
- The complication ("PD") opens the list from a watch face that offers it; it shows no live data and never refreshes. (where: other hardware)
- Battery and idle: with the setting on and the phone idle overnight, the tile still works in the morning and the phone's battery stats show no wake lock for Pocketdesk's Bluetooth service; the tile's own connect ends within the timeout and leaves no open socket on the phone (`adb shell dumpsys bluetooth_manager` shows none after a press). (where: other hardware)
- The size: the release watch APK is about 1 MB; note the installed size on a watch. (where: other hardware)

**265. Keys to taps: a phone game played from the keyboard and mouse (Settings, Keymaps for phone apps)**

- Aiming: with an Aim area, "Aim with the mouse" (and Ctrl+Alt+Shift+M) hides and locks the pointer to the window; moving the mouse drags one finger around the area (a camera in a 3D game or a map pans); still for a moment lifts it, moving again puts it down in the middle; running to the area's edge lifts and puts it down in the middle again; a mouse button mapped as a key taps; Ctrl+Alt+Shift+M again or Ctrl+Alt+Shift alone frees the pointer. Try a 1000 Hz mouse and a 144 Hz screen: no lag builds up and the phone logs no flood. (where: other hardware)
- An older phone (a build before this change) with a keymap played: it ignores the finger batches and still takes touch and typing; the desktop does not break. (where: other hardware)

**266. The computer turns on the phone's hotspot (Settings, Let my computers turn on my hotspot; Overview, Use my phone's hotspot)**

- The first check, per Android: "Use my phone's hotspot" starts the phone's Wi-Fi hotspot within about 15 seconds with no tap and no screen on the phone, under its own saved name and password (the computer sent neither), Android shows its usual hotspot indicator, and the helper's log line says it started. Record the Android version and maker for each phone tried; if the helper logs "no tethering manager" or "did not start", record the exact line (the call may need another route on that Android). Android 11, 12, 13, 14 and 15 are each owed. (where: other hardware)
- An older phone (a build before this) or an older computer: the computer shows no buttons for a phone that never reported `hotspot`; an older phone drops an `hotspot_on` ask it does not know and nothing breaks. (where: other hardware)

**278. A borrowed window takes the size of its window (resize follows the viewer, window handoff)**

- Hyprland 0.56 as the host (never started on the Arch laptop beside the owner's session): the lent window fills its resized screen at once through the compositor's full screen, and `hypr_refill` (out of full screen and in again) puts one right that did not; the index a session captures it by is still right after the output is placed right of the others again (`j/monitors`, then the next view's dial). (where: other hardware)
- A resize the host cannot do (Hyprland refuses the monitor rule, or the mode takes more than 2 s to show): the borrowed view stays up at the size it had, the window is not returned, a later resize is tried again, and the loan's capture index is right after it (the window is still there a minute later). (where: other hardware)

**276. Smooth keyframes (low-latency window handoff, phase 1: no whole keyframe on a two second beat)**

- Windows with an NVIDIA card, in-process NVENC, H.264, the setting on: the stream opens, plays, and a keyframe still comes on request (`forced-idr`), with no keyframe on a two second beat in the log of a session; HEVC through NVENC with the setting on gets whole keyframes every 10 s and on request (no intra refresh: a wave's start is told from an IDR only for H.264). `h264_nvenc` with `intra-refresh` is untested on a real card. (where: other hardware)
- An AMD, Intel Windows or NVIDIA-on-Linux computer (the FFmpeg sidecar): with the setting on nothing changes (a keyframe every two seconds, no intra refresh), and the setting off is as before. (where: other hardware)

**277. Zero-copy capture (low-latency window handoff, phase 2: the screen goes from the compositor to VA-API as a DMA-BUF)**

- A wlroots compositor that answers `failed` to the import of a modifier it advertises (a driver quirk; Sway on this card imports all it offers): the next modifier is used, or the capture copies with one warning, and the connection is never closed with `invalid_wl_buffer`. (where: other hardware)
- A compositor whose `zwp_linux_dmabuf_v1` is version 3 or older (no main device): the log says once that the capture copies because the compositor's GPU is not named, and the stream plays. (where: other hardware)
- AMD and other Intel generations: the output surface of the video processor is ready when the conversion returns (the unit test `the_buffer_is_free_when_the_conversion_returns` passes and is not skipped), and an empty desktop reaches the card as linear after a few seconds. (where: other hardware)
- AMD (radeonsi VA-API) with Sway or Hyprland: the log's "Zero-copy capture: ... modifier=..." line names a modifier the compositor and the card agree on, the picture is the same as with "Copy the screen through memory" on (look at it, then compare the frames a second and the host's processor use), and a card whose import fails turns the capture back to copies with one warning. (where: other hardware)
- Other Intel generations (Gen 8 and 9 with i965 or iHD, Xe with its 4-tiled modifiers): the same, and the modifiers with a second plane (compressed ones) are skipped. (where: other hardware)
- A hybrid laptop whose compositor renders on the discrete card while the encoder opens the integrated one: the log says "the compositor renders on another GPU" once and the capture copies; with both on one card the DMA-BUF capture works. (where: other hardware)
- NVIDIA (proprietary driver; NVENC or the software encoder as the encoder): no DMA-BUF is made (only the in-process VA-API encoder takes one) and the capture is unchanged. (where: other hardware)

**279. Dragging a window past the edge (phase 4 of the window handoff)**

- xdg-desktop-portal-hyprland with a button held at the barrier: the capture activates (the pointer crosses) while Super and the left button are down, and libei then sends the left button's and Super's release with no press before them, which is what the driver reads. If the compositor ends or refuses the capture while a button is held, or sends no release, say which (the log at debug names "no window to hand over" or nothing). (where: other hardware)
- The window moved locally before the drag crossed: as the capture takes the pointer, Hyprland's own window drag ends cleanly (no window left half-dragged, stuck to the pointer, or the button stuck down over there). (where: other hardware)
- A text selection dragged to the edge and over it, with the default trigger: no window is taken. With "Any left-button drag": a selection inside a floating window at the edge takes that window (said in the Settings note). (where: other hardware)
- A tiled window, a full-screen window, a drag with the right button (Super and right drag resizes): the pointer crosses and nothing is taken or lent. (where: other hardware)

**282. Zero-copy capture on the PipeWire path (low-latency window handoff, phase 2b: GNOME's and KDE's screen goes from the compositor to VA-API as a DMA-BUF)**

- AMD (radeonsi VA-API) with GNOME or KDE: the offered list, the modifier the compositor fixes, the picture, and that PipeWire's mapping of the buffers (the stream is connected with `MAP_BUFFERS`) does not fail for a buffer the card cannot map into memory. If the stream errors in DMA-BUF mode the capture connects it again on memory buffers once (the log says "the stream failed with DMA-BUF offered" and "connected again, memory buffers only") and ends only if that fails too: say which happened. (where: other hardware)
- NVIDIA or a hybrid laptop with the proprietary driver: the capture's probe runs libEGL and VA-API inside the server process, so a driver that crashes there takes the server down (no process isolation is built); start a session with the PipeWire capture and say whether the probe returned or the server died. (where: other hardware)
- Other Intel generations (Gen 8 and 9 with i965 or iHD, Xe with its 4-tiled modifiers) and NVIDIA (the proprietary driver: no VA-API device, so nothing is offered and the capture is unchanged). (where: other hardware)
- A hybrid laptop whose compositor renders on the discrete card while the encoder opens the integrated one: the card refuses the compositor's buffer, the log says once that the capture is back to copies, and the stream plays. (where: other hardware)

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

**236. The rest of the pages (Desktop UX 4)**

- Pair → Phone at the smallest window (640×440): the code is smaller but scans from the OPPO, and the PIN and "Whose phone is it?" are whole. (where: OPPO)

**243. The card menu, window chips and font scale (Android UX 2)**

- The same for "Play this phone's sound there" (it makes sound, so never in a script run). (where: by hand)
- TalkBack on the card menu: the two headings are read as headings, Manage as collapsed and then expanded; on a window chip, the whole title and the "Show the full title" action. (where: by hand)

**257. A direct link on Windows: the Mobile Hotspot**

- The Settings rows on Windows: "Offer a direct link" off hides the card and a running link goes off within 5 s; the note names the Mobile Hotspot, not NetworkManager. (where: by hand)

**258. Open any phone app from the computer (Phone apps page, Apps part 1)**

- A click with no screen shown yet: the page says to tap the notification; the phone shows "Tap to show your screen and open …", its tap brings Android's one screen-capture question, and after Start the app opens and the window here shows it. A second app then opens with no question and no notification (the one consent serves the connection). With Android 14's "single app" choice picked in the question: what the window shows (the app chosen, not what is opened after) is noted, nothing depends on it. (where: OPPO) Partly checked 2026-11-01 (the OPPO): with no screen shown, the open was answered `needs_screen` and the phone posted "Tap to show your screen and open Settings"; its tap brought Android's one screen-capture question ("Start recording or casting with Pocketdesk?", Cancel and "Start now" only: ColorOS 14's question has no single-app choice, so that part has nothing to note), and after "Start now" Settings opened (`opened`); a second open (Calculator) then opened with no question and no notification. The window here was not looked at.
- The control service off: a click is answered `control_off`, the page says to turn on Pocketdesk control in Android's Accessibility settings, and nothing is asked of the phone's owner by itself. (where: OPPO) Partly checked 2026-11-01 (the OPPO, Pocketdesk control found off in Accessibility settings): an open asked of the phone was answered `control_off` at once. Found: beside it the phone also posted a "Tap to open: Settings" notification (the P2 fallback, `PhoneActions.open` for the plain, no-screen ask, for ten minutes) — by design in the code, but the item's "nothing is asked of the phone's owner by itself" is true only of dialogs, not of that notification; decide which the item means. The page's wording is desktop UI, not looked at.
- The phone locked (a secure lock) and asleep: "locked" and "unlock the phone once", then the same click works after the unlock. A swipe-only lock is dismissed by Hands-free (see section 259), not answered "locked". (where: OPPO) Partly checked 2026-11-01 (the OPPO has a swipe-only lock, no secure lock to try): asleep with the lock screen up, awake with the lock screen up, an open was answered `locked` both times (the swipe lock was not dismissed); after swiping it away the same open was `opened`. A secure lock and its "unlock the phone once" wording were not tried.
- The page by hand on Windows and on Linux: the search narrows as it is typed, "Opened from here" shows the last apps in the order opened, the phone's name picker with two phones, the layout in a narrow window, and Settings' "Show the Phone apps page" off takes the page out of the list. (where: by hand)

**259. Hands-free apps: a paused share, the screen-sharing question accepted, a lock swiped away**

- Whether ColorOS 14 on the OPPO exposes the screen-sharing question's confirm button to the control service: with "Accept screen sharing for my computers" and "Let the computer open my apps" on and no cast running, a click on an app on the Phone apps page opens the app with no tap; the phone's log (`ConsentPress`) says "pressed after N looks" and nothing of the screen's words. If the question is hidden from accessibility or worded differently, the log says "not_found" and the page says "Tap needed on the phone": that is the expected fallback, and what the OPPO does goes in the results. (where: OPPO)
- Pause and resume: show an app's screen, close the window on the computer, and the phone's notification says "Paused" with Stop and minutes left; the status bar still shows Android's own sharing indicator. Click an app again within ten minutes: the window comes back and the app opens with no question and no tap, and the encoder started again (the picture is live). A different computer's open (or a second pairing) gets a new question, and the old pause is gone. Stop ends it; after ten minutes it ends by itself; locking the phone ends it (Android stops the projection), and the next open then asks as before. (where: OPPO)
- The desktop pages by hand: "Tap needed on the phone" appears in the accent colour with the message, and "locked_secure" shows the Extend Unlock sentence, on Windows and on Linux. (where: by hand)

**261. Phone apps on the computer's Start menu, from notifications, and dropped files shared into the app in front**

- Windows: right-click an app, Pin to Start: "<App> (phone)" is in Start's All apps with the app's own icon (not Pocketdesk's), and its right-click offers Pin to taskbar. Click it with Pocketdesk running: the app opens on the phone and the phone's screen window comes up, and the Pocketdesk main window does not move; click it with Pocketdesk closed: Pocketdesk starts, then the same. Unpin on the page removes the shortcut and the icon file. A second pin of an app with the same name keeps the first. (where: by hand)
- Linux (KDE, GNOME, Hyprland with a launcher): Pin to the applications menu: the entry "<App> (phone)" shows in the launcher (after a moment on some desktops) with the icon; running it opens the app and the screen window. An app whose name has quotes, `$`, `%`, a backslash or a path separator pins and runs with the name as given and nothing else started (check the `.desktop` file by eye). Unpin removes `pocketdesk-phone-*.desktop` and the icon. (where: by hand)
- Settings on both: "Offer to pin phone apps to Start or the applications menu" off removes the right-click menu and keeps made shortcuts working; "Share a dropped file into the phone's app in front" off sends dropped files as before. (where: by hand)
- The Files page's Kept folders section and the three Settings rows on the desktop (the folder box with Choose folder, the cap, the deletion switch) by hand on Windows and on Linux, light and dark, and in a narrow window; the Show in folder button opens the folder. (where: by hand)

**262. A spare phone at home as the wake helper (Settings, Wake helper for this home)**

- The switch: with "Wake helper for this home" off (the default) nothing listens (`adb shell dumpsys activity services dev.pocketdesk.android` lists no `WakeHelperService`, and the phone's iroh endpoint is not bound). Turn it on: a quiet notification "Waiting for a charger and Wi-Fi", or "Waking your computers for your other phones" when plugged in on Wi-Fi. (where: OPPO)
- Charger and Wi-Fi gate: unplug the helper: within a few seconds the notification says it is waiting and the log says "not listening" (the endpoint is closed: a wake asked now says the helper could not be reached); plug it in: it listens again. Turn Wi-Fi off with the charger on: same. On the battery for 30 minutes with the switch on, the app uses no measurable battery (no timer, no wake lock). (where: OPPO)
- A refused helper stops listening at once: remove it on the desktop while it is serving: within a minute the notification goes and the endpoint is closed (the log says "not listening"), and it sends no offer again. (where: OPPO)
- The words in Spanish and Tamil (Settings, the Wake settings dialog, the notification), and the desktop's Settings, Wake row, light and dark, in a narrow window, by hand on Windows and on Linux. (where: by hand)

**263. Phone apps in windows of their own, several at once (Settings, Separate windows (Shizuku))**

- Switch on, "Let the computer open my apps" on, phone unlocked: click an app on the Phone apps page: a window with the app's name opens on the computer, the app's picture in it, and the phone's own screen stays as it was (try with it off, and with another app in front); the log (`WindowCast`, `WindowHost`) names the display. Try Android 14 first, then a phone on 10 to 13 if one is at hand. (where: OPPO)
- The Phone apps page's line about how apps open, and Settings, "Open phone apps in windows of their own", by hand on Windows and on Linux, light and dark, and in a narrow window; the window's title, its Back-only bar and its control hint; the viewer windows in a Wayland session. (where: by hand)

**264. Wear OS tile, complication and list (Settings, Watch controls; `android/wear`)**

- Ring: the tile's "Ring phone" makes the phone sound the alarm for 30 s with Stop on the phone; the notification says "Your watch is ringing this phone"; a second press within a minute says "Too soon, wait a moment". (where: by hand)

**265. Keys to taps: a phone game played from the keyboard and mouse (Settings, Keymaps for phone apps)**

- A swipe: the Swipe tool, drag from a start to an end, a key: pressing it draws a swipe from the start to the end over the set time (a list scrolls by the right amount) and lifts at the end; pressing again meanwhile does nothing; the time slider changes its speed. (where: OPPO)
- The keymap file: one per app in the state folder under `keymaps/`, named `app.<package>.json` (a package such as `con.example` is `app.con.example.json`, a file from before the prefix is renamed as it is read); a second window of the same app opens with it; another app has its own; a file edited into something unsound (a key twice, Escape, a radius out of range, a name with a slash) or past 64 KiB is ignored and the editor starts empty; removing every binding removes the file. No keymap is in any log or sent to the phone. (where: by hand)
- The Settings row and the bar by hand on Windows and on Linux, light and dark, in a narrow window; the editor's list beside the picture, the markers' labels and the warning; the pointer lock on Windows (confined and recentred) and on a Wayland session (locked). The Settings page's snapshot (`settings_*.png`) shows only the top of the page: confirm by eye the Phones section row, as the snapshot test cannot run without a GPU here. (where: by hand)
