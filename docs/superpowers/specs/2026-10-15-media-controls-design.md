# The computer's media from the phone (T8c)

Date: 2026-10-15. Status: design, on autopilot (the user asked for the roadmap to keep going; the decisions are mine).

## Why

T8 in the roadmap names "media controls both ways". The phone can already set the computer's volume, mute and output (the session's Sound sheet), but it cannot pause the video or skip the song playing there. That is the remote control people reach for most in KDE Connect. This part goes phone → computer. The other way, the computer controlling the phone's media, needs the phone's notification access and a desktop UI, so it is its own item.

## What

- **The Sound sheet gets "Now playing on the computer".** It shows the title, the artist and the app, with Previous, Play/Pause and Next. Buttons the player cannot use are disabled. The card reads the state when the sheet opens, every 2 s while it is open, and right after a button press. With nothing playing it says so.
- **The computer** answers with the media session the system names as current:
  - **Windows:** the Global System Media Transport Controls (`GlobalSystemMediaTransportControlsSessionManager`), which cover browsers, Spotify and the Media Player.
  - **Linux:** MPRIS through `playerctl`, the same way the Sound sheet already drives `wpctl`/`pactl`. Without `playerctl` the card says it is not available.

## Wire

Two new actions on the existing `kdeconnect.pocketdesk.audio` request, which is under the **Audio** permission. A phone with Audio off gets neither the titles nor the buttons.
- `{action: "media"}` → `{ok: true, playing: {title, artist, app, isPlaying, canPlayPause, canNext, canPrevious} | null}`.
- `{action: "mediaControl", command: "playPause" | "next" | "previous"}` → `{ok}` or `{ok: false, error}`. Any other command is refused.
- Titles and artists are cut to 200 characters and stripped of control characters, as notification text is.
- There is no change to the bridge: the phone already sends any audio request (`RustCore.audioControl`).

KDE Connect's `kdeconnect.mpris` was not used. It needs a player list and its own action names, the only client is this app, and the Audio permission already gates the audio packet.

## Settings

The per-phone **Audio** permission in the desktop's Devices tab turns it off. The phone side runs only when the sheet is opened, so it has no switch of its own.

## Tests

- server: the request's parsing (actions, commands, refusals) and the answer's cleaning (control characters, length), with the platform faked; the Windows reading against the real API is an `#[ignore]`d live test.
- Kotlin: the card's text from an answer (nothing playing, disabled buttons, a refusal), JVM-tested.
- Device: something playing on this computer (a browser video) shown on the A71, then paused and skipped from it.
