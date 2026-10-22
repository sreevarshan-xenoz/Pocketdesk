# N5: the computer's music in the phone's media controls

Plan: `docs/research/2026-10-21-n-cycle.md`, N5.

## Why

- **The computer's music reaches the phone only when asked.** The Sound sheet (T8c), the media widget (G5) and the tile (G6) each ask while open or tapped. Nothing shows it where a phone shows music: the lock screen, the shade's media controls, a headset's buttons, a watch.
- **Requests elsewhere:** KDE bug 337485, and KDE Discuss's "No album art" from Windows 11 (2025-01-11).

## What

### The computer

- **The subscription:** a phone with its switch on asks to be told (`kdeconnect.pocketdesk.computer_media_watch {on}`), held to its Audio permission, as T8c is.
- **The push:** while any linked phone watches, the server reads what plays every 2 s (Windows' media sessions; `playerctl` elsewhere). On a change, and again each minute, it pushes `kdeconnect.pocketdesk.computer_media {playing, artId, art}` to each phone that watches. `playing` has T8c's fields or null. A phone that unlinks, or loses Audio, is told no more.
- **The cover:** on Windows, the session's thumbnail; elsewhere, `mpris:artUrl` when it is a local file. It must be a JPEG or PNG by its header, at most 256 KiB. `artId` is its SHA-256's first 16 hex digits, and the bytes go once per track and phone, as N2's do.
- **No echo:** Pocketdesk's own entry in the media controls (K7, the phone's music) is never taken for the computer's. The current session is the system's when it is not Pocketdesk's own, else the first other one playing, else the first other one. T8c's answers and presses use the same choice.

### The phone

- **The switch:** Settings "Computer's music in my media controls", off by default.
- **The session:** with the switch on and the own computer linked, the notification listener (`media/ComputerMedia.kt`) asks to be told, and shows what the computer plays as a media session of its own:
  - its title, artist and app;
  - its cover;
  - a media notification (`MediaStyle`, a quiet channel) with Previous, Play/Pause and Next as the player offers them.

  Android shows these on the lock screen, in the shade's media controls and on a watch, and a headset's buttons reach the session while it is the one playing.
- **The presses** go through T8c's `mediaControl`, on the live link.
- **When it ends:**
  - nothing playing for ten seconds, or paused for five minutes;
  - no push for 150 s (the link dropped);
  - the switch off.

  Any of these ends the session and its notification. The switch off also stops the watch.
- **No echo:** N2's reporters never report the phone's own session.

## Not in it

- A seek bar, and volume from the phone's keys (K4 has them in Remote).
- A computer player's own presses beyond Previous, Play/Pause and Next.

## Safety

- **SECURITY.md limit 49:** with the phone's switch on, the computer's titles, artists, app and cover go to the phone while a phone watches, and show on its lock screen; a press there plays, pauses or skips the computer's music.
- **Headset buttons:** they go to the computer's session only while it is the one playing on the phone.

## Tests

- **Server:**
  - the session's choice (own entry skipped);
  - the push's body and when it is due;
  - the watchers kept and dropped.
- **Phone:** `ComputerMediaRule` (what to show, when to end) on the JVM.
- **On the OPPO, by the runner:**
  - a computer media report made through a seam (`POCKETDESK_MEDIA_FILE`, so this box's own players are untouched) reaching the phone's media session (`dumpsys media_session`);
  - a press from the session reaching the seam.

## As built

- **As designed, with the review's fixes.**
  - Pocketdesk's own entry in the media controls goes by one name, core's `MEDIA_CONTROLS_APP_ID`, which the tray (K7), the toasts and the server's skip all read.
  - A phone that asks again is told afresh, the cover's bytes included; and the phone asks again when no push came for 75 s or a cover it does not hold is named, at most every 30 s (`ComputerMediaRule.Watch`), since a link that dropped and came back between two looks seems the same.
  - A cover not there when a track starts is looked for at the next ten looks: a browser sets its artwork a moment after its title.
  - Nothing playing ends the session after ten seconds, not at once: Windows lists no player for a moment between two tracks.
  - Play and Pause send the computer's one toggle only when they change what shows.
- **The run on the OPPO** (2026-10-21): `n5-media` PASS (shown in the media session, not echoed back as the phone's music, Next reached the computer, gone when it stopped), and `n2-media` PASS beside it.
- **The runner's check** (`n5-media`) sends a media key with `cmd media_session dispatch next`, which reaches whichever session holds the media keys; on a phone where another player holds them it fails and says so.

## Review

A fresh review found 0 Critical, 3 Important and 9 Minor. All were fixed except the runner's cover check (owed by eye, checklist §179).

- **Important:**
  - a cover missing at a track's start was never read again;
  - a cover the phone could not decode was asked for, with its bytes, every 30 s;
  - a draw queued behind a slow press could leave the session and its notification after close.
- **Minor:**
  - Pause while paused started the computer's music;
  - a cover's pixels were not bounded;
  - a Linux cover read could hang on a FIFO;
  - the seam was not announced nor held to an absolute path;
  - a newer push dropped the cover's one-time bytes;
  - a gap between tracks tore the session down;
  - the echo guard's name unheld by anything;
  - SECURITY.md's limit did not say where the cover comes from;
  - README and the status.
