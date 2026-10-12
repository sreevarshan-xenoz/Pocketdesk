# Phone audio playback — design

Status: decided 2026-10-12, branch `feat/phone-audio` (cut from `feat/pocketdesk`). Roadmap 2791, with the minimum of 2792 and 2795 that a player needs.

## Why

The desktop already captures its own sound, Opus-encodes it and sends it to the phone on every session:
- Linux: PipeWire loopback;
- Windows: WASAPI loopback.

The phone throws it away. The bridge queues the packets, `RustCore.receiveAudio` exists, and nothing calls it. `linux-link capabilities` says so: `desktop -> phone ... played: no`. A remote desktop with no sound is the gap the user named first in the backlog.

Success means: during a session, what the desktop plays comes out of the phone, with latency small enough that a video does not feel detached (target ≤ 150 ms queued on the phone), and a toggle to turn it off.

## What exists (read, not assumed)

| Piece | State |
|---|---|
| Server | `streamer.rs::run_audio_pipeline`: 48 kHz stereo, 64 kbit/s, 20 ms Opus frames. One QUIC uni stream per packet: an 18-byte `PacketHeader` (`stream_kind = AUDIO`, `sequence`) and then the raw Opus packet. |
| Core client | `client.rs` reads uni streams one at a time, in accept order, and offers audio to an 8-slot channel (`try_deliver_audio`: skip when full, never block video). |
| Bridge | `api::receive_audio` drains that channel into `[count][len][data]…`; `nativeReceiveAudio` exports it. |
| App | `RustCore.receiveAudio()` parses it. **No caller.** `MicCapture.kt` is the reverse path, a MediaCodec Opus *encoder* (the `opus` crate does not cross-compile under cargo-ndk). |

A defect this work must fix first: `receive_audio` and `receive_frames` both lock `STREAMING_HANDLE` and then **wait for a packet while holding it**. With an audio poller running, every video receive can wait up to the audio timeout. Today nobody polls audio, so nobody has seen this.

## Decisions (made autonomously; each is cheap to revisit)

| # | Decision | Why |
|---|---|---|
| A1 | Decode with **MediaCodec `audio/opus`** and play with **`AudioTrack`** (PCM 16-bit, 48 kHz stereo, `PERFORMANCE_MODE_LOW_LATENCY`, `USAGE_MEDIA`). | The AOSP software Opus *decoder* ships on every API level the app supports (minSdk 26), unlike the encoder (API 28+). The `opus` crate cannot be cross-compiled here (see `MicCapture.kt`), and AAudio/Oboe would add a C++ toolchain for a latency gain AudioTrack's fast path mostly matches. |
| A2 | The bridge keeps the audio receiver **and the video receiver** in their own locks (`AUDIO_RX`, `VIDEO_RX`), apart from `STREAMING_HANDLE`. The session install sets them, and the teardown clears them. | This stops the audio poller from stalling video (see above). It also fixes a live defect found while planning: `receive_frames` waits up to 50 ms on a still screen while holding the handle that every input send locks first, so a tap on a still desktop could wait out the whole poll. The video half is its own commit. |
| A3 | `receiveAudio` returns `(sequence, data)` pairs: `[count u32]` then `[seq u64][len u32][data]` per packet, big-endian. Both ends ship together, with no compatibility shim (project rule). | The player needs the sequence number to drop packets that arrive late or twice. |
| A4 | One **playout thread** per session. It polls `receiveAudio(20 ms)`, decodes synchronously, and writes to the track with `WRITE_BLOCKING`. | This mirrors the `h264-decode` thread. A blocking write gives the track's buffer natural back-pressure, and the core's 8-slot channel drops the excess. |
| A5 | Latency policy: start playing once **60 ms** is queued. When more than **150 ms** is queued, skip decoded frames until it is back to 60 ms. Drop any packet whose sequence is not above the last one played. Losses get no concealment (silence via starvation). | This is a fixed, bounded budget. Sizing the buffer from measured jitter (2792), a playout clock and drift (2793), A/V sync (2794) and PLC/FEC (2795) are their own roadmap items. The core reads streams in order, so reordering is rare and handled by dropping. |
| A6 | Codec setup: `csd-0` is an `OpusHead` (version 1, 2 channels, pre-skip 312, 48 kHz, mapping family 0); `csd-1` is the pre-skip and `csd-2` the seek pre-roll (80 ms), each a 64-bit nanosecond count in little-endian (native) order, as Android's Opus decoder requires. | The wire carries raw Opus packets with no container, so the phone builds the header the encoder would have written. The 312-sample pre-skip is libopus' 48 kHz look-ahead; getting it wrong only trims or keeps 6.5 ms once at session start. |
| A7 | Audio focus: request `AUDIOFOCUS_GAIN`, accepting a delayed gain. A transient loss pauses and drops incoming packets, and playback refills the 60 ms on regain. A "can duck" loss lowers the volume to 30 % instead. Revised after the final review: focus is requested only when the desktop first sends sound, and a permanent loss is not taken back (switching off and on asks again). | A phone call or another media app must win, as users expect. Requesting at connect paused the phone's music even with a silent desktop, and re-requesting after a permanent loss would override the user's choice. |
| A8 | A **"Desktop sound on this phone"** switch in the Quick Settings sheet (Streaming group), persisted in `Prefs`, **on by default**. It acts live: off stops the player and frees the decoder and track at once. The server keeps sending (64 kbit/s), and the phone skips the packets. | Remote desktops play sound by default. Telling the server to stop would be a wire change for 8 KB/s. |
| A9 | Playback follows the **video stream's lifetime**: it starts when `DecoderHost` connects and stops in `DecoderHost.stop()`. A small `DesktopAudio` controller plays exactly when `enabled && streamUp`. | It keeps playing in PiP (the surface stays alive) and stops when the session does. One rule means no second lifecycle to keep in sync. |
| A10 | The capability row becomes `desktop -> phone ... played: yes`, since the receiving end now plays it. Its unit test and `docs/capabilities.md` change with it. | The table is the project's source of truth for exactly this claim (roadmap 2054). |

## Out of scope (YAGNI)

- Jitter sizing from measurements (2792).
- A playout clock and drift correction (2793).
- A/V sync (2794).
- FEC/PLC (2795).
- Output-device switching, which Android routes automatically (2796).
- Per-app capture and a source picker (2797, 2798).
- Rate or channel negotiation (2799).
- Bitrate profiles (2800).
- Muting the desktop's own speakers while the phone plays (the existing audio control sheet already mutes).
- Telling the server to stop sending.

## Architecture

```
android/bridge
  session.rs   AUDIO_RX: its own TokioMutex<Option<Receiver<AudioPacket>>>
  api.rs       install_streaming stores audio_rx there; stop clears it;
               receive_audio waits on AUDIO_RX only; payload gains seq
  lib.rs       nativeReceiveAudio encodes [count][seq][len][data]…

android/app  stream/
  OpusHead.kt          csd-0/1/2 for the decoder (pure)
  PlayoutPolicy.kt     late/duplicate drop, prebuffer and cap hysteresis (pure)
  DesktopAudioPlayer.kt  thread: receiveAudio → MediaCodec → AudioTrack, focus
  DesktopAudio.kt      controller: enabled && streamUp → player running
  RemoteDesktopView.kt DecoderHost reports streamUp/streamDown
ui/QuickSettingsSheet.kt + Prefs + strings (en/es/ta): the switch

core/src/capabilities.rs + docs/capabilities.md: played = yes
```

### Data flow

1. QUIC uni stream → core client → 8-slot channel.
2. On the playout thread, `receiveAudio(20)` returns `(seq, opus)` pairs.
3. `PlayoutPolicy.accept(seq)` drops late or duplicate packets. The decoder input is the Opus packet, with the presentation time = seq × 20 ms.
4. The decoder output is 3840 bytes of PCM per frame. `PlayoutPolicy.shouldWrite(queuedMs)` decides; a written frame goes into `AudioTrack.write(BLOCKING)`.
5. `queuedMs` = frames written − `playbackHeadPosition`, in ms. `play()` starts once the prebuffer is reached.

### Error handling

| Situation | Behavior |
|---|---|
| No `audio/opus` decoder, or the track fails to build | Log once and leave the player off for this session; video is unaffected. No UI message: the AOSP decoder exists on every supported Android version, so this is a log-only fault. |
| The decoder throws mid-session | Release the decoder and track, and rebuild once. If it fails again, stop for the session and log. |
| The stream goes down | `DecoderHost.stop()` → `streamDown()` → the thread joins (2 s max) and everything is released. |
| Focus lost | Pause and flush; packets are dropped until focus is regained (A7). |

## Testing

| Layer | What is tested |
|---|---|
| Bridge | Unit test: `receive_audio` returns sequences in order and in the new payload layout. A test that `receive_frames` returns a queued frame while a `receive_audio` call is waiting on an empty channel (RED today: they share a lock). |
| Kotlin JVM | `OpusHead` bytes against the RFC 7845 layout. `PlayoutPolicy`: late/duplicate drop, the prebuffer gate, and the 150 → 60 ms hysteresis. |
| Core | The capabilities unit test flips to `played == true` for `desktop -> phone`, and `capabilities_doc` holds the doc to it. |
| Gates | Windows gates, bridge clippy and tests, `assembleDebug` + `lintDebug` (no new finding in changed files) + `testDebugUnitTest`. |
| Device (owed if no phone is attached) | Play a video on the desktop: the phone sounds within ~150 ms, and the switch silences it at once. A phone call interrupts and playback resumes after. Sound continues in PiP and stops on Exit. Video fps and e2e are unchanged with audio on (the lock fix). |
