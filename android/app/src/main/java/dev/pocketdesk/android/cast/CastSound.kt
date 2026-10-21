package dev.pocketdesk.android.cast

import android.annotation.SuppressLint
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioPlaybackCaptureConfiguration
import android.media.AudioRecord
import android.media.MediaCodec
import android.media.MediaFormat
import android.media.projection.MediaProjection
import android.util.Log
import androidx.annotation.RequiresApi
import dev.pocketdesk.android.bridge.RustCore

/**
 * What the phone's apps play, sent with its screen (K5): captured under the
 * cast's projection (Android 10+), encoded to Opus at 48 kHz stereo in
 * 20 ms frames with MediaCodec's encoder, as the mic share's, and queued on
 * the cast's own sound queue. Apps that forbid capture, and calls, are
 * silent: Android leaves them out.
 */
class CastSound(private val projection: MediaProjection) {
    @Volatile
    private var running = false
    private var worker: Thread? = null

    /** Whether it still captures: it ends by itself when the capture or the link fails (M7). */
    val alive: Boolean get() = running

    /** Start capturing; whether it runs. RECORD_AUDIO is the caller's check. */
    @SuppressLint("MissingPermission")
    @RequiresApi(29)
    fun start(): Boolean {
        if (running) return true
        val config =
            AudioPlaybackCaptureConfiguration.Builder(projection)
                .addMatchingUsage(AudioAttributes.USAGE_MEDIA)
                .addMatchingUsage(AudioAttributes.USAGE_GAME)
                .addMatchingUsage(AudioAttributes.USAGE_UNKNOWN)
                .build()
        val format =
            AudioFormat.Builder()
                .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                .setSampleRate(SAMPLE_RATE)
                .setChannelMask(AudioFormat.CHANNEL_IN_STEREO)
                .build()
        val record =
            runCatching {
                AudioRecord.Builder()
                    .setAudioFormat(format)
                    .setBufferSizeInBytes(FRAME_BYTES * 8)
                    .setAudioPlaybackCaptureConfig(config)
                    .build()
                    .also { it.startRecording() }
            }.getOrElse {
                Log.w(TAG, "capture: ${it.message}")
                return false
            }
        val encoder =
            runCatching {
                MediaCodec.createEncoderByType(MIME).apply {
                    configure(
                        MediaFormat.createAudioFormat(MIME, SAMPLE_RATE, 2).apply {
                            setInteger(MediaFormat.KEY_BIT_RATE, BIT_RATE)
                            setInteger(MediaFormat.KEY_PCM_ENCODING, AudioFormat.ENCODING_PCM_16BIT)
                        },
                        null,
                        null,
                        MediaCodec.CONFIGURE_FLAG_ENCODE,
                    )
                    start()
                }
            }.getOrElse {
                Log.w(TAG, "Opus encoder: ${it.message}")
                record.release()
                return false
            }
        running = true
        worker =
            Thread({ loop(record, encoder) }, "CastSound").apply {
                isDaemon = true
                start()
            }
        return true
    }

    private fun loop(record: AudioRecord, encoder: MediaCodec) {
        val pcm = ByteArray(FRAME_BYTES)
        val info = MediaCodec.BufferInfo()
        var frame = 0L
        try {
            while (running) {
                var offset = 0
                while (offset < FRAME_BYTES && running) {
                    val n = record.read(pcm, offset, FRAME_BYTES - offset)
                    // An error (the audio server restarted, the projection
                    // ended) ends the capture: it never comes back (K5 review:
                    // it spun a core).
                    if (n < 0) {
                        Log.w(TAG, "capture ended: $n")
                        running = false
                    }
                    if (n <= 0) break
                    offset += n
                }
                if (!running) break
                if (offset < FRAME_BYTES) continue
                val input = encoder.dequeueInputBuffer(INPUT_TIMEOUT_US)
                if (input >= 0) {
                    encoder.getInputBuffer(input)!!.apply {
                        clear()
                        put(pcm)
                    }
                    encoder.queueInputBuffer(input, 0, FRAME_BYTES, frame * FRAME_US, 0)
                    frame++
                }
                while (true) {
                    val out = encoder.dequeueOutputBuffer(info, 0)
                    if (out < 0) break
                    if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0 && info.size > 0) {
                        val packet = ByteArray(info.size)
                        encoder.getOutputBuffer(out)!!.apply {
                            position(info.offset)
                            limit(info.offset + info.size)
                            get(packet)
                        }
                        // A full queue drops it: late sound is worse than a gap.
                        if (RustCore.castAudio(packet) == RustCore.CAST_NONE) running = false
                    }
                    encoder.releaseOutputBuffer(out, false)
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "the cast's sound stopped: ${e.message}")
        } finally {
            running = false
            runCatching { encoder.stop() }
            encoder.release()
            runCatching { record.stop() }
            record.release()
        }
    }

    /** Stop; safe to call more than once, from any thread. */
    fun stop() {
        running = false
        worker?.join(500)
        worker = null
    }

    private companion object {
        const val TAG = "CastSound"
        const val MIME = "audio/opus"
        const val SAMPLE_RATE = 48_000
        const val BIT_RATE = 96_000
        const val FRAME_MS = 20
        /** 960 stereo 16-bit frames. */
        const val FRAME_BYTES = SAMPLE_RATE / 1000 * FRAME_MS * 2 * 2
        const val FRAME_US = FRAME_MS * 1_000L
        const val INPUT_TIMEOUT_US = 20_000L
    }
}
