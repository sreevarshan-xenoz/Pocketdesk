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
- **Phone (`cast/`):** `CastService` (foreground service of type `mediaProjection`), the consent intent, `MediaProjection.Callback` before the virtual display, `CastEncoder` (MediaCodec AVC from a Surface, low latency, no B-frames, a keyframe every 5 s and on request, repeat-previous-frame for still screens), rotation through `resize` and a new config. The bridge (`connect_cast`, `nativeCastConfig`, `nativeCastFrame`, `nativeCastState`) queues frames and sends them in order on the session, with at most a round trip of frames undelivered (as the computer's own video); a full queue drops the frame and the phone sends a keyframe.

## Not in phase 1

Control (phase 2), live drag, starting a cast from the computer, the phone's sound, HEVC, and YUV rendering.

## Checks

- **Rust:** the mode marker; the `0xFA` routing; the relay framing and token; a loopback cast session (`server/tests/cast_session.rs`: ready, a config, a keyframe over 64 KB and deltas reach the relay in order; refused without the permission).
- **Kotlin:** `CastSession` states, the encoder's size choice.
- **Device (A71 → Windows):** consent, the window showing the phone and following rotation, stop from the status bar closing it, refusal with the permission off, latency against a stopwatch, ffmpeg's CPU.

## As built (phase 1)

- **Wire:** a frame stream is `[0xFA, 0x00] + keyframe u8 + seq u64 LE + Annex-B` (`core::streaming::cast`), not a `PacketHeader`: the server needs only those two fields. `seq` counts from 0 at each encoder start, which sends a config first. The computer's keyframe ask is a header-only stream of kind `3` (`STREAM_KIND_CAST_KEYFRAME`). A cast is input only to the server: no capture, encode or sound, its input packets not injected, and it is refused without the Cast permission (`phone screen not allowed`) or a relay (`phone screen not shown here`).
- **Gaps:** frames finish out of order on their own streams. The server's `CastChain` asks for a keyframe when a delta frame follows a gap, or when the relay could not take a frame, at most every 250 ms; a late frame does not move the chain back.
- **Relay:** one per server process (LAN and WAN), started with the service. `cast.json` holds the session, the phone, the port and the token, without a size (the config message carries it). A new viewer gets the last config and a keyframe ask; the newest cast replaces an older one; a cast whose session ended without its `End` is noticed by its closed keyframe channel within a heartbeat. The viewer may send only `WantKeyframe`.
- **App:** the ffmpeg flags were measured with ffmpeg 8. `-fflags nobuffer` is left out (it drops the packets read while probing, the first keyframe among them), and the rawvideo encoder holds a frame unless it runs on one thread (`-threads 1`); `-vsync passthrough` (accepted by 4.x to 8) and an access unit delimiter after each frame remove the other two frames of delay. `-max_error_rate 1` keeps a broken stretch from ending the decoder. A viewer skips deltas until a keyframe and asks for one once. Closing the window stops showing that cast; the Sessions tab's "Show screen" brings it back.
- **Phone:** the bridge copies each access unit from MediaCodec's direct buffer and queues it (6 deep), and sends from the queue only while fewer than a round trip of frames (8 to 64, `core::streaming::in_flight`) are undelivered; a frame that finds the queue full is dropped and the encoder sends a keyframe. Configs wait up to 2 s for room. The encoder asks for Baseline and falls back to the encoder's default profile; keyframes carry the SPS and PPS the codec gave at its start.
- **One session at a time:** the phone holds one session, so opening the computer's screen or the remote while casting ends the cast, quietly. The cast watches only its own session (`nativeCastState` is `none` for any other) and stops only a cast (`nativeStopCast`), never the session that replaced it.
- **Sessions:** the mirror says `cast` (not `control_only`); the Sessions tab, `status`, its `--json` and the new-session notification say the phone is showing its screen.
- **Kotlin:** `CastPlan` (the encoded size, when to send a keyframe) is JVM-tested; a cast's states are the remote's (`RemoteSession`).
- **Device run (A71 → Windows, LAN):** checklist §77.
