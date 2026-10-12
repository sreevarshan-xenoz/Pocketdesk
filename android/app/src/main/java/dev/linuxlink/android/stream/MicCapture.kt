package dev.linuxlink.android.stream

import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaCodec
import android.media.MediaCodecList
import android.media.MediaFormat
import android.media.MediaRecorder
import dev.linuxlink.android.bridge.RustCore
import java.util.concurrent.ConcurrentLinkedQueue

/**
 * R4 E2 phone-mic share: capture the phone microphone, encode it to Opus
 * with MediaCodec, and stream 20 ms packets to the desktop, where the
 * server's mic relay feeds a pw-loopback virtual source ("Pocketdesk Mic")
 * apps can select.
 *
 * Opus encoding lives here, not in the Rust bridge, deliberately: the
 * `opus` crate builds libopus through CMake, which does not survive
 * cargo-ndk cross-compilation. The AOSP software Opus encoder (API 28+)
 * covers the same ground, so [isAvailable] probes for it and the feature
 * is simply absent on older devices (minSdk 26).
 *
 * The wire contract is fixed on both ends: 48 kHz mono, 20 ms frames
 * (960 samples / 1920 PCM bytes), matching the server's decoder and the
 * PipeWire node properties.
 */
class MicCapture {
    @Volatile
    private var running = false
    private var worker: Thread? = null
    private var record: AudioRecord? = null
    private var codec: MediaCodec? = null

    // Encoded packets are drained into this queue and shipped on the same
    // worker thread, so a slow send never starves the PCM reader.
    private val pending = ConcurrentLinkedQueue<ByteArray>()

    /**
     * Returns null on success, or a human-readable reason on failure; on
     * failure every resource (including the server-side start) is already
     * rolled back. [onDropped] fires on the worker thread when the session
     * dies under us (transport failures in a row) so the UI can clear its
     * toggle.
     *
     * RECORD_AUDIO must be granted by the caller — RemoteScreen checks it
     * (and requests it) before calling, so the AudioRecord construction
     * here can't be rejected.
     */
    @android.annotation.SuppressLint("MissingPermission")
    fun start(onDropped: () -> Unit): String? {
        if (running) return null
        RustCore.startMic().exceptionOrNull()?.let { return it.message ?: "startMic failed" }
        val encoder =
            try {
                // createEncoderByType, not the ByString variant: API 37's
                // android.jar removed the latter.
                MediaCodec.createEncoderByType(MIME).apply {
                    configure(
                        MediaFormat.createAudioFormat(MIME, SAMPLE_RATE, 1).apply {
                            setInteger(MediaFormat.KEY_BIT_RATE, BIT_RATE)
                            setInteger(MediaFormat.KEY_PCM_ENCODING, ENCODING)
                        },
                        null,
                        null,
                        MediaCodec.CONFIGURE_FLAG_ENCODE,
                    )
                    start()
                }
            } catch (e: Exception) {
                RustCore.stopMic()
                return "Opus encoder: ${e.message}"
            }
        val recorder =
            try {
                AudioRecord(
                    MediaRecorder.AudioSource.MIC,
                    SAMPLE_RATE,
                    CHANNEL_IN,
                    ENCODING,
                    AudioRecord.getMinBufferSize(SAMPLE_RATE, CHANNEL_IN, ENCODING)
                        .coerceAtLeast(FRAME_BYTES * 4),
                ).also { it.startRecording() }
            } catch (e: Exception) {
                encoder.release()
                RustCore.stopMic()
                return "Microphone: ${e.message}"
            }
        if (recorder.recordingState != AudioRecord.RECORDSTATE_RECORDING) {
            recorder.release()
            encoder.release()
            RustCore.stopMic()
            return "Microphone unavailable"
        }
        codec = encoder
        record = recorder
        running = true
        worker =
            Thread {
                runLoop(encoder, recorder, onDropped)
            }.apply {
                name = "MicCapture"
                isDaemon = true
                start()
            }
        return null
    }

    private fun runLoop(encoder: MediaCodec, recorder: AudioRecord, onDropped: () -> Unit) {
        val pcm = ByteArray(FRAME_BYTES)
        val info = MediaCodec.BufferInfo()
        var frameIndex = 0L
        var sendFailures = 0
        try {
            while (running) {
                var offset = 0
                while (offset < FRAME_BYTES && running) {
                    val n = recorder.read(pcm, offset, FRAME_BYTES - offset)
                    if (n <= 0) break
                    offset += n
                }
                if (!running) break
                if (offset < FRAME_BYTES) continue // record error/read short of a frame: retry
                val inIdx = encoder.dequeueInputBuffer(INPUT_TIMEOUT_US)
                if (inIdx >= 0) {
                    encoder.getInputBuffer(inIdx)!!.apply {
                        clear()
                        put(pcm)
                    }
                    encoder.queueInputBuffer(inIdx, 0, FRAME_BYTES, frameIndex * FRAME_US, 0)
                    frameIndex++
                }
                drain(encoder, info)
                while (true) {
                    val packet = pending.poll() ?: break
                    if (RustCore.sendMicOpus(packet).isFailure) {
                        pending.clear()
                        // Twice in a row means the session is gone; a single
                        // blip may just be one congested stream.
                        if (++sendFailures >= 2) {
                            onDropped()
                            return
                        }
                        break
                    }
                    sendFailures = 0
                }
            }
        } catch (_: Exception) {
            // Codec or record died under us (e.g. another app grabbed the mic).
        } finally {
            cleanup()
        }
    }

    private fun drain(encoder: MediaCodec, info: MediaCodec.BufferInfo) {
        while (true) {
            val outIdx = encoder.dequeueOutputBuffer(info, 0)
            if (outIdx < 0) break // INFO_TRY_AGAIN_LATER or format change (none expected)
            if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0 && info.size > 0) {
                val out = encoder.getOutputBuffer(outIdx)!!
                val packet = ByteArray(info.size)
                out.position(info.offset)
                out.limit(info.offset + info.size)
                out.get(packet)
                pending.add(packet)
            }
            encoder.releaseOutputBuffer(outIdx, false)
            if (info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) break
        }
    }

    /** Idempotent: safe to call after the worker already cleaned up. */
    fun stop() {
        if (worker == null && !running) return
        running = false
        worker?.join(500)
        worker = null
        cleanup()
        RustCore.stopMic()
    }

    private fun cleanup() {
        running = false
        runCatching { codec?.stop() }
        codec?.release()
        codec = null
        runCatching {
            if (record?.recordingState == AudioRecord.RECORDSTATE_RECORDING) record?.stop()
        }
        record?.release()
        record = null
        pending.clear()
    }

    companion object {
        private const val MIME = "audio/opus"
        private const val SAMPLE_RATE = 48_000
        private const val BIT_RATE = 32_000
        private const val FRAME_MS = 20
        private const val FRAME_BYTES = SAMPLE_RATE / 1000 * FRAME_MS * 2 // 960 s16 samples
        private const val FRAME_US = FRAME_MS * 1_000L
        private const val CHANNEL_IN = AudioFormat.CHANNEL_IN_MONO
        private const val ENCODING = AudioFormat.ENCODING_PCM_16BIT
        private const val INPUT_TIMEOUT_US = 20_000L

        /**
         * MediaCodec's software Opus encoder ("c2.android.opus.encoder")
         * arrived in Android 9 (API 28); minSdk is 26, so probe the codec
         * list instead of assuming it.
         */
        fun isAvailable(): Boolean =
            MediaCodecList(MediaCodecList.ALL_CODECS).codecInfos.any { info ->
                info.isEncoder && info.supportedTypes.any { it.equals(MIME, ignoreCase = true) }
            }
    }
}
