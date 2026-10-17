# Ring my phone from the computer, any time (N1)

Date: 2026-10-17. Plan: `docs/research/2026-10-17-finish-and-polish.md` §N1.

## The problem

- **The computer cannot ring its phones.** Pocketdesk rings KDE Connect phones (3c), not its own: the server never sends `kdeconnect.findmydevice` to a phone. The phone already takes the packet (the bridge's control reader sets its siren latch), but only a session's `SirenWatcher` looks at the latch, so a ring could only reach a phone in a session, and nothing sends one.
- **Where people lose a phone, the session is not open.** KDE's find-my-phone fails when the phone is locked (bug 479290) or in Do Not Disturb (bug 402384).

## The design

1. **A ring request:** the desktop app's Ring on each phone in Devices, and `pocketdesk ring <phone>`, queue a request in `<state>/rings_out/<id>.json` (`core::local::rings`, the same shape as `links_out`: one file each, taken and deleted by the server, dropped after 60 s). The CLI refuses at once a phone that is not linked (`live_sessions.json`), as `pocketdesk link` does, and says the phone needs "Ring from the computer" on.
2. **The server rings:** a watcher takes each request and sends `kdeconnect.findmydevice {ring: true}` over the phone's live connection (a session's or the background link), under a new per-phone permission, **Ring** ("Ring this phone", on by default), at most once a minute per phone. The packet is now one the computer sends too (`outgoingCapabilities`, `docs/packets.md`).
3. **The phone keeps a link for it:** a phone Setting, "Ring from the computer" (off by default; it asks for notification access, as the other background features do), keeps the background link from the notification listener, as "Files from the computer in the background" does, and checks the siren latch every 2 s.
4. **The phone rings loudly, in the background and locked:** a notification on its own channel, "Ring from the computer", with the alarm sound and `USAGE_ALARM` (heard in Do Not Disturb when alarms are allowed, which is Android's default), insistent (it repeats until stopped), a full-screen intent onto the lock screen, and a Stop action; it goes after 30 s. The alarm stream is raised to its maximum while it rings and put back after. No foreground service is needed: a notification's sound plays from the background.
5. **In a session,** the existing siren dialog still shows; the ring is the same notification, so it is heard with the screen off too.

## Security

Only a paired computer's connection carries the ring, and only with the phone's Ring permission on the computer and its own switch on the phone. One a minute per phone stops a computer, or a script with the owner's account, from making a phone ring without end. A ring tells the phone nothing else.

## Tests

- core: `rings` request round trip, stale and future requests dropped.
- server: a queued ring goes to its phone when Ring is on, not when off, not to another phone, and not twice in a minute.
- phone (JVM): the ring rule (switch, latch, 30 s, the once-a-minute guard on the phone side as well).

## Device check

The A71 with "Ring from the computer" on, against a throwaway server: rings from the background and locked, Stop from the lock screen, goes by itself; nothing with the switch off or Ring off. Kept to a second or two each.

## As built

- **The permission:** `Capability::Ring` (`ring` in `devices --json`), on by default for phones and off in `DevicePermissions::computer()`; its switch in Devices, "Ring this phone from here". `pair::allows` checks a capability that gates what the computer sends.
- **The queue and the server:** `core::local::rings` (`rings_out/`, 60 s), `server/src/ring_phone.rs` (`run_watcher`, `Pace`: a phone at most once a minute, counted only when a ring goes), `FIND` in `plugins::SERVER_PUSHES`, so `docs/packets.md` lists `kdeconnect.findmydevice` both ways. `pocketdesk ring <phone> [--json]` (`local_api::ring_in`) refuses at once a phone with Ring off or no link (`live_sessions.json`'s sessions and `linked`).
- **The desktop app:** a Ring button per phone in Devices, on while it is linked (`Device::linked`) with Ring on and not a computer; its tip says why it is off. The notice names the phone.
- **The phone:** "Ring from the computer" (`Prefs.ringFromComputer`, off by default) asks for notification access; `ring/BackgroundRing` on the listener's own 2 s ticker keeps the link and applies `RingRule` (JVM-tested): outside a session the latch is always taken, rung only with the switch on, once a minute. `ring/PhoneRinger` posts the ring on channel `ring_from_computer` (the alarm sound, `USAGE_ALARM`, bypass DnD where allowed, public on the lock screen), insistent, with a full-screen intent, Stop (and dismissing) through `StopRingReceiver`, `setTimeoutAfter(30 s)`; the alarm stream is raised to its maximum and put back after. The notification names the computer. `USE_FULL_SCREEN_INTENT` is in the manifest.
- **Found on the device:** eight Settings switches (and the new one) changed their setting without redrawing Settings, so they did not move until Settings was opened again; every setter now redraws.
- **Checked on the A71** against a throwaway server (checklist §91): with the switch on and the app in the background, screen off, `pocketdesk ring` rang it on the alarm stream within seconds; it went by itself after 30 s; rung again a minute later, Stop from the shade silenced it at 8 s, and the alarm volume was back as before; with the switch off, the computer sent the ring and the phone dropped it. Notification access was granted for the check and revoked after.

