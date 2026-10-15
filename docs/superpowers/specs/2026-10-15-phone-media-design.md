# The phone's media from the computer (T8d)

Date: 2026-10-15. Status: built (see As built), on autopilot (the user asked for the roadmap to keep going; the decisions are mine).

## Why

T8 in the roadmap names "media controls both ways"; T8c did phone → computer. This part is the other way: music or a podcast playing on the phone, paused or skipped from the desk without reaching for the phone. KDE Connect does both ways.

## What

- **The phone reports what it plays** during a session: the title, the artist, the app, whether it plays, and which of Previous, Play/Pause and Next work. It sends a report at the start, on any change (checked every 2 s), and nothing while nothing plays.
  - It reads Android's active media sessions (`MediaSessionManager.getActiveSessions`). Android allows that only to an app with notification access, the access "This phone's notifications" already asks for.
  - It is on only when the new phone setting **"My media on the computer"** is on (off by default) and that access is granted. Turning the setting on without the access opens Android's access page, as the notifications switch does.
- **The computer shows it** in the desktop app's Sessions tab, under each phone in a session: "Playing on the phone: Song · Artist (Spotify)", with Previous, Play/Pause and Next.
  - A press is queued for the server (`<state>/phone_media_out/`, one file per press, taken within 10 s), as Send link is.
  - The server pushes it to that phone, which passes it to that media session's transport controls.
- **The local API:** `pocketdesk devices --json` carries `media` per phone (its last report, or null). There is no command for the buttons yet, because the desktop app is the use.

## Wire

- Phone → computer: `kdeconnect.pocketdesk.phone_media {title, artist, app, isPlaying, canPlayPause, canNext, canPrevious}`, or `{}` when nothing plays any more.
  - The server takes it only from a paired phone, as the battery is taken, and keeps it in memory and in `<state>/phone_media.json` (written at most every 2 s, a phone no longer paired dropped, as the battery's file is).
  - Text is cleaned as T8c's is: one line, no invisible format characters, at most 200 characters.
- Computer → phone: `kdeconnect.pocketdesk.phone_media_control {command: "playPause" | "next" | "previous"}`. The phone acts only on these three, only while its setting is on, and only on the session it reported.
- **Permission:** a new per-phone **Media** permission (Devices tab, on by default). With it off, the server ignores the phone's reports and sends it no presses. The phone's own setting is the other switch.

## Settings

- Phone: "My media on the computer" (off; needs notification access).
- Desktop: the per-phone Media permission. The Sessions tab shows the card only for a phone that reported.

## Tests

- core: the report's parsing and cleaning; the press queue (stale, future, mislabelled and bad presses dropped).
- server: a report kept only from a paired phone with Media on; a press pushed only to its phone, only with Media on.
- desktop: the card's text.
- Kotlin: which session is reported (the playing one, else the most recent), when a report is sent (a change; `{}` once when playing stops), and that a press acts only with the setting on.
- Device: something playing on the A71, shown in the Sessions tab, then paused and skipped from it.

## As built

- **Where it lives.** `core::protocol::phone_media` (packets, the report, `clean_line`, now shared with T8c), `core::local::phone_media` (the file and the press queue), the server's `PhoneMediaPlugin` and watcher, the Sessions tab, the bridge's `media_presses`, and the phone's `MediaRule` (JVM-tested) and `PhoneMediaEffect`.
- **Changed from the design.**
  - The phone reports while Pocketdesk is in front (like the battery and the control link it rides), not with the app in the background: music on a phone in a pocket is not covered yet.
  - The computer shows a phone's media only while it is connected, paired and allowed Media, so no stale song outlives a session, a turned-off permission or the phone's switch.
  - `phone_media.json` is written owner-only: it is listening history.
- **Fresh review (0 Critical, 3 Important, minors), fixed one commit each:**
  - stale reports (a session ended, Media or the phone's setting turned off) were still shown with live buttons;
  - a phone still playing was never reported again after a server restart or reconnect;
  - a press waiting on the phone had no age, so one sent while the app was in the background acted minutes later.
  - Minors fixed: presses only to a phone still paired, the file owner-only, the desktop's notice in plain words, one session list per check.
  - Ruled: the app's name may be the package name on Android 11+ (package visibility); orphaned temp files and one phone in two sessions showing two rows are left.
