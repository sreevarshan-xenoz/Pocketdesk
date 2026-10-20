# The phone's music in the computer's own media controls (K7)

**Date:** 2026-10-20. **Branch:** `feat/media-controls-tray`. The K cycle's seventh item (`docs/research/2026-10-20-k-cycle.md`).

## What is wrong

- **A phone's music shows only on the Sessions page.** T8d's report and its Previous, Play/Pause and Next live in the desktop app's window. The computer's own media controls (the volume flyout on Windows, the keyboard's media keys) know nothing of it.
- **Asked for:** KDE Connect shows the phone's player in Plasma's media controls. Phone Link keeps its player in its own window.

## What changes

- **Desktop Settings "The phone's music in this computer's media controls"** (`phone_media_controls`, off by default, Windows).
- **With it on, the tray program shows a phone's player in Windows' media controls.** It uses `SystemMediaTransportControls` on a hidden window of its own (`ISystemMediaTransportControlsInterop::GetForWindow`). The player shows:
  - the title and the artist;
  - playing or paused;
  - Previous, Play/Pause and Next, as the phone's player offers them.
- **Which phone:** one in a session whose report is in `phone_media.json` (connected now, paired, Media allowed: T8d) and said again within 150 s (the phone repeats it each minute). A playing phone comes before a paused one, then the newest; the phone shown stays while it plays; a paused one stays five minutes.
- **A press** (the flyout's buttons, or a media key Windows routes to it) goes through `phone_media_out/`, as the Sessions page's does, held to the phone's Media permission.
- **It goes away** when no phone qualifies (its session ended, its report not said again, paused five minutes, Media turned off, the server stopped), and when the setting is turned off.

## Left out

- **Linux** (an MPRIS player on the session bus): not built here, since nothing here can run it. The row is not offered there.
- **The window's own tray** (when the tray program is not running): as H3's key, it needs the tray program.
- **Album art:** T8d's report carries none.
- **A phone outside a session:** T8d reports only during a session, with Pocketdesk in front on the phone, so the player shows then.

## Risks

- **The keyboard's media keys:** Windows sends them to the session it chose as current, which may become the phone's while it plays, not the owner's own player. Off by default, and gone five minutes after the phone pauses.
- **The name Windows shows:** an unpackaged program is shown by its executable. The tray program takes Pocketdesk's registered AppUserModelID, as the server's toasts do, so the flyout says Pocketdesk.

## Testing

- **Unit:** which phone is shown (playing before paused, newest first, none when no report); a button's command; the setting's row.
- **Live (`#[ignore]`d, Windows):** a player shown with a probe report, read back through `GlobalSystemMediaTransportControlsSessionManager` (T8c's reader): its title, artist and state; then gone.
- **By hand (checklist §154; K3 is §153):** the flyout with a phone playing in a session; Play/Pause, Next and Previous from the flyout and the keyboard's keys (input on this box: with the owner).

## As built

- `desktop/src/phone_player.rs`: `Choice` (which phone, tested), `command` (a button's command, tested); on Windows `prepare` (WinRT and the AppUserModelID, once, before any window), `Player` (the hidden window, the media controls, a press through `model::press_phone_media` that sets the state it leaves at once) and `Watch` (the setting, read when the config changes; a failed player tried again a minute later).
- The tray program calls `prepare` first and `Watch::follow` each refresh with the phones in a session.
- The live test (`a_phone_s_song_shows_in_the_media_controls`, ignored) passed on this Windows box: the song read back through `GlobalSystemMediaTransportControlsSessionManager` from `Pocketdesk.Server`, playing; a toggle from that reader reaching `phone_media_out/` and shown paused at once; the session gone after.

## Review

A fresh review (0 Critical, 3 Important, 7 Minor), fixed one commit each:

- **Important:** a song outliving its session (a background link kept the report), a report frozen once the phone's app left the front, and a paused phone keeping the media keys for good.
- **Minor, fixed:** two playing phones swapping, a failure never retried, a press's state shown seconds late, the AppUserModelID set after the first window and at each player, WinRT readied at each player, a song with no title, the Settings note and the spec.
- **Left:** the name Windows shows comes from the server's registration of its AppUserModelID, made at its first toast; before that the flyout may show the bare id.
