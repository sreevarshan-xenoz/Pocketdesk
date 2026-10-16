# The phone as a remote, without the screen (T10i)

Research item 12 (`docs/research/2026-10-15-what-makes-it-worth-using.md`): KDE Connect's remote input and presenter, and the "Unified Remote" kind of app. Often the computer's screen is in front of you (a TV, a projector, the desk across the room) and the phone is wanted only as a trackpad, a keyboard, media keys or a presenter clicker. Streaming the screen then costs the phone's battery and data and the computer's GPU for nothing.

## What it does

- **Phone:** each saved computer gets "Remote", opening a screen with no video. It holds the trackpad (the session's dock: move, tap, two-finger scroll), the keyboard with its bar, a row of media and presenter keys (Play/Pause, Previous, Next, Volume, Page Up/Down, arrows, F5, Esc, B), and the shortcut bar.
- **Computer:** an input-only session captures, encodes and sends nothing, and plays no sound. It goes through the same gate as a session (paired, its key, the owner's approval), needs the Input permission (refused without it), and follows the driving rules (one phone drives at a time). The Sessions tab and `pocketdesk status` show it as "control only", and its notification says the phone "is controlling" the computer.

## How

- **Wire:** a new client config stream, `SESSION_MODE_MARKER = [0xFB, 0x00] + mode u8` (1 = input only), read by `read_client_config` into `ClientClaim.input_only`. An input-only client sends device id, its WAN key (LAN) and the mode right after connecting, ahead of its first link report, so the mode is among the loop's four (a fourth stream, that first report, is then read as an unknown marker and dropped).
- **Ready:** once the gate admits the session, the server opens one uni stream with a `PacketHeader` of a new kind, `STREAM_KIND_READY`. The phone waits for it before it shows the controls, so taps made while the owner decides are not queued and injected later.
- **Server (`run_pipeline`):** capture, encode, video transport, audio and the bitrate arbiter are not started; the monitor (input, view-only, `LinkFeedback`, the 30 s silence rule) and the telemetry poll are. Packets about the picture (`MouseMoveAbs`, `WindowCrop`, `RequestKeyframe`, quality) are dropped. The session record does not name the mode yet.
- **Old servers** ignore the marker and stream video: the client treats any video stream on an input-only session as "server too old" and closes, and the phone says so.
- **Bridge:** `connect_streaming(.., input_only)` and its WAN twin; the mode is part of what "already connected" compares; `remote_state` (`none`, `waiting`, `ready`, `tooOld`).
- **Phone:** `RemoteInputScreen` and a small controller (connect, WAN fallback, ready, closed reason), with the session's foreground service and keep-screen-on.

## Not done

- A laser pointer, the phone's volume keys driving the computer's volume, Mute (not in the key contract yet).

## Checks

- **Rust:** the mode marker over loopback; an input-only pipeline over loopback (the ready stream comes, a relative move arrives, an absolute tap never does, no video or audio); how the client takes a ready stream and a picture (`remote_stream`); the status line's `control only`.
- **Kotlin:** the screen's key rows map to the contract.
- **Device (A71 → Windows, Input off for safety, then a scratch window):** a remote session with no video in the server log and Task Manager's GPU idle; ready after the owner's approval; the Sessions tab's "control only"; keys and a trackpad move with Input on, into a scratch Notepad only.

## As built

- **The bridge** reports the remote's state as `{"state": none | waiting | ready | tooOld}` (`nativeRemoteState`); `RemoteSession` (JVM-tested) turns it, and the closed reason, into the screen's states.
- **Found on the A71, fixed:** Retry after a refusal reused the refused session (a closed session no longer counts as connected); a session whose pipeline ended was never closed, so a remote waited forever (the server now closes it; the phone also gives up after 45 s).
- **Not done:** the approval prompt still says the phone wants to stream; the remote has no Mute (not in the key contract).
- **Fresh review (0 Critical, 2 Important), fixed one commit each:** "too old" was hidden by the close that follows it, and leaving the screen during a failing LAN dial could start the internet fallback after the stop (an orphan session). Minors fixed: a remote reports its link every 5 s, the test proves the absolute tap is dropped, and this spec. Left: the owner is asked to approve before a phone without Input is refused; a mode stream later than 200 ms would read as a picture session (the phone would then say "too old"); the session's foreground service still takes the microphone type when the mic is allowed; a stop on leaving can race a session opened at once for the same computer.
