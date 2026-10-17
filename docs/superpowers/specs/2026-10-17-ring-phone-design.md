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
