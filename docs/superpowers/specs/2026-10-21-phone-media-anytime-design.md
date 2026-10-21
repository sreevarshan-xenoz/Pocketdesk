# N2: the phone's music on the computer without a session, with its cover

Plan: `docs/research/2026-10-21-n-cycle.md`, N2.

## Why

- **T8d's report comes only while the app is open:** `PhoneMediaEffect` checks every 2 s while the app is started, so the computer shows the phone's song only during a session.
- **K7 had to ignore reports outside a session** (its review: a frozen "Playing" whose presses went nowhere), so Windows' media controls show a phone's player only in a session.
- **Phone Link's player works whenever the phone is linked** (Microsoft's support page). KDE asked for the same in bug 400030.
- **No cover is sent,** so the media controls show a blank square.

## What

### The phone

- **The switch:** Settings "My media on the computer in the background", off, under "My media on the computer". It needs that switch and notification access.
- **The listener reports.** With the switch on, the notification listener (`media/BackgroundMedia.kt`) watches the media sessions:
  - `MediaSessionManager.addOnActiveSessionsChangedListener`, plus a `MediaController.Callback` on each session for its state and metadata;
  - a change is reported half a second later, so a burst is one report;
  - there is no polling.
- **One report, as now.** It is T8d's `kdeconnect.pocketdesk.phone_media` with `background: true`, sent only on the own computer's live link (`sendPhonePacketOnLink`, never dialed for it).
- **Players that report often.** Some players set their state every second. A callback compares what it would report (which carries no position) with the last report sent, and does nothing when they match. The cover is scaled and encoded on the listener's worker, only when the metadata changes.
- **When the report is said again:**
  - every minute while something plays, or for five minutes after it pauses (`MediaRule.RESEND_MS`, K7's `PAUSED_SECS`);
  - when the link comes back up.
- **The link** is kept up for this only while a report is current: playing, or paused under five minutes. It is not kept all day.
- **Presses** from the computer are taken on the listener's 2 s answers tick and act on the reported session.
- **One reporter at a time.** The app's own `PhoneMediaEffect` steps aside while the listener's reporter is actually registered (`BackgroundMedia.active`, a flag in the process), not merely while the switch is on. A listener that ColorOS froze, or that is not bound, leaves the session's reporter at work.
- **The switch is kept in step on the keeper's tick,** as M4's `TypeRunner.sync` is. The callbacks are unregistered when the switch goes off and in `onDestroy`.
- **The registration points:**
  - a listener feature (`HealthCheck.listenerFeatures`, so M1 keeps the app unfrozen for it);
  - `BackgroundRunning`;
  - L7's switch report (`media_background`) and M2a's feature table;
  - the answers tick's link condition.
- **The cover:**
  - `METADATA_KEY_ART`, else `ALBUM_ART`, else `DISPLAY_ICON`;
  - scaled to at most 256 px a side, a JPEG at quality 80, as `art` (base64);
  - its `artId` is the first 16 hex digits of the JPEG's SHA-256, carried by every report while that cover shows;
  - the bytes go once per `artId` and per link. A player that delivers its cover after the title sends a new `artId` when it does, and two albums of one title differ;
  - a track without one sends no `artId`.
  - The in-app reporter sends the cover too.
- **`MediaRule`** (JVM-tested) gains the rules: when to say it again (`due`), whether the link is wanted, and when the cover goes.

### The computer

- **`PhoneMedia`** gains `background` (serde default false) and `art_id` (optional, 16 characters `[0-9a-f]`, else none). The cover's bytes are read by the plugin from the packet and never enter `PhoneMedia` (written to `phone_media.json` every 2 s).
- **The server keeps the cover** in `<state>/phone_media_art/`, owner-only (`core::local::phone_media::keep_art`):
  - it must be a JPEG by its header, at most 256 KiB, declaring at most 512 px a side (`notification_icons::jpeg_size`);
  - it is named by the phone and its content, with the phone's older cover removed;
  - at most 64 are kept.
- **Which cover a report shows.** `PhoneMediaState.art` names the file. A report whose `artId` matches the kept one keeps it. One with another id and no bytes, or with no id, shows none. `{}` forgets it.
- **When a phone's player shows** (`phone_player::shown`, tested): with a fresh report, and either in a session (as now), or linked (`live_sessions.json`'s `linked`) with a background report.
- **Where it shows:**
  - **K7's media controls** show it with its cover (`SetThumbnail` from the file).
  - **The Overview's phone block** shows "Playing: …" with Previous, Play/Pause and Next.
  - **The tray** shows a line under the phone with Play/Pause.
  - **The Sessions page** is as now.
- **Settings on the computer:** K7's row, unchanged.

## Not in it

- A seek bar or position.
- Several players at once: the most recent one playing, as T8d picks.
- Linux's MPRIS: K7 left it out, and N10 takes it.

## Safety and privacy

- **Off by default,** since titles and covers leave the phone in the background.
- **Only the phone's own computer** gets reports.
- **Covers are checked by their header** before anything decodes them, owner-only, bounded in size and count.
- SECURITY.md limit 46 says what is sent and kept.

## Tests

- **Core:** reading `background` and `artId`; `keep_art` refusing a non-JPEG, an oversized one and too many, and replacing the phone's older cover.
- **Server:** the reports keep or drop the cover by its id.
- **Desktop:** `shown` for in a session, linked with a background report, linked without one, stale. The Overview's line and its harness test.
- **Phone:** `MediaRule`'s new rules on the JVM.
- **Owed on a device:** a real player in the background with the screen off, the cover in Windows' media controls, presses, and an hour's battery by N7's script.
