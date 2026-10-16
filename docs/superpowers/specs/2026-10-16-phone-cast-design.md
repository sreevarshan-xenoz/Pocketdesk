# The phone's screen on the computer (T10k)

Research item 10 (`docs/research/2026-10-15-what-makes-it-worth-using.md`): the reverse of Pocketdesk so far. The phone shows its own screen in a window of the desktop app, and later is driven from it. No adb, no USB debugging: scrcpy needs both, and Phone Link's app mirroring is limited to a few brands. In code the feature is **cast** ("phone screen" is T10j's `virtual_screen`, and "mirror" is the live-sessions mirror).

## What it does

- **Phone:** each saved computer gets "Show screen on computer". Android asks once per cast ("Start recording or casting?"); the phone then sends its screen, about 6 Mbit/s of H.264, the long side at most 1920. Stopping it from the notification or the status bar ends it.
- **Computer:** the desktop app opens a window at the phone's shape showing it, and closes it when the cast ends. The Sessions tab lists the cast. A new per-phone permission, **Phone screen**, gates it (on by default, like every permission but Exec); the owner's approval and the pairing gate apply as for any session.
- **Phase 2 (control):** clicks, drags, the wheel and typing in that window drive the phone through an Accessibility service the owner turns on, with a per-computer "Let this computer control this phone" switch (off by default).

## How (phase 1, view-only)

- **Wire:** a session mode `SESSION_MODE_CAST = 2` on the v1 stream connection (T10i's marker). The phone sends `[0xFA, 0x01] + w u16 + h u16` (config, on every encoder start) and each access unit on a uni stream `[0xFA, 0x00] + PacketHeader + Annex-B`, keyframes carrying SPS/PPS. The server's monitor reads the first two bytes of each client stream and hands `0xFA` streams to a cast reader (keyframes are larger than its 64 KB input limit), with a 2 MB cap. The server starts no capture, encode, video, sound or arbiter, answers `STREAM_KIND_READY`, and refuses without the permission (`phone screen not allowed`). A phone that gets video back closes the session (the computer is too old).
- **Server → app (`server/src/cast_relay.rs`, `core::local::cast`):** while a cast is admitted the server listens on `127.0.0.1:0` and writes `cast.json` (owner-only: session, phone, port, a 32-byte token, size) in the state folder; the app connects, sends the token (checked in constant time within 2 s), and gets framed messages `[type u8][len u32 LE][payload]`: config, frame, end, heartbeat. One viewer at a time. The file goes when the cast ends.
- **App (`desktop/src/cast_view.rs`):** an `ffmpeg` sidecar decodes (`-fflags nobuffer -flags low_delay -probesize 32 -f h264 -i pipe:0 -f rawvideo -pix_fmt rgba pipe:1`, no console window on Windows), an access unit delimiter after each frame so it is emitted at once; frames go to an egui texture in a second native window (`show_viewport_deferred`), letterboxed, resized on a new config.
- **Phone (`cast/`):** `CastService` (foreground service of type `mediaProjection`), the consent intent, `MediaProjection.Callback` before the virtual display, `CastEncoder` (MediaCodec AVC from a Surface, low latency, no B-frames, a keyframe every 5 s and on request, repeat-previous-frame for still screens), rotation through `resize` and a new config. The bridge (`connect_cast`, `nativeCastConfig`, `nativeCastFrame`, `nativeCastState`) queues frames and sends them, newest first, on the session.

## Not in phase 1

Control (phase 2), live drag, starting a cast from the computer, the phone's sound, HEVC, and YUV rendering.

## Checks

- **Rust:** the mode marker; the `0xFA` routing; the relay framing and token; a loopback cast session (`server/tests/cast_session.rs`: ready, a config, a keyframe over 64 KB and deltas reach the relay in order; refused without the permission).
- **Kotlin:** `CastSession` states, the encoder's size choice.
- **Device (A71 → Windows):** consent, the window showing the phone and following rotation, stop from the status bar closing it, refusal with the permission off, latency against a stopwatch, ffmpeg's CPU.
