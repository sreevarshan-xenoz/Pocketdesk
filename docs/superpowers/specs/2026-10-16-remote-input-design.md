# The phone as a remote, without the screen (T10i)

Research item 12 (`docs/research/2026-10-15-what-makes-it-worth-using.md`): KDE Connect's remote input and presenter, and the "Unified Remote" kind of app. Often the computer's screen is in front of you (a TV, a projector, the desk across the room) and the phone is wanted only as a trackpad, a keyboard, media keys or a presenter clicker. Streaming the screen then costs the phone's battery and data and the computer's GPU for nothing.

## What it does

- **Phone:** each saved computer gets "Remote", opening a screen with no video. It holds the trackpad (the session's dock: move, tap, two-finger scroll), the keyboard with its bar, a row of media and presenter keys (Play/Pause, Previous, Next, Volume, Page Up/Down, arrows, F5, Esc, B), and the shortcut bar.
- **Computer:** an input-only session captures, encodes and sends nothing, and plays no sound. It goes through the same gate as a session (paired, its key, the owner's approval), needs the Input permission (refused without it), and follows the driving rules (one phone drives at a time). The Sessions tab and `pocketdesk status` show it as "control only", and its notification says the phone "is controlling" the computer.

## How

- **Wire:** a new client config stream, `SESSION_MODE_MARKER = [0xFB, 0x00] + mode u8` (1 = input only), read by `read_client_config` into `ClientClaim.input_only`. An input-only client sends device id, its WAN key (LAN) and the mode: three streams, within the loop's four.
- **Ready:** once the gate admits the session, the server opens one uni stream with a `PacketHeader` of a new kind, `STREAM_KIND_READY`. The phone waits for it before it shows the controls, so taps made while the owner decides are not queued and injected later.
- **Server (`run_pipeline`):** capture, encode, video transport, audio and the bitrate arbiter are not started; the monitor (input, view-only, `LinkFeedback`, the 30 s silence rule) and the telemetry poll are. Packets about the picture (`MouseMoveAbs`, `WindowCrop`, `RequestKeyframe`, quality) are dropped. The session record says `mode=input`.
- **Old servers** ignore the marker and stream video: the client treats any video stream on an input-only session as "server too old" and closes, and the phone says so.
- **Bridge:** `connect_streaming(.., input_only)` and its WAN twin; the mode is part of what "already connected" compares; no video or audio receivers; `isSessionReady`, `legacyServer`.
- **Phone:** `RemoteInputScreen` and a small controller (connect, WAN fallback, ready, closed reason), with the session's foreground service and keep-screen-on.

## Not done

- A laser pointer, the phone's volume keys driving the computer's volume, Mute (not in the key contract yet).

## Checks

- **Rust:** the mode marker over loopback; an input-only pipeline over loopback (input arrives, no video or audio stream, the ready stream does, closing the client ends it); a video stream on an input-only client marks it legacy; the mirror's `video_off`.
- **Kotlin:** the screen's key rows map to the contract.
- **Device (A71 → Windows, Input off for safety, then a scratch window):** a remote session with no video in the server log and Task Manager's GPU idle; ready after the owner's approval; the Sessions tab's "control only"; keys and a trackpad move with Input on, into a scratch Notepad only.
