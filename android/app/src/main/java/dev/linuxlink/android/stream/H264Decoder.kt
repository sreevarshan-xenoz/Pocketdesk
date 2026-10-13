package dev.linuxlink.android.stream

import android.media.MediaCodec
import android.media.MediaFormat
import android.os.Build
import android.os.SystemClock
import android.util.Log
import android.view.Surface
import dev.linuxlink.android.bridge.RustCore
import java.nio.ByteBuffer
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Low-latency H.264/H.265 decoder that drains encoded access units from the
 * Rust bridge ([RustCore.receiveFrames]) and renders them straight to a
 * [Surface].
 *
 * Runs on a dedicated thread ([start] blocks); stop it by clearing [running]
 * and joining via [RustCore.stopStreaming] on the caller side. The stream is
 * Annex-B NAL units with in-band SPS/PPS (the encoder keyframes carry them),
 * so buffers are fed to MediaCodec as-is; the decoder signals the real format
 * on the first output buffer.
 *
 * R4 C1: the codec is not known at start — the server only upgrades to
 * HEVC when both ends negotiate it — so configuration is deferred to the
 * first keyframe, whose NAL header is sniffed for `video/avc` vs
 * `video/hevc` (`sniffMime`). Frames before the first IDR are dropped (the
 * server always starts a pipeline with an IDR, so this is a few ms at most).
 *
 * [width]/[height] are only the initial guess — window-crop rebuilds the
 * server encoder at the window's resolution mid-session, so when
 * INFO_OUTPUT_FORMAT_CHANGED reports different dimensions the codec is
 * reconfigured and the cached keyframe re-seeds it (the server always emits
 * an IDR as the first packet after a rebuild). The new size is announced via
 * [onVideoSize] so the UI can re-layout.
 *
 * R4 C2: the codec CAN change mid-session — the server's encoder-fallback
 * ladder degrades to software H.264 when a hardware encoder dies — so every
 * keyframe is re-sniffed; on a MIME change the MediaCodec instance is swapped
 * and the fresh keyframe re-seeds it, with [onCodec] notifying the UI.
 *
 * Roadmap Phase 1 (decode/render percentiles): this is the only place the
 * phone's half of the latency chain is visible, so each feed↔output pair is
 * measured here and handed to the bridge via [RustCore.recordSample]; the
 * desktop folds them into the session record's `dec_*`/`rnd_*` tails.
 */
class H264Decoder(
    private val surface: Surface,
    width: Int,
    height: Int,
    private val onVideoSize: ((Int, Int) -> Unit)? = null,
    private val onCodec: ((String) -> Unit)? = null,
) {
    val running = AtomicBoolean(false)

    /**
     * Fired from the decode thread when the bridge goes silent for
     * [STALL_MS] — see the decode loop in [start]. Set after construction so
     * it can close over the generation guard in [RemoteDesktopView].
     */
    var onStalled: (() -> Unit)? = null

    /** Fired from the decode thread when the desktop closes the stream, with its reason. */
    var onClosed: ((String) -> Unit)? = null

    private var codec: MediaCodec? = null
    private var configW = width
    private var configH = height
    private var mime = MIME_AVC

    /**
     * The run since the latest keyframe: that keyframe and every frame fed
     * after it. A reconfigure restarts the codec, and re-feeding only the
     * keyframe left the next P-frame referring to frames the new run never
     * saw — the decoder rejects that as a frame-number gap and shows nothing
     * useful until the next IDR, seconds away on a quiet desktop.
     */
    private val run = ArrayList<ByteArray>()
    private var runBytes = 0L

    /**
     * Set when a frame could not be fed: until the next keyframe, every
     * P-frame would decode against a missing reference, so they are skipped.
     */
    private var awaitingKeyframe = false

    /**
     * When each fed-but-not-yet-rendered access unit went in, and the moment the
     * previous frame reached the panel. The desktop can measure capture, encode
     * and the wire; these two are the phone's half of the chain, and the bridge
     * ships them back for the session record.
     */
    private val fedAtUs = ArrayDeque<Long>()
    private var lastRenderAtUs = 0L

    /** Forget a run's timings: its feeds can no longer produce outputs. */
    private fun resetTimings() {
        fedAtUs.clear()
        lastRenderAtUs = 0L
    }

    /**
     * One frame reached the panel. Pair it with the feed that produced it
     * (decode time) and with the frame before it (render cadence). Feeds and
     * outputs line up because the stream is B-frame-free (R2#1); anything
     * unmatched — the frame that straddles a reconfigure, say — goes
     * unmeasured rather than being reported against the wrong feed.
     */
    private fun reportTimings() {
        val now = nowMicros()
        if (fedAtUs.isNotEmpty()) {
            RustCore.recordSample(RustCore.SAMPLE_DECODE, now - fedAtUs.removeFirst())
        }
        val sinceRender = now - lastRenderAtUs
        // A longer gap than this is the link sitting idle between frames, not
        // the panel being slow to draw one.
        if (lastRenderAtUs != 0L && sinceRender in 1 until RENDER_GAP_MAX_US) {
            RustCore.recordSample(RustCore.SAMPLE_RENDER, sinceRender)
        }
        lastRenderAtUs = now
    }

    /**
     * Configure MediaCodec for real-time, minimum-latency decode.
     *
     * - `KEY_LOW_LATENCY` (API 30+) drops reordering/buffering delay.
     * - `KEY_PRIORITY` real-time requests the fastest scheduling.
     * B-frame-free encoder output (R2#1) is what actually keeps `LOW_LATENCY`
     * honored end to end.
     */
    private fun buildFormat(
        mime: String,
        width: Int,
        height: Int,
    ): MediaFormat {
        val format = MediaFormat.createVideoFormat(mime, width, height)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            format.setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
        }
        // 0 = realtime priority class (highest urgency for the codec scheduler).
        format.setInteger(MediaFormat.KEY_PRIORITY, 0)
        format.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, MAX_INPUT_SIZE)
        return format
    }

    /**
     * Blocking decode loop. Waits for the first keyframe to learn the codec,
     * configures MediaCodec, then repeatedly pulls a batch of frames and
     * feeds them, rendering every decoded output buffer to the surface
     * immediately.
     */
    fun start() {
        running.set(true)
        var mediaCodec: MediaCodec? = null
        try {
            val bufferInfo = MediaCodec.BufferInfo()
            // Liveness baseline. The bridge only clears its "active" flag on an
            // explicit stop, so a desktop-side teardown (server kick, Wi-Fi
            // loss, process exit) leaves the polls going quiet forever: the UI
            // kept showing a frozen frame with no error and no way out. The
            // encoder runs keyint=60, so a live link delivers a packet every
            // couple of seconds — a long gap means the link is gone, not that
            // the desktop is still.
            //
            // The first frame gets longer: a desktop that asks before each
            // session holds the stream until its owner answers (up to 30 s).
            var lastFrameAt = SystemClock.elapsedRealtime()
            var sawFrame = false
            while (running.get()) {
                val frames = RustCore.receiveFrames(timeoutMs = POLL_TIMEOUT_MS)
                if (frames.isEmpty()) {
                    // The desktop closed the stream (refused the session,
                    // ended it): say so now rather than after the watchdog.
                    RustCore.streamClosedReason?.let { reason ->
                        Log.w(TAG, "stream closed by the desktop: $reason")
                        onClosed?.invoke(reason)
                        break
                    }
                    val silentFor = SystemClock.elapsedRealtime() - lastFrameAt
                    if (silentFor > if (sawFrame) STALL_MS else FIRST_FRAME_MS) {
                        Log.w(TAG, "video feed silent for $silentFor ms — link dropped")
                        onStalled?.invoke()
                        break
                    }
                    continue
                }
                lastFrameAt = SystemClock.elapsedRealtime()
                sawFrame = true
                for (frame in frames) {
                    if (!running.get()) break
                    if (mediaCodec == null) {
                        // Annex-B access-unit sniffing needs a keyframe header.
                        if (!frame.isKeyframe) continue
                        mime = sniffMime(frame.data)
                        mediaCodec = openCodec(mime)
                    } else if (frame.isKeyframe) {
                        // R4 C2: the server's fallback ladder can rebuild
                        // mid-session on a different codec (HEVC → H.264).
                        // Keyframes are the only safe place to notice —
                        // re-sniff and swap if the header disagrees.
                        val sniffed = sniffMime(frame.data)
                        if (sniffed != mime) {
                            val replacement = runCatching { openCodec(sniffed) }.getOrNull()
                            if (replacement != null) {
                                runCatching { mediaCodec.stop() }
                                runCatching { mediaCodec.release() }
                                mediaCodec = replacement
                                mime = sniffed
                                onCodec?.invoke(codecLabel(sniffed))
                            }
                        }
                    }
                    feed(mediaCodec, bufferInfo, frame.data, frame.isKeyframe)
                }
            }
        } finally {
            running.set(false)
            mediaCodec?.let { opened ->
                runCatching { opened.stop() }
                runCatching { opened.release() }
            }
            codec = null
        }
    }

    /** Create + configure + start a decoder instance for [mime] at the
     * current known size. */
    private fun openCodec(mime: String): MediaCodec =
        MediaCodec.createDecoderByType(mime).also {
            it.configure(buildFormat(mime, configW, configH), surface, null, 0)
            it.start()
            resetTimings()
            codec = it
        }

    fun stop() {
        running.set(false)
    }

    private fun feed(
        codec: MediaCodec,
        info: MediaCodec.BufferInfo,
        data: ByteArray,
        isKeyframe: Boolean,
    ) {
        if (isKeyframe) {
            awaitingKeyframe = false
            run.clear()
            runBytes = 0
        } else if (awaitingKeyframe) {
            return
        }
        val inIndex = dequeueInput(codec, info)
        if (inIndex < 0) {
            Log.w(TAG, "decoder took no input; skipping to the next keyframe")
            awaitingKeyframe = true
            run.clear()
            runBytes = 0
            return
        }
        remember(data)
        val inputBuffer: ByteBuffer = codec.getInputBuffer(inIndex) ?: run {
            codec.queueInputBuffer(inIndex, 0, 0, 0, 0)
            return
        }
        inputBuffer.clear()
        inputBuffer.put(data)
        var flags = 0
        if (isKeyframe) flags = flags or MediaCodec.BUFFER_FLAG_KEY_FRAME
        codec.queueInputBuffer(inIndex, 0, data.size, System.nanoTime() / 1000, flags)
        // Only a frame that actually went into the codec can come out of it.
        if (fedAtUs.size >= MAX_PENDING_FEEDS) {
            // A real-time decoder never holds this many frames, so the queue has
            // lost sync with the output run. Keep the recent feeds and drop the
            // stale one instead of growing without bound.
            fedAtUs.removeFirst()
        }
        fedAtUs.addLast(nowMicros())

        drain(codec, info)
    }

    /** Keep [data] in the current run while the run stays a sane size. */
    private fun remember(data: ByteArray) {
        if (runBytes + data.size > MAX_RUN_BYTES) {
            // A run this long only re-seeds partially; the keyframe is still
            // worth keeping, the tail is not.
            while (run.size > 1) runBytes -= run.removeAt(run.size - 1).size
            return
        }
        run.add(data)
        runBytes += data.size
    }

    /**
     * An input buffer, draining output while waiting so a decoder holding
     * full output buffers can free its input side. -1 if none came.
     */
    private fun dequeueInput(
        codec: MediaCodec,
        info: MediaCodec.BufferInfo,
    ): Int {
        repeat(INPUT_ATTEMPTS) {
            val inIndex = codec.dequeueInputBuffer(INPUT_TIMEOUT_US)
            if (inIndex >= 0) return inIndex
            drain(codec, info)
        }
        return -1
    }

    private fun drain(
        codec: MediaCodec,
        info: MediaCodec.BufferInfo,
    ) {
        var outIndex = codec.dequeueOutputBuffer(info, 0)
        while (outIndex >= 0) {
            // Render to the surface as soon as a frame is available.
            codec.releaseOutputBuffer(outIndex, true)
            reportTimings()
            outIndex = codec.dequeueOutputBuffer(info, 0)
        }
        if (outIndex == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED && maybeReconfigure(codec)) {
            return // reconfigure re-fed the stream; resume draining next frame
        }
    }

    /**
     * Reconfigure the codec when the signaled format size differs from what
     * it was configured with. Returns true if a reconfigure happened (the
     * caller must not consume output from the old run).
     */
    private fun maybeReconfigure(codec: MediaCodec): Boolean {
        val format = codec.outputFormat
        val w = format.getInteger(MediaFormat.KEY_WIDTH)
        val h = format.getInteger(MediaFormat.KEY_HEIGHT)
        if (w <= 0 || h <= 0) return false
        if (w == configW && h == configH) {
            // First format change also confirms the guessed size: report it
            // so the UI letterboxes (and maps direct-touch taps) against the
            // real frame, not the container.
            onVideoSize?.invoke(w, h)
            return false
        }
        configW = w
        configH = h
        // Whatever is still queued belongs to the run that just ended.
        resetTimings()
        runCatching {
            codec.stop()
            codec.configure(buildFormat(mime, w, h), surface, null, 0)
            codec.start()
        }.onFailure { return true }
        onVideoSize?.invoke(w, h)
        // Re-seed: the new run needs the keyframe and every frame since, in
        // order, or the next live P-frame has nothing to refer to. Output is
        // released unrendered; only the newest picture is worth showing.
        val seeds = run.toList()
        runCatching {
            for ((i, seed) in seeds.withIndex()) {
                var inIndex = codec.dequeueInputBuffer(INPUT_TIMEOUT_US)
                var attempts = 1
                while (inIndex < 0 && attempts < INPUT_ATTEMPTS) {
                    discardOutput(codec)
                    inIndex = codec.dequeueInputBuffer(INPUT_TIMEOUT_US)
                    attempts++
                }
                if (inIndex < 0) {
                    Log.w(TAG, "re-seed stalled after $i of ${seeds.size} frames")
                    awaitingKeyframe = true
                    break
                }
                codec.getInputBuffer(inIndex)?.let { buf ->
                    buf.clear()
                    buf.put(seed)
                }
                val flags = if (i == 0) MediaCodec.BUFFER_FLAG_KEY_FRAME else 0
                codec.queueInputBuffer(inIndex, 0, seed.size, System.nanoTime() / 1000, flags)
                discardOutput(codec)
            }
        }.onFailure { awaitingKeyframe = true }
        return true
    }

    /** Release ready output without drawing it (re-seed frames are stale). */
    private fun discardOutput(codec: MediaCodec) {
        val info = MediaCodec.BufferInfo()
        var outIndex = codec.dequeueOutputBuffer(info, 0)
        while (outIndex >= 0) {
            codec.releaseOutputBuffer(outIndex, false)
            outIndex = codec.dequeueOutputBuffer(info, 0)
        }
    }

    companion object {
        private const val TAG = "H264Decoder"
        private const val MIME_AVC = "video/avc"
        private const val MIME_HEVC = "video/hevc"
        private const val MAX_INPUT_SIZE = 4 * 1024 * 1024
        private const val POLL_TIMEOUT_MS = 50
        private const val INPUT_TIMEOUT_US = 10_000L

        /** Waits for an input buffer before a frame is given up (~50 ms). */
        private const val INPUT_ATTEMPTS = 5

        /** Bound on the run kept for re-seeding (a few seconds at desktop rates). */
        private const val MAX_RUN_BYTES = 16L * 1024 * 1024

        /**
         * Monotonic microseconds, deep sleep included. `elapsedRealtimeNanos` is
         * API 17 and present in the SDK jars we build against; the nicer
         * `elapsedRealtimeMicros` is not, so the conversion happens here.
         */
        private fun nowMicros(): Long = SystemClock.elapsedRealtimeNanos() / 1000

        /** Frame gap that means the link is dead rather than the desktop idle. */
        private const val STALL_MS = 10_000L

        /** The wait for a session's first frame: the desk's 30 s approval, and margin. */
        private const val FIRST_FRAME_MS = 45_000L

        /** Pending feeds before the feed↔output pairing is considered lost. */
        private const val MAX_PENDING_FEEDS = 8

        /** A longer gap between rendered frames is an idle desktop, not a slow panel. */
        private const val RENDER_GAP_MAX_US = 1_000_000L

        /**
         * R4 C1: decide the codec from the first NAL header of an Annex-B
         * keyframe access unit. Our encoders start H.264 keyframes with SPS
         * (nal_unit_type 7 → first byte 0x67) and HEVC keyframes with VPS/SPS
         * (nal_unit_type 32/33 in bits 1..6 → 0x40/0x42, followed by the
         * layer/temporal byte 0x01). Anything unrecognized stays H.264 —
         * the negotiated default.
         */
        fun sniffMime(data: ByteArray): String {
            var i = 0
            while (i < data.size && data[i].toInt() == 0) i++
            // Skip the start-code 0x01 after the zero run (3- or 4-byte code).
            if (i >= data.size || data[i].toInt() != 1) return MIME_AVC
            i++
            if (i + 1 >= data.size) return MIME_AVC
            val b0 = data[i].toInt() and 0xFF
            val b1 = data[i + 1].toInt() and 0xFF
            val hevcType = (b0 shr 1) and 0x3F
            return if ((hevcType == 32 || hevcType == 33) && b1 == 0x01) MIME_HEVC else MIME_AVC
        }

        /** UI-facing codec name for the switch notice (R4 C2). */
        fun codecLabel(mime: String): String = if (mime == MIME_HEVC) "H.265" else "H.264"
    }
}
