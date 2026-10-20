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
- **Which phone:** one whose report is in `phone_media.json` (connected now, paired, Media allowed: T8d). A playing phone comes before a paused one, and the newest report before an older one.
- **A press** (the flyout's buttons, or a media key Windows routes to it) goes through `phone_media_out/`, as the Sessions page's does, held to the phone's Media permission.
- **It goes away** when no phone reports (its session ended, Media turned off, the server stopped), and when the setting is turned off. It is never left showing a song after the phone stopped.

## Left out

- **Linux** (an MPRIS player on the session bus): not built here, since nothing here can run it. The row is not offered there.
- **The window's own tray** (when the tray program is not running): as H3's key, it needs the tray program.
- **Album art:** T8d's report carries none.
- **A phone outside a session:** T8d reports only during a session, so the player shows then.

## Risks

- **The keyboard's media keys:** Windows sends them to the session it chose as current, which may become the phone's while it plays, not the owner's own player. Off by default, and gone once nothing plays.
- **The name Windows shows:** an unpackaged program is shown by its executable. The tray program takes Pocketdesk's registered AppUserModelID, as the server's toasts do, so the flyout says Pocketdesk.

## Testing

- **Unit:** which phone is shown (playing before paused, newest first, none when no report); a button's command; the setting's row.
- **Live (`#[ignore]`d, Windows):** a player shown with a probe report, read back through `GlobalSystemMediaTransportControlsSessionManager` (T8c's reader): its title, artist and state; then gone.
- **By hand (checklist §154):** the flyout with a phone playing in a session; Play/Pause, Next and Previous from the flyout and the keyboard's keys (input on this box: with the owner).
