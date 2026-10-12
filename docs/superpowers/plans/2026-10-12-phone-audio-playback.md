# Phone audio playback Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the phone play the desktop's sound during a session, with a live on/off switch, without slowing video or input.

**Architecture:** The bridge moves the video and audio receivers out of the session lock, so a waiting receive no longer blocks input or the other receive. Audio packets reach Kotlin with their sequence numbers. A playout thread decodes Opus with MediaCodec and plays it through a low-latency AudioTrack, under a fixed 60/150 ms latency budget and audio focus. Playback follows the video stream's lifetime and a persisted switch.

**Tech Stack:** Rust (tokio, JNI bridge), Kotlin + Jetpack Compose, Android MediaCodec `audio/opus`, AudioTrack, AudioFocusRequest, JUnit 4.

**Spec:** `docs/superpowers/specs/2026-10-12-phone-audio-playback-design.md`

## Global Constraints

- Branch `feat/phone-audio`. One logical change per commit, conventional-commit messages, files staged by name, `git diff --cached --name-only` checked before each commit.
- Every commit message ends with `Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>`.
- Wire format: 48 kHz, 2 channels, 20 ms Opus frames, 64 kbit/s (set by the server; the phone does not change it).
- Latency budget: start at **60 ms** queued; past **150 ms** queued, skip decoded frames until it is back to **60 ms**.
- `csd-0` = OpusHead v1, 2 channels, pre-skip **312**, 48000 Hz, gain 0, mapping family 0. `csd-1` = pre-skip in ns (**6 500 000**); `csd-2` = **80 000 000** ns. Both are 64-bit little-endian.
- Audio payload from the bridge, big-endian: `u32 count`, then per packet `u64 sequence`, `u32 len`, `len` bytes. Both ends ship in the same commit (no compatibility shim).
- minSdk 26: only API ≤ 26 Android calls without a version check. `createDecoderByType` (not `ByString`, which API 37's jar removed).
- The switch is **on by default**, persisted in `Prefs` (`desktop_sound`), and acts live.
- Strings ship in en, es and ta together.
- Gates (from the repo root unless noted):
  - Rust: `cargo fmt --all -- --check` (check its exit status);
  - `cargo clippy --workspace --all-targets --exclude linux-link-android-bridge --exclude iroh-spike -- -D warnings`;
  - `cargo test --workspace --exclude linux-link-android-bridge --exclude iroh-spike`;
  - in `android/bridge`: `cargo clippy --all-targets -- -D warnings` and `cargo test`.
  - Android (in `android/`): `./gradlew --offline assembleDebug lintDebug testDebugUnitTest`. Lint stays at 21 findings (19 warnings + 2 hints) with none in a changed file. Run it in the background; it can take several minutes.
  - The APK needs the bridge `.so`: `cd android/bridge && cargo ndk -t arm64-v8a -o ../app/src/main/jniLibs build` after any bridge change (the output is gitignored).
- Never run power, lock or server-stop actions on this box.

## Review Focus

1. **The desktop goes silent, then plays again.** WASAPI loopback sends nothing while nothing plays, so the phone starves. Expected: playback pauses and refills 60 ms before resuming, with no crackle at a burst. Pinned in Task 3 (`aStarvedTrackPausesToRefill`).
2. **The server's sequence restarts at 0.** This happens with a new audio task on the same player. Expected: packets are accepted again, not dropped as late forever. Pinned in Task 3 (`aSequenceFarBehindIsARestart`).
3. **The track's 32-bit head position wraps** (after ~24.8 h at 48 kHz). Expected: queued time stays correct across the wrap. Pinned in Task 3 (`queuedTimeIsRightAcrossTheHeadWrap`).
4. **A truncated or hostile audio payload** (a count larger than the bytes, a negative length). Expected: the parser returns the whole packets it could read and never throws. Pinned in Task 2 (`aTruncatedPayloadYieldsTheWholePacketsOnly`).
5. **Input sent while video is still** (no frames for seconds). Expected: an input send takes the session lock at once, not after the frame poll's 50 ms. Pinned in Task 1 (`a_waiting_frame_receive_does_not_hold_the_session_lock`).

---

### Task 1: Video receive stops holding the session lock

**Files:**
- Modify: `android/bridge/src/session.rs` (`StreamingHandle`, new `VIDEO_RX`)
- Modify: `android/bridge/src/api.rs` (`install_streaming`, `teardown_streaming_handle`, `receive_frames`, tests module, imports at line ~22)

**Interfaces:**
- Produces: `pub(crate) static VIDEO_RX: LazyLock<TokioMutex<Option<tokio::sync::mpsc::Receiver<linux_link_core::streaming::EncodedPacket>>>>` in `session.rs`. Also a test-only `SESSION_STATICS: LazyLock<tokio::sync::Mutex<()>>` in `api.rs`'s test module, which Task 2's tests reuse.
- `receive_frames(timeout_ms: u64) -> Vec<FrameDto>` keeps its signature.

- [ ] **Step 1: Write the failing test** (in `api.rs`'s `#[cfg(test)] mod tests`)

```rust
    /// Tests that swap the session statics run one at a time.
    static SESSION_STATICS: std::sync::LazyLock<tokio::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

    fn frame(sequence: u64) -> linux_link_core::streaming::EncodedPacket {
        linux_link_core::streaming::EncodedPacket {
            data: vec![0, 0, 0, 1],
            is_keyframe: true,
            timestamp: std::time::Instant::now(),
            sequence,
        }
    }

    #[tokio::test]
    async fn a_waiting_frame_receive_does_not_hold_the_session_lock() {
        let _serial = SESSION_STATICS.lock().await;
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        *crate::VIDEO_RX.lock().await = Some(rx);
        let waiting = tokio::spawn(receive_frames(500));
        tokio::time::sleep(Duration::from_millis(30)).await;

        // What every input send does first: take the session handle.
        let locked =
            tokio::time::timeout(Duration::from_millis(100), (*STREAMING_HANDLE).lock()).await;
        assert!(locked.is_ok(), "an input send waited on the frame poll");
        drop(locked);

        tx.send(frame(7)).await.unwrap();
        let frames = waiting.await.unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].sequence, 7);
        *crate::VIDEO_RX.lock().await = None;
    }
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cd android/bridge && cargo test a_waiting_frame_receive`
Expected: compile error, `cannot find value VIDEO_RX in the crate root`.

- [ ] **Step 3: Implement**

In `session.rs`, next to `STREAMING_HANDLE`:

```rust
/// The live session's video receiver, kept apart from [`STREAMING_HANDLE`]:
/// `receive_frames` waits on it for up to its timeout, and holding the
/// session handle that long held up every input send behind it.
pub(crate) static VIDEO_RX: LazyLock<
    TokioMutex<Option<tokio::sync::mpsc::Receiver<linux_link_core::streaming::EncodedPacket>>>,
> = LazyLock::new(|| TokioMutex::new(None));
```

Then delete the `packet_rx` field (and its `#[allow(dead_code)]` and doc line) from `StreamingHandle`. Make sure `VIDEO_RX` is reachable as `crate::VIDEO_RX`: follow the re-export pattern used for `STREAMING_HANDLE`. Check `lib.rs` for `pub(crate) use session::…` and add `VIDEO_RX` there, and to the `use crate::{…}` list at the top of `api.rs`.

In `api.rs`, `install_streaming`: before `let mut handle = (*STREAMING_HANDLE).lock().await;` add

```rust
    *VIDEO_RX.lock().await = Some(packet_rx);
```

and remove `packet_rx,` from the `StreamingHandle { … }` literal.

In `teardown_streaming_handle`, first line:

```rust
    VIDEO_RX.lock().await.take();
```

In `receive_frames`, replace

```rust
        let mut guard = (*STREAMING_HANDLE).lock().await;
        let Some(handle) = guard.as_mut() else {
            return frames;
        };
        match tokio::time::timeout_at(deadline, handle.packet_rx.recv()).await {
```

with

```rust
        let mut guard = VIDEO_RX.lock().await;
        let Some(rx) = guard.as_mut() else {
            return frames;
        };
        match tokio::time::timeout_at(deadline, rx.recv()).await {
```

and the inner `handle.packet_rx.try_recv()` with `rx.try_recv()`.

- [ ] **Step 4: Run the test and the bridge gates**

Run: `cd android/bridge && cargo test && cargo clippy --all-targets -- -D warnings && cd ../.. && cargo fmt --all -- --check`
Expected: all pass, including `a_waiting_frame_receive_does_not_hold_the_session_lock`.

- [ ] **Step 5: Commit**

```bash
git add android/bridge/src/session.rs android/bridge/src/api.rs android/bridge/src/lib.rs
git commit -m "fix(android): a still screen no longer delays input by 50 ms

receive_frames waited for a frame (50 ms when the screen is still)
while holding STREAMING_HANDLE, which every input send locks first, so a
tap on a still desktop could wait out the whole poll. The video
receiver now lives in its own lock (VIDEO_RX), set by install_streaming
and cleared by the teardown.

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

(Stage `lib.rs` only if Step 3 touched it.)

---

### Task 2: Audio reaches Kotlin with its sequence, without blocking anything

**Files:**
- Modify: `android/bridge/src/session.rs` (`StreamingHandle.audio_rx` → new `AUDIO_RX`)
- Modify: `android/bridge/src/api.rs` (`install_streaming`, `teardown_streaming_handle`, `receive_audio`, new `audio_payload`, tests)
- Modify: `android/bridge/src/lib.rs` (`nativeReceiveAudio` uses `api::audio_payload`)
- Create: `android/app/src/main/java/dev/linuxlink/android/bridge/AudioPayload.kt`
- Modify: `android/app/src/main/java/dev/linuxlink/android/bridge/RustCore.kt:374-389` (`receiveAudio`)
- Test: `android/app/src/test/java/dev/linuxlink/android/bridge/AudioPayloadTest.kt`

**Interfaces:**
- Consumes: `SESSION_STATICS`, `frame()`, `VIDEO_RX` from Task 1.
- Produces:
  - Rust `pub async fn receive_audio(timeout_ms: u64) -> Vec<(u64, Vec<u8>)>`.
  - Rust `pub fn audio_payload(packets: &[(u64, Vec<u8>)]) -> Vec<u8>`.
  - Kotlin `class AudioChunk(val sequence: Long, val data: ByteArray)`, `object AudioPayload { fun parse(bytes: ByteArray): List<AudioChunk> }`.
  - `RustCore.receiveAudio(timeoutMs: Int = 20): List<AudioChunk>`.

- [ ] **Step 1: Write the failing Rust tests** (`api.rs` tests module)

```rust
    /// The bytes AudioPayloadTest.kt parses; both tests pin this string.
    const AUDIO_FIXTURE_HEX: &str =
        "00000002000000000000000700000003010203000000000000000800000000";

    #[test]
    fn the_audio_payload_carries_each_sequence() {
        let bytes = audio_payload(&[(7, vec![1, 2, 3]), (8, vec![])]);
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, AUDIO_FIXTURE_HEX);
    }

    #[tokio::test]
    async fn a_waiting_audio_receive_blocks_neither_video_nor_input() {
        let _serial = SESSION_STATICS.lock().await;
        let (audio_tx, audio_rx) = tokio::sync::mpsc::channel(4);
        let (video_tx, video_rx) = tokio::sync::mpsc::channel(4);
        *crate::AUDIO_RX.lock().await = Some(audio_rx);
        *crate::VIDEO_RX.lock().await = Some(video_rx);
        let waiting = tokio::spawn(receive_audio(500));
        tokio::time::sleep(Duration::from_millis(30)).await;

        video_tx.send(frame(1)).await.unwrap();
        let frames = tokio::time::timeout(Duration::from_millis(200), receive_frames(50))
            .await
            .expect("video waited on the audio poll");
        assert_eq!(frames.len(), 1);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), (*STREAMING_HANDLE).lock())
                .await
                .is_ok(),
            "input waited on the audio poll"
        );

        audio_tx
            .send(linux_link_core::streaming::AudioPacket {
                data: vec![9],
                sequence: 41,
                timestamp: std::time::Instant::now(),
                is_config: false,
            })
            .await
            .unwrap();
        assert_eq!(waiting.await.unwrap(), vec![(41, vec![9])]);
        *crate::AUDIO_RX.lock().await = None;
        *crate::VIDEO_RX.lock().await = None;
    }
```

- [ ] **Step 2: Run them and watch them fail**

Run: `cd android/bridge && cargo test audio`
Expected: compile errors, `cannot find function audio_payload` and `cannot find value AUDIO_RX`.

- [ ] **Step 3: Implement the bridge half**

In `session.rs`, next to `VIDEO_RX`:

```rust
/// The live session's audio receiver, apart from [`STREAMING_HANDLE`] and
/// [`VIDEO_RX`] for the same reason: `receive_audio` waits on it.
pub(crate) static AUDIO_RX: LazyLock<
    TokioMutex<Option<tokio::sync::mpsc::Receiver<linux_link_core::streaming::AudioPacket>>>,
> = LazyLock::new(|| TokioMutex::new(None));
```

Then delete `audio_rx` (field, attribute and doc line) from `StreamingHandle`, and export `AUDIO_RX` the same way as `VIDEO_RX`.

In `api.rs`: `install_streaming` adds `*AUDIO_RX.lock().await = Some(audio_rx);` next to the `VIDEO_RX` line and drops `audio_rx,` from the handle literal. `teardown_streaming_handle` adds `AUDIO_RX.lock().await.take();`. Replace `receive_audio` and its doc comment with:

```rust
/// Receive queued audio packets from the streaming client: each one raw
/// Opus (20 ms @ 48 kHz stereo) with the sequence the desktop gave it, so
/// the player can drop one that arrives late. Up to
/// `MAX_AUDIO_PACKETS_PER_RECEIVE`; waits on `AUDIO_RX` only.
pub async fn receive_audio(timeout_ms: u64) -> Vec<(u64, Vec<u8>)> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    let mut packets = Vec::with_capacity(MAX_AUDIO_PACKETS_PER_RECEIVE);
    let mut guard = AUDIO_RX.lock().await;
    let Some(rx) = guard.as_mut() else {
        return packets;
    };
    if let Ok(Some(packet)) = tokio::time::timeout_at(deadline, rx.recv()).await {
        packets.push((packet.sequence, packet.data));
        while packets.len() < MAX_AUDIO_PACKETS_PER_RECEIVE {
            match rx.try_recv() {
                Ok(packet) => packets.push((packet.sequence, packet.data)),
                Err(_) => break,
            }
        }
    }
    packets
}

/// `receive_audio`'s packets as the JNI payload, big-endian: `u32 count`,
/// then per packet `u64 sequence`, `u32 len`, `len` bytes.
pub fn audio_payload(packets: &[(u64, Vec<u8>)]) -> Vec<u8> {
    let total: usize = packets.iter().map(|(_, data)| 12 + data.len()).sum();
    let mut buf = Vec::with_capacity(4 + total);
    buf.extend_from_slice(&(packets.len() as u32).to_be_bytes());
    for (sequence, data) in packets {
        buf.extend_from_slice(&sequence.to_be_bytes());
        buf.extend_from_slice(&(data.len() as u32).to_be_bytes());
        buf.extend_from_slice(data);
    }
    buf
}
```

In `lib.rs`, `nativeReceiveAudio` becomes:

```rust
/// Drain up to 32 Opus audio packets as a binary blob; see
/// [`api::audio_payload`] for the layout.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_linuxlink_android_bridge_RustCore_nativeReceiveAudio(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
    timeout_ms: jint,
) -> jbyteArray {
    let packets = RUNTIME.block_on(api::receive_audio(timeout_ms.max(0) as u64));
    match env.byte_array_from_slice(&api::audio_payload(&packets)) {
        Ok(arr) => arr.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}
```

- [ ] **Step 4: Run the bridge tests and gates**

Run: `cd android/bridge && cargo test && cargo clippy --all-targets -- -D warnings`
Expected: PASS, including both new tests.

- [ ] **Step 5: Write the failing Kotlin test** (`AudioPayloadTest.kt`)

```kotlin
package dev.linuxlink.android.bridge

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class AudioPayloadTest {
    /** android/bridge/src/api.rs's AUDIO_FIXTURE_HEX; both tests pin it. */
    private val fixture = hex("00000002000000000000000700000003010203000000000000000800000000")

    private fun hex(s: String) = ByteArray(s.length / 2) { s.substring(it * 2, it * 2 + 2).toInt(16).toByte() }

    @Test
    fun parsesTheBridgesLayout() {
        val chunks = AudioPayload.parse(fixture)
        assertEquals(listOf(7L, 8L), chunks.map { it.sequence })
        assertArrayEquals(byteArrayOf(1, 2, 3), chunks[0].data)
        assertEquals(0, chunks[1].data.size)
    }

    @Test
    fun aTruncatedPayloadYieldsTheWholePacketsOnly() {
        assertEquals(listOf(7L), AudioPayload.parse(fixture.copyOf(fixture.size - 3)).map { it.sequence })
        assertTrue(AudioPayload.parse(ByteArray(0)).isEmpty())
        // A count far beyond the bytes, and a negative length.
        assertTrue(AudioPayload.parse(hex("7fffffff")).isEmpty())
        assertTrue(AudioPayload.parse(hex("00000001" + "0000000000000001" + "ffffffff")).isEmpty())
    }
}
```

- [ ] **Step 6: Run it and watch it fail**

Run (in `android/`): `./gradlew --offline testDebugUnitTest --tests dev.linuxlink.android.bridge.AudioPayloadTest`
Expected: compile failure, `Unresolved reference: AudioPayload`.

- [ ] **Step 7: Implement the Kotlin half**

`AudioPayload.kt`:

```kotlin
package dev.linuxlink.android.bridge

import java.nio.ByteBuffer
import java.nio.ByteOrder

/** One Opus packet from the desktop, with the sequence it was sent under. */
class AudioChunk(val sequence: Long, val data: ByteArray)

/**
 * The bridge's audio payload (`api::audio_payload` in the Rust bridge),
 * big-endian: `u32 count`, then per packet `u64 sequence`, `u32 len`,
 * `len` bytes. A short or malformed payload yields the whole packets
 * before the fault; it never throws.
 */
object AudioPayload {
    fun parse(bytes: ByteArray): List<AudioChunk> {
        val buf = ByteBuffer.wrap(bytes).order(ByteOrder.BIG_ENDIAN)
        if (buf.remaining() < 4) return emptyList()
        val count = buf.int
        val chunks = ArrayList<AudioChunk>(count.coerceIn(0, 32))
        repeat(count.coerceAtLeast(0)) {
            if (buf.remaining() < 12) return chunks
            val sequence = buf.long
            val len = buf.int
            if (len < 0 || buf.remaining() < len) return chunks
            val data = ByteArray(len)
            buf.get(data)
            chunks += AudioChunk(sequence, data)
        }
        return chunks
    }
}
```

In `RustCore.kt`, replace `receiveAudio` (the doc comment through its closing brace) with:

```kotlin
    /** Drain the desktop's Opus packets (20 ms @ 48 kHz stereo each), with sequences. */
    fun receiveAudio(timeoutMs: Int = 20): List<AudioChunk> =
        AudioPayload.parse(nativeReceiveAudio(timeoutMs))
```

- [ ] **Step 8: Run the Kotlin test, rebuild the `.so`, and run the Android gates**

Run: `cd android/bridge && cargo ndk -t arm64-v8a -o ../app/src/main/jniLibs build`, then (in `android/`, in the background) `./gradlew --offline assembleDebug lintDebug testDebugUnitTest`.
Expected: BUILD SUCCESSFUL, `AudioPayloadTest` passes, lint `0 errors, 19 warnings, 2 hints` and no finding in `AudioPayload.kt` or `RustCore.kt`. Also run `cargo fmt --all -- --check` (exit 0).

- [ ] **Step 9: Commit**

```bash
git add android/bridge/src/session.rs android/bridge/src/api.rs android/bridge/src/lib.rs \
  android/app/src/main/java/dev/linuxlink/android/bridge/AudioPayload.kt \
  android/app/src/main/java/dev/linuxlink/android/bridge/RustCore.kt \
  android/app/src/test/java/dev/linuxlink/android/bridge/AudioPayloadTest.kt
git commit -m "feat(android): audio reaches Kotlin with its sequence, in its own lock

The phone's player needs each Opus packet's sequence to drop late ones,
and polling for audio must not hold up video or input: receive_audio
waited while holding STREAMING_HANDLE. The audio receiver now lives in
AUDIO_RX, and the payload carries a u64 sequence per packet. The Rust
and Kotlin tests pin one fixture.

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: The pure parts of playback: Opus header and playout policy

**Files:**
- Create: `android/app/src/main/java/dev/linuxlink/android/stream/OpusHead.kt`
- Create: `android/app/src/main/java/dev/linuxlink/android/stream/PlayoutPolicy.kt`
- Test: `android/app/src/test/java/dev/linuxlink/android/stream/OpusHeadTest.kt`
- Test: `android/app/src/test/java/dev/linuxlink/android/stream/PlayoutPolicyTest.kt`

**Interfaces:**
- Produces:
  - `object OpusHead { SAMPLE_RATE: Int; CHANNELS: Int; PRE_SKIP: Int; SEEK_PREROLL_NS: Long; fun csd0(): ByteArray; fun csd1(): ByteArray; fun csd2(): ByteArray }`
  - `class PlayoutPolicy { fun accept(sequence: Long): Boolean; fun shouldWrite(queuedMs: Int): Boolean; fun readyToPlay(queuedMs: Int): Boolean; fun starved(playing: Boolean, queuedMs: Int): Boolean; fun rebuffer() }`
  - companion: `START_MS = 60`, `CAP_MS = 150`, `RESTART_GAP = 250L`, `fun queuedMs(framesWritten: Long, headPosition: Int): Int`

- [ ] **Step 1: Write the failing tests**

`OpusHeadTest.kt`:

```kotlin
package dev.linuxlink.android.stream

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Test

class OpusHeadTest {
    @Test
    fun csd0IsAnRfc7845IdentificationHeader() {
        val head = OpusHead.csd0()
        assertEquals(19, head.size)
        assertEquals("OpusHead", String(head, 0, 8, Charsets.US_ASCII))
        assertArrayEquals(
            // version 1, 2 channels, pre-skip 312 LE, 48000 LE, gain 0, family 0
            byteArrayOf(1, 2, 0x38, 0x01, 0x80.toByte(), 0xBB.toByte(), 0, 0, 0, 0, 0),
            head.copyOfRange(8, 19),
        )
    }

    @Test
    fun csd1AndCsd2AreLittleEndianNanoseconds() {
        assertArrayEquals(le(6_500_000L), OpusHead.csd1())
        assertArrayEquals(le(80_000_000L), OpusHead.csd2())
    }

    private fun le(v: Long) = ByteArray(8) { (v ushr (8 * it)).toByte() }
}
```

`PlayoutPolicyTest.kt`:

```kotlin
package dev.linuxlink.android.stream

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PlayoutPolicyTest {
    @Test
    fun lateAndRepeatedPacketsAreDropped() {
        val policy = PlayoutPolicy()
        assertTrue(policy.accept(0))
        assertTrue(policy.accept(1))
        assertFalse("repeat", policy.accept(1))
        assertFalse("late", policy.accept(0))
        assertTrue("a gap is fine", policy.accept(5))
    }

    @Test
    fun aSequenceFarBehindIsARestart() {
        val policy = PlayoutPolicy()
        assertTrue(policy.accept(1_000))
        assertTrue("a new audio task counts from 0", policy.accept(0))
        assertTrue(policy.accept(1))
    }

    @Test
    fun playbackStartsAtTheStartLevel() {
        val policy = PlayoutPolicy()
        assertFalse(policy.readyToPlay(40))
        assertTrue(policy.readyToPlay(60))
    }

    @Test
    fun pastTheCapFramesAreSkippedUntilBackAtTheStartLevel() {
        val policy = PlayoutPolicy()
        assertTrue(policy.shouldWrite(100))
        assertFalse("over the cap", policy.shouldWrite(151))
        assertFalse("still draining", policy.shouldWrite(100))
        assertTrue("back at the start level", policy.shouldWrite(60))
        assertTrue(policy.shouldWrite(100))
    }

    @Test
    fun aStarvedTrackPausesToRefill() {
        val policy = PlayoutPolicy()
        assertTrue(policy.starved(playing = true, queuedMs = 0))
        assertFalse(policy.starved(playing = true, queuedMs = 20))
        assertFalse("not playing yet", policy.starved(playing = false, queuedMs = 0))
    }

    @Test
    fun rebufferEndsADrain() {
        val policy = PlayoutPolicy()
        policy.shouldWrite(200)
        policy.rebuffer()
        assertTrue(policy.shouldWrite(100))
    }

    @Test
    fun queuedTimeIsRightAcrossTheHeadWrap() {
        assertEquals(60, PlayoutPolicy.queuedMs(framesWritten = 48 * 60L, headPosition = 0))
        // Written count has wrapped past 2^32; the head has not yet.
        val written = (0xFFFF_FFFFL + 1 + 48 * 10) and 0xFFFF_FFFFL
        val head = (0xFFFF_FFFFL - 48 * 50 + 1).toInt()
        assertEquals(60, PlayoutPolicy.queuedMs(written, head))
    }
}
```

- [ ] **Step 2: Run them and watch them fail**

Run (in `android/`): `./gradlew --offline testDebugUnitTest --tests 'dev.linuxlink.android.stream.*'`
Expected: compile failure, `Unresolved reference: OpusHead` / `PlayoutPolicy`.

- [ ] **Step 3: Implement**

`OpusHead.kt`:

```kotlin
package dev.linuxlink.android.stream

import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * The codec-specific data Android's Opus decoder needs: `csd-0` is the
 * RFC 7845 identification header, and `csd-1`/`csd-2` are the pre-skip and
 * seek pre-roll as little-endian nanosecond counts. The desktop sends bare
 * Opus packets with no container, so the phone writes the header its
 * encoder would have. The wire format (48 kHz stereo) is fixed by the
 * server's audio pipeline.
 */
object OpusHead {
    const val SAMPLE_RATE = 48_000
    const val CHANNELS = 2

    /** libopus' look-ahead at 48 kHz, in samples (6.5 ms). */
    const val PRE_SKIP = 312

    /** RFC 7845's recommended seek pre-roll. */
    const val SEEK_PREROLL_NS = 80_000_000L

    fun csd0(): ByteArray =
        ByteBuffer.allocate(19).order(ByteOrder.LITTLE_ENDIAN).apply {
            put("OpusHead".toByteArray(Charsets.US_ASCII))
            put(1) // version
            put(CHANNELS.toByte())
            putShort(PRE_SKIP.toShort())
            putInt(SAMPLE_RATE) // the input rate; informational
            putShort(0) // output gain
            put(0) // channel mapping family 0: mono or stereo
        }.array()

    fun csd1(): ByteArray = nanos(PRE_SKIP * 1_000_000_000L / SAMPLE_RATE)

    fun csd2(): ByteArray = nanos(SEEK_PREROLL_NS)

    private fun nanos(value: Long): ByteArray =
        ByteBuffer.allocate(8).order(ByteOrder.LITTLE_ENDIAN).putLong(value).array()
}
```

`PlayoutPolicy.kt`:

```kotlin
package dev.linuxlink.android.stream

/**
 * The player's decisions, kept apart from MediaCodec and AudioTrack so they
 * can be tested (spec A5): a fixed latency budget, with no measured jitter
 * sizing or drift handling (roadmap 2792/2793). Owned by the playout thread;
 * not thread-safe.
 */
class PlayoutPolicy {
    private var lastSequence = -1L
    private var draining = false

    /**
     * Whether a packet is newer than every one played so far. One far
     * behind is a new audio task on the desktop counting from 0 again, not
     * a late packet.
     */
    fun accept(sequence: Long): Boolean {
        val restart = lastSequence - sequence > RESTART_GAP
        if (sequence <= lastSequence && !restart) return false
        lastSequence = sequence
        return true
    }

    /** Whether to write the next decoded frame, given how much is queued. */
    fun shouldWrite(queuedMs: Int): Boolean {
        if (draining && queuedMs <= START_MS) {
            draining = false
        } else if (!draining && queuedMs > CAP_MS) {
            draining = true
        }
        return !draining
    }

    /** Whether enough is queued to start (or restart) the track. */
    fun readyToPlay(queuedMs: Int): Boolean = queuedMs >= START_MS

    /** Whether a playing track ran dry and should pause to refill. */
    fun starved(playing: Boolean, queuedMs: Int): Boolean = playing && queuedMs == 0

    /** Forget a drain in progress (after a flush, the queue is empty). */
    fun rebuffer() {
        draining = false
    }

    companion object {
        const val START_MS = 60
        const val CAP_MS = 150

        /** 5 s of 20 ms packets: further back than any reorder. */
        const val RESTART_GAP = 250L

        private const val FRAMES_PER_MS = 48
        private const val MASK = 0xFFFF_FFFFL

        /**
         * Milliseconds queued in a track, from the frames written so far and
         * its playback head. Both are 32-bit counters that wrap (after
         * ~24.8 h at 48 kHz), so the difference is taken modulo 2^32.
         */
        fun queuedMs(framesWritten: Long, headPosition: Int): Int =
            (((framesWritten - (headPosition.toLong() and MASK)) and MASK) / FRAMES_PER_MS).toInt()
    }
}
```

- [ ] **Step 4: Run the tests and the Android gates**

Run (in `android/`, in the background): `./gradlew --offline assembleDebug lintDebug testDebugUnitTest`
Expected: BUILD SUCCESSFUL, all `OpusHeadTest` and `PlayoutPolicyTest` cases pass, lint unchanged at 21 with no finding in the new files.

- [ ] **Step 5: Commit**

```bash
git add android/app/src/main/java/dev/linuxlink/android/stream/OpusHead.kt \
  android/app/src/main/java/dev/linuxlink/android/stream/PlayoutPolicy.kt \
  android/app/src/test/java/dev/linuxlink/android/stream/OpusHeadTest.kt \
  android/app/src/test/java/dev/linuxlink/android/stream/PlayoutPolicyTest.kt
git commit -m "feat(android): the Opus header and playout policy for desktop sound

The decisions the phone's player makes, kept apart from MediaCodec and
AudioTrack so a JVM test can hold them:

- OpusHead builds the csd-0/1/2 Android's decoder needs for the bare
  Opus packets the desktop sends.
- PlayoutPolicy drops late or repeated packets but treats a sequence
  far behind as a restart. It starts at 60 ms queued, skips frames past
  150 ms until back at 60, pauses a starved track to refill, and
  computes queued time across the 32-bit head wrap.

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: The player, wired to the video stream's lifetime

**Files:**
- Create: `android/app/src/main/java/dev/linuxlink/android/stream/DesktopAudioPlayer.kt`
- Create: `android/app/src/main/java/dev/linuxlink/android/stream/DesktopAudio.kt`
- Modify: `android/app/src/main/java/dev/linuxlink/android/Prefs.kt` (add `desktop_sound`)
- Modify: `android/app/src/main/java/dev/linuxlink/android/stream/RemoteDesktopView.kt` (`DecoderHost.start` gains `context`; reports up/down)

**Interfaces:**
- Consumes: `RustCore.receiveAudio(timeoutMs): List<AudioChunk>` (Task 2); `OpusHead`, `PlayoutPolicy` (Task 3).
- Produces:
  - `Prefs.desktopSound(ctx: Context): Boolean` (default `true`) and `Prefs.setDesktopSound(ctx: Context, enabled: Boolean)`.
  - `object DesktopAudio { fun streamUp(context: Context); fun streamDown(); fun setEnabled(context: Context, enabled: Boolean) }`.

There is no JVM test here: everything left is Android framework calls, and the logic they depend on is Task 3's. The verification is the build plus the device check in Task 6.

- [ ] **Step 1: Prefs**

In `Prefs.kt`, add `private const val KEY_DESKTOP_SOUND = "desktop_sound"` beside the other keys, and after `setHaptics`:

```kotlin
    /** Whether the desktop's sound plays on the phone during a session. */
    fun desktopSound(ctx: Context): Boolean =
        prefs(ctx).getBoolean(KEY_DESKTOP_SOUND, true)

    fun setDesktopSound(ctx: Context, enabled: Boolean) {
        prefs(ctx).edit { putBoolean(KEY_DESKTOP_SOUND, enabled) }
        bump()
    }
```

- [ ] **Step 2: The player** (`DesktopAudioPlayer.kt`)

```kotlin
package dev.linuxlink.android.stream

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioTrack
import android.media.MediaCodec
import android.media.MediaFormat
import android.util.Log
import dev.linuxlink.android.bridge.RustCore
import java.nio.ByteBuffer
import java.util.concurrent.atomic.AtomicInteger

/**
 * Plays the desktop's sound on the phone: one thread polls the bridge for
 * Opus packets, decodes them with MediaCodec and writes PCM to a low-latency
 * AudioTrack. [PlayoutPolicy] keeps the latency budget. Audio focus decides
 * whether anything is heard at all: a call or another media app wins. Spec:
 * docs/superpowers/specs/2026-10-12-phone-audio-playback-design.md.
 */
class DesktopAudioPlayer(context: Context) {
    private val context = context.applicationContext

    @Volatile
    private var running = false
    private var worker: Thread? = null

    fun start() {
        if (running) return
        running = true
        worker = Thread({ run() }, "desktop-audio").apply {
            isDaemon = true
            priority = Thread.MAX_PRIORITY
            start()
        }
    }

    /** Stops the thread and waits for it (at most 2 s) to release everything. */
    fun stop() {
        running = false
        worker?.let { runCatching { it.join(2000) } }
        worker = null
    }

    /** A failure rebuilds once; a second ends playback for this session. */
    private fun run() {
        repeat(2) { attempt ->
            if (!running) return
            val outcome = runCatching { playUntilStopped() }
            outcome.exceptionOrNull()?.let { Log.w(TAG, "playback failed (attempt ${attempt + 1})", it) }
            if (outcome.isSuccess) return
        }
        Log.w(TAG, "desktop sound is off for this session")
    }

    private fun playUntilStopped() {
        val focus = AtomicInteger(FOCUS_NONE)
        val attributes = AudioAttributes.Builder()
            .setUsage(AudioAttributes.USAGE_MEDIA)
            .setContentType(AudioAttributes.CONTENT_TYPE_MOVIE)
            .build()
        val audioManager = context.getSystemService(AudioManager::class.java)
        val focusRequest = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
            .setAudioAttributes(attributes)
            .setOnAudioFocusChangeListener { change ->
                focus.set(
                    when (change) {
                        AudioManager.AUDIOFOCUS_GAIN -> FOCUS_FULL
                        AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK -> FOCUS_DUCKED
                        else -> FOCUS_NONE
                    },
                )
            }
            .build()
        if (audioManager.requestAudioFocus(focusRequest) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED) {
            focus.set(FOCUS_FULL)
        }

        val codec = MediaCodec.createDecoderByType(MediaFormat.MIMETYPE_AUDIO_OPUS)
        var track: AudioTrack? = null
        try {
            codec.configure(
                MediaFormat.createAudioFormat(
                    MediaFormat.MIMETYPE_AUDIO_OPUS,
                    OpusHead.SAMPLE_RATE,
                    OpusHead.CHANNELS,
                ).apply {
                    setByteBuffer("csd-0", ByteBuffer.wrap(OpusHead.csd0()))
                    setByteBuffer("csd-1", ByteBuffer.wrap(OpusHead.csd1()))
                    setByteBuffer("csd-2", ByteBuffer.wrap(OpusHead.csd2()))
                },
                null,
                null,
                0,
            )
            codec.start()
            val minBuffer = AudioTrack.getMinBufferSize(
                OpusHead.SAMPLE_RATE,
                AudioFormat.CHANNEL_OUT_STEREO,
                AudioFormat.ENCODING_PCM_16BIT,
            )
            track = AudioTrack.Builder()
                .setAudioAttributes(attributes)
                .setAudioFormat(
                    AudioFormat.Builder()
                        .setSampleRate(OpusHead.SAMPLE_RATE)
                        .setChannelMask(AudioFormat.CHANNEL_OUT_STEREO)
                        .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                        .build(),
                )
                // Room above the cap, so a write blocks only when the
                // policy has already let the queue run long.
                .setBufferSizeInBytes(maxOf(minBuffer, BYTES_PER_MS * PlayoutPolicy.CAP_MS * 2))
                .setTransferMode(AudioTrack.MODE_STREAM)
                .setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY)
                .build()
            Log.i(TAG, "desktop sound started")
            loop(codec, track, focus)
        } finally {
            runCatching { codec.stop() }
            runCatching { codec.release() }
            track?.let { t ->
                Log.i(TAG, "desktop sound stopped (${t.underrunCount} underruns)")
                runCatching { t.pause() }
                runCatching { t.flush() }
                runCatching { t.release() }
            }
            audioManager.abandonAudioFocusRequest(focusRequest)
        }
    }

    private fun loop(codec: MediaCodec, track: AudioTrack, focus: AtomicInteger) {
        val policy = PlayoutPolicy()
        val info = MediaCodec.BufferInfo()
        var framesWritten = 0L
        var playing = false
        var heard = true
        while (running) {
            val state = focus.get()
            if (state == FOCUS_NONE) {
                if (heard) {
                    track.pause()
                    track.flush()
                    playing = false
                    heard = false
                    framesWritten = track.playbackHeadPosition.toLong() and 0xFFFF_FFFFL
                    policy.rebuffer()
                }
                RustCore.receiveAudio(POLL_MS) // drained and dropped while unheard
                continue
            }
            heard = true
            track.setVolume(if (state == FOCUS_DUCKED) DUCKED_VOLUME else 1f)

            for (chunk in RustCore.receiveAudio(POLL_MS)) {
                if (!policy.accept(chunk.sequence)) continue
                val index = codec.dequeueInputBuffer(INPUT_WAIT_US)
                if (index < 0) continue // the decoder is full: drop this packet
                val input = codec.getInputBuffer(index) ?: continue
                input.clear()
                if (chunk.data.size > input.capacity()) {
                    codec.queueInputBuffer(index, 0, 0, 0, 0)
                    continue
                }
                input.put(chunk.data)
                codec.queueInputBuffer(index, 0, chunk.data.size, chunk.sequence * FRAME_US, 0)
            }

            while (true) {
                val index = codec.dequeueOutputBuffer(info, 0)
                if (index == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) continue
                if (index < 0) break
                val output = codec.getOutputBuffer(index)
                var queued = PlayoutPolicy.queuedMs(framesWritten, track.playbackHeadPosition)
                if (policy.starved(playing, queued)) {
                    track.pause()
                    playing = false
                }
                if (output != null && info.size > 0 && policy.shouldWrite(queued)) {
                    output.position(info.offset)
                    output.limit(info.offset + info.size)
                    val written = track.write(output, info.size, AudioTrack.WRITE_BLOCKING)
                    if (written > 0) framesWritten = (framesWritten + written / BYTES_PER_FRAME) and 0xFFFF_FFFFL
                    queued = PlayoutPolicy.queuedMs(framesWritten, track.playbackHeadPosition)
                }
                codec.releaseOutputBuffer(index, false)
                if (!playing && policy.readyToPlay(queued)) {
                    track.play()
                    playing = true
                }
            }
        }
    }

    private companion object {
        const val TAG = "DesktopAudio"
        const val FOCUS_NONE = 0
        const val FOCUS_FULL = 1
        const val FOCUS_DUCKED = 2
        const val DUCKED_VOLUME = 0.3f
        const val POLL_MS = 20
        const val INPUT_WAIT_US = 10_000L
        const val FRAME_US = 20_000L
        const val BYTES_PER_FRAME = 4 // 16-bit stereo
        const val BYTES_PER_MS = 48 * BYTES_PER_FRAME
    }
}
```

- [ ] **Step 3: The controller** (`DesktopAudio.kt`)

```kotlin
package dev.linuxlink.android.stream

import android.content.Context
import dev.linuxlink.android.Prefs

/**
 * Plays the desktop's sound exactly while a video stream is up and the
 * user's switch is on (spec A8/A9). The stream reports up/down from
 * DecoderHost; the switch calls [setEnabled].
 */
object DesktopAudio {
    private var appContext: Context? = null
    private var streamUp = false
    private var player: DesktopAudioPlayer? = null

    @Synchronized
    fun streamUp(context: Context) {
        appContext = context.applicationContext
        streamUp = true
        apply()
    }

    @Synchronized
    fun streamDown() {
        streamUp = false
        apply()
    }

    @Synchronized
    fun setEnabled(context: Context, enabled: Boolean) {
        appContext = context.applicationContext
        Prefs.setDesktopSound(context, enabled)
        apply()
    }

    private fun apply() {
        val context = appContext
        val wanted = streamUp && context != null && Prefs.desktopSound(context)
        if (wanted && player == null) {
            player = DesktopAudioPlayer(context!!).also { it.start() }
        } else if (!wanted) {
            val stopping = player ?: return
            player = null
            // Off the caller's thread: the switch is on the UI thread, and
            // the player may take a write's worth of time to wind down.
            Thread({ stopping.stop() }, "desktop-audio-stop").start()
        }
    }
}
```

- [ ] **Step 4: Report the stream up and down from `DecoderHost`**

In `RemoteDesktopView.kt`, add `context: android.content.Context,` as the first parameter of `DecoderHost.start`. In its thread body, replace `if (connected) active.start()` with:

```kotlin
            if (connected) {
                active.start()
                // Not for a generation that stop() already ended.
                if (decoder === active) DesktopAudio.streamUp(context)
            }
```

In `DecoderHost.stop()`, right after `decoder = null`, add `DesktopAudio.streamDown()`. In `surfaceCreated`, pass the factory's `context` first: `decoderHost.start(context, holder.surface, address, …)`. Then check for other `decoderHost.start(` call sites with `grep -n "decoderHost.start(" android/app/src/main/java -r` and pass a `Context` there too.

- [ ] **Step 5: Build, lint and test**

Run (in `android/`, in the background): `./gradlew --offline assembleDebug lintDebug testDebugUnitTest`
Expected: BUILD SUCCESSFUL, lint still 21 with no finding in a changed file, all unit tests pass. If lint flags a new finding in a changed file (e.g. a missing API check), fix it rather than baselining it.

- [ ] **Step 6: Commit**

```bash
git add android/app/src/main/java/dev/linuxlink/android/stream/DesktopAudioPlayer.kt \
  android/app/src/main/java/dev/linuxlink/android/stream/DesktopAudio.kt \
  android/app/src/main/java/dev/linuxlink/android/Prefs.kt \
  android/app/src/main/java/dev/linuxlink/android/stream/RemoteDesktopView.kt
git commit -m "feat(android): play the desktop's sound on the phone

The desktop has sent Opus audio on every session since F1, and the
phone dropped all of it. A playout thread now polls the bridge,
decodes with MediaCodec's audio/opus decoder and writes to a
low-latency AudioTrack, under PlayoutPolicy's 60/150 ms budget. It
takes audio focus (a call or another media app wins; a duck lowers the
volume to 30 %) and rebuilds once on a failure.

DesktopAudio plays exactly while the video stream is up and the
desktop_sound pref is on (default on; the switch follows).

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: The "Desktop sound on this phone" switch

**Files:**
- Modify: `android/app/src/main/java/dev/linuxlink/android/ui/QuickSettingsSheet.kt` (two parameters, one row)
- Modify: `android/app/src/main/java/dev/linuxlink/android/ui/RemoteScreen.kt` (state + wiring at the `QuickSettingsSheet(` call, line ~1073)
- Modify: `android/app/src/main/res/values/strings.xml`, `values-es/strings.xml`, `values-ta/strings.xml`

**Interfaces:**
- Consumes: `Prefs.desktopSound`, `DesktopAudio.setEnabled` (Task 4).

- [ ] **Step 1: Strings**

Add after `row_mic` in each file:
- `values/strings.xml`: `<string name="row_desktop_sound">Desktop sound on this phone</string>`
- `values-es/strings.xml`: `<string name="row_desktop_sound">Sonido del escritorio en el teléfono</string>`
- `values-ta/strings.xml`: `<string name="row_desktop_sound">கணினியின் ஒலி இந்தக் கைபேசியில்</string>`

- [ ] **Step 2: The sheet row**

In `QuickSettingsSheet`'s parameter list, after `onCyclePane: () -> Unit,`:

```kotlin
    desktopSound: Boolean,
    onSetDesktopSound: (Boolean) -> Unit,
```

and in the Streaming group, after the `row_pane` `ValueRow`:

```kotlin
            ToggleRow(LlIcons.Volume, stringResource(R.string.row_desktop_sound), desktopSound, onSetDesktopSound)
```

- [ ] **Step 3: Wire it in `RemoteScreen`**

Next to the other session state in `RemoteScreen` (where `clipboardSync` is declared), add:

```kotlin
    var desktopSound by remember { mutableStateOf(Prefs.desktopSound(context)) }
```

At the `QuickSettingsSheet(` call, after the `onCyclePane = …` argument:

```kotlin
            desktopSound = desktopSound,
            onSetDesktopSound = {
                desktopSound = it
                DesktopAudio.setEnabled(context, it)
            },
```

Add the imports `dev.linuxlink.android.Prefs` (if absent) and `dev.linuxlink.android.stream.DesktopAudio`.

- [ ] **Step 4: Build, lint and test**

Run (in `android/`, in the background): `./gradlew --offline assembleDebug lintDebug testDebugUnitTest`
Expected: BUILD SUCCESSFUL, lint still 21 with no finding in a changed file (in particular, no `MissingTranslation`).

- [ ] **Step 5: Commit**

```bash
git add android/app/src/main/java/dev/linuxlink/android/ui/QuickSettingsSheet.kt \
  android/app/src/main/java/dev/linuxlink/android/ui/RemoteScreen.kt \
  android/app/src/main/res/values/strings.xml android/app/src/main/res/values-es/strings.xml \
  android/app/src/main/res/values-ta/strings.xml
git commit -m "feat(android): a switch for desktop sound in Quick Settings

\"Desktop sound on this phone\" in the Streaming group turns playback off
and on live and is remembered (on by default). en/es/ta.

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: The capability table, docs and status say the phone plays it

**Files:**
- Modify: `core/src/capabilities.rs:60-70` (doc comment), `:216-222` (the `desktop -> phone` row) and `:687` (the test assertion)
- Modify: `docs/capabilities.md` (Linux; edited to match, since it can only be regenerated on Linux) and `docs/capabilities-windows.md` (regenerated)
- Modify: `docs/device-verification-checklist.md` (new §25), `README.md` (only if an audio line claims the phone is silent), `AGENTS.md` (status bullet)

- [ ] **Step 1: Flip the test first**

In `core/src/capabilities.rs`'s `audio_rows_say_which_end_can_hear_them`, change `assert!(!played("desktop -> phone"));` to `assert!(played("desktop -> phone"));`.

Run: `cargo test -p linux-link-core --lib audio_rows_say_which_end_can_hear_them`
Expected: FAIL (the row still says `false`).

- [ ] **Step 2: Flip the row and its comments**

The row's comment becomes "The Android app plays it: a MediaCodec Opus decoder into a low-latency AudioTrack (`DesktopAudioPlayer.kt`), on by default behind a Quick Settings switch." Set `played_on_the_receiving_end: true`. In the field's doc comment (lines ~60-67), remove the sentence that says the phone has no playout path.

Run: `cargo test -p linux-link-core --lib capabilities`
Expected: PASS.

- [ ] **Step 3: Regenerate the Windows doc, and hand-edit the Linux one to match**

Run: `cargo run -q -p linux-link-server -- capabilities --markdown > docs/capabilities-windows.md`

In `docs/capabilities.md`, change the `desktop -> phone` row's last cell from `**no**` to `yes`. The generator prints the "on the wire but not in the air" paragraph only for unplayed rows, so delete it (line ~50) and the blank line before it.

Run: `cargo test -p linux-link-server --test capabilities_doc` (it checks the Windows doc on this box).
Expected: PASS. Then `git diff docs/capabilities-windows.md`, and expect only the `played` cell and that paragraph to change. If anything else changed, the file was already stale: stop and report it rather than committing unrelated drift.

- [ ] **Step 4: Checklist, README, AGENTS**

Append to `docs/device-verification-checklist.md`, before `## Recording results`:

```markdown
## 25. Desktop sound on the phone

Built on `feat/phone-audio`; spec `docs/superpowers/specs/2026-10-12-phone-audio-playback-design.md`. Nothing here has run on a phone yet.

- [ ] Play a video on the desktop during a session: the phone plays its sound, and the lag behind the picture is small (≤ ~150 ms by ear).
- [ ] Quick Settings → "Desktop sound on this phone" off: silent at once; on again: sound returns within a second. The setting survives an app restart.
- [ ] Pause the desktop's audio for 10 s, then resume: it comes back cleanly, without a crackle burst.
- [ ] A phone call (or another app playing music) silences it; afterwards it resumes.
- [ ] In PiP the sound continues; Exit stops it.
- [ ] With sound on, the HUD's fps and e2e match a session with it off, and taps on a still desktop feel immediate (the bridge lock fixes).
- [ ] `adb logcat -s DesktopAudio`: "desktop sound started", and on exit "stopped (N underruns)" with N small.
```

README: run `grep -n -i "audio" README.md`. If a line says desktop sound is not played on the phone, correct it to say the phone plays it, with the switch. Otherwise leave README alone.

AGENTS.md: add a status bullet at the top of *Current Status*:

```markdown
- 2026-10-12: **The phone plays the desktop's sound (branch `feat/phone-audio`; spec `docs/superpowers/specs/2026-10-12-phone-audio-playback-design.md`).**
  - A MediaCodec Opus decoder feeds a low-latency AudioTrack, with a fixed 60/150 ms budget, audio focus and a live "Desktop sound on this phone" switch (on by default). It follows the video stream's lifetime.
  - Fixed on the way: the bridge's frame and audio receives waited while holding the session lock, so a still screen delayed every tap by up to 50 ms. Both receivers now have their own locks.
  - `linux-link capabilities` now says `desktop -> phone ... played: yes`.
  - Not verified: everything on a phone (checklist §25; no phone was attached). Out of scope, as roadmap items: measured jitter sizing, drift, A/V sync, PLC (2792-2795).
  - Gates: <fill in the measured counts from Step 5>.
```

- [ ] **Step 5: Full gates**

Run: `cargo fmt --all -- --check`, workspace clippy, `cargo test --workspace --exclude linux-link-android-bridge --exclude iroh-spike`, then in `android/bridge` `cargo clippy --all-targets -- -D warnings` and `cargo test`. Then the Android gates in the background. Put the real counts into the AGENTS bullet.

- [ ] **Step 6: Commit**

```bash
git add core/src/capabilities.rs docs/capabilities.md docs/capabilities-windows.md \
  docs/device-verification-checklist.md AGENTS.md
# plus README.md only if Step 4 changed it
git commit -m "docs: the capability table says the phone plays desktop sound

The desktop -> phone audio row flips to played: yes, its test with it.
docs/capabilities-windows.md is regenerated; docs/capabilities.md is
edited to match and owes its regeneration on Linux. Checklist §25 lists
the device checks, and AGENTS.md records the feature and the bridge
lock fix.

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```
