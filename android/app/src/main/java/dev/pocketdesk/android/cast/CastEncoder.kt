package dev.pocketdesk.android.cast

import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.view.Surface
import dev.pocketdesk.android.bridge.RustCore
import java.nio.ByteBuffer

/**
 * The phone's screen on the computer (T10k): an H.264 encoder fed by a
 * [Surface] the virtual display draws into. Its thread tells the bridge the
 * size, then hands it each access unit; keyframes carry the stream's SPS and
 * PPS, so the computer can start from any of them. A keyframe is sent every
 * [CastPlan.KEYFRAME_INTERVAL_S], when the computer asks, and after the
 * link dropped a frame. [onEnded] runs (on its thread) when the cast
 * session is gone.
 */
class CastEncoder(
    val width: Int,
    val height: Int,
    private val onEnded: () -> Unit,
) {
    private val codec: MediaCodec = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
    val surface: Surface

    @Volatile
    private var running = true
    private val thread: Thread

    init {
        try {
            configure(baseline = true)
        } catch (e: Exception) {
            // An encoder without the Baseline profile: its default, still
            // without B-frames where the platform can say so.
            Log.i(TAG, "Baseline refused (${e.message}), using the encoder's default profile")
            codec.reset()
            configure(baseline = false)
        }
        surface = codec.createInputSurface()
        codec.start()
        thread = Thread(::drain, "cast-encode").apply { start() }
    }

    private fun configure(baseline: Boolean) {
        val format =
            MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, width, height).apply {
                setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
                setInteger(MediaFormat.KEY_BIT_RATE, CastPlan.BIT_RATE)
                setInteger(MediaFormat.KEY_FRAME_RATE, CastPlan.FRAME_RATE)
                setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, CastPlan.KEYFRAME_INTERVAL_S)
                setLong(MediaFormat.KEY_REPEAT_PREVIOUS_FRAME_AFTER, CastPlan.REPEAT_AFTER_US)
                // Realtime: frames are encoded as they come, not batched.
                setInteger(MediaFormat.KEY_PRIORITY, 0)
                if (baseline) {
                    setInteger(MediaFormat.KEY_PROFILE, MediaCodecInfo.CodecProfileLevel.AVCProfileBaseline)
                }
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    setInteger(MediaFormat.KEY_MAX_B_FRAMES, 0)
                }
            }
        codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
    }

    /** Ask for a keyframe now. */
    fun requestKeyframe() {
        runCatching {
            codec.setParameters(Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0) })
        }
    }

    private fun drain() {
        val info = MediaCodec.BufferInfo()
        var parameterSets: ByteArray? = null
        var seq = 0L
        try {
            // The computer is told the size before any frame of it.
            var told = RustCore.castConfig(width, height)
            while (running && told == CastPlan.FULL) told = RustCore.castConfig(width, height)
            if (told == CastPlan.NO_CAST) return end()
            while (running) {
                if (RustCore.castKeyframeWanted()) requestKeyframe()
                val index = codec.dequeueOutputBuffer(info, DEQUEUE_TIMEOUT_US)
                if (index < 0) continue
                val buffer = codec.getOutputBuffer(index)
                val ended = info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0
                if (buffer != null && info.size > 0) {
                    if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0) {
                        parameterSets = ByteArray(info.size).also {
                            buffer.position(info.offset)
                            buffer.get(it)
                        }
                    } else {
                        val keyframe = info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0
                        val sets = parameterSets
                        val result =
                            if (keyframe && sets != null) {
                                val whole = ByteBuffer.allocateDirect(sets.size + info.size)
                                whole.put(sets)
                                buffer.limit(info.offset + info.size).position(info.offset)
                                whole.put(buffer)
                                RustCore.castFrame(whole, 0, whole.position(), true, seq)
                            } else {
                                RustCore.castFrame(buffer, info.offset, info.size, keyframe, seq)
                            }
                        seq++
                        if (CastPlan.wantsKeyframe(result)) requestKeyframe()
                        if (result == CastPlan.NO_CAST) {
                            codec.releaseOutputBuffer(index, false)
                            return end()
                        }
                    }
                }
                codec.releaseOutputBuffer(index, false)
                if (ended) break
            }
        } catch (e: Exception) {
            // Released under it, or the codec failed: the cast cannot go on.
            if (running) {
                Log.w(TAG, "The cast encoder stopped", e)
                end()
            }
        }
    }

    private fun end() {
        if (running) {
            running = false
            onEnded()
        }
    }

    /** Stop and free it; [onEnded] does not run. */
    fun release() {
        running = false
        runCatching { thread.join(JOIN_MS) }
        runCatching { codec.stop() }
        runCatching { codec.release() }
        runCatching { surface.release() }
    }

    private companion object {
        const val TAG = "CastEncoder"
        const val DEQUEUE_TIMEOUT_US = 100_000L
        const val JOIN_MS = 3_000L
    }
}
