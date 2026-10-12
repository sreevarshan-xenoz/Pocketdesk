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
class DesktopAudioPlayer {
    @Volatile
    private var running = false
    private var worker: Thread? = null

    /** Starts playing; [context] is held only by the playout thread, as its application context. */
    fun start(context: Context) {
        if (running) return
        running = true
        val appContext = context.applicationContext
        worker = Thread({ run(appContext) }, "desktop-audio").apply {
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
    private fun run(context: Context) {
        repeat(2) { attempt ->
            if (!running) return
            val outcome = runCatching { playUntilStopped(context) }
            outcome.exceptionOrNull()?.let { Log.w(TAG, "playback failed (attempt ${attempt + 1})", it) }
            if (outcome.isSuccess) return
        }
        Log.w(TAG, "desktop sound is off for this session")
    }

    private fun playUntilStopped(context: Context) {
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
