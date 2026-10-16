package dev.pocketdesk.android.bridge

import android.content.Context
import android.media.MediaCodec
import org.json.JSONObject
import java.io.File
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.math.roundToInt

/**
 * Kotlin facade over the Rust JNI bridge (`libpocketdesk_android_bridge.so`).
 *
 * Async Rust calls are bridged via `block_on`, so every method here blocks the
 * calling thread; invoke them from a background coroutine/dispatcher, never
 * the main thread. Fallible calls return a JSON envelope `{"ok": ...}` or
 * `{"error": "..."}` which is unwrapped into [Result].
 */
object RustCore {
    init {
        System.loadLibrary("pocketdesk_android_bridge")
    }

    private external fun nativeVersion(): String
    private external fun nativeInit(context: Context)
    private external fun nativeSetDataDir(path: String)
    private external fun nativePinPeerCertificate(address: String, fingerprintHex: String): String
    private external fun nativeForgetPeerCertificate(address: String): String
    private external fun nativeIsDiscoveryActive(): Boolean
    private external fun nativeCheckTailscaleStatus(): String
    private external fun nativeGetPeers(): String
    private external fun nativeConnectToPeer(address: String, port: Int, wanIdentity: String): String
    private external fun nativeSendClipboard(address: String, port: Int, content: String): String
    private external fun nativeGetClipboard(address: String, port: Int): String
    private external fun nativeGetClipboardState(address: String, port: Int): String
    private external fun nativeSendNotification(address: String, port: Int, body: String): String
    private external fun nativeSendPhonePacket(address: String, port: Int, packetType: String, body: String): String
    private external fun nativeSendPhonePacketOnLink(address: String, port: Int, packetType: String, body: String): String
    private external fun nativeTakeSmsRequests(): String
    private external fun nativeControlPeer(): String
    private external fun nativeSendClipboardImage(address: String, port: Int, path: String): String
    private external fun nativeFetchClipboardImage(
        address: String,
        port: Int,
        id: String,
        path: String,
    ): String
    private external fun nativePollIncomingPackets(): String
    private external fun nativeSessionStatus(): String
    private external fun nativeIsStreamingActive(): Boolean
    private external fun nativeStreamClosedReason(): String
    private external fun nativeGetStreamingRtt(): Int
    private external fun nativeGetStreamingStats(): String
    private external fun nativeRecordSample(kind: Int, micros: Long)
    private external fun nativeListTrustedPeers(): String
    private external fun nativeForgetTrustedPeer(label: String): String
    private external fun nativeStopV2(): String
    private external fun nativeStopStreaming(): String
    private external fun nativeConnectStreaming(
        address: String,
        port: Int,
        monitorIndex: Int,
        codecCaps: Int,
    ): String
    private external fun nativeConnectStreamingWan(
        address: String,
        identityJson: String,
        monitorIndex: Int,
        codecCaps: Int,
    ): String
    private external fun nativeGetWanIdentity(): String
    private external fun nativeConnectRemote(address: String, port: Int): String
    private external fun nativeConnectRemoteWan(address: String, identityJson: String): String
    private external fun nativeRemoteState(): String
    private external fun nativeReconnectStreaming(
        address: String,
        port: Int,
        monitorIndex: Int,
        attempt: Int,
        codecCaps: Int,
    ): String

    private external fun nativeResetReconnectBackoff()
    private external fun nativeConnectV2(address: String, port: Int): String
    private external fun nativeReceiveFrames(timeoutMs: Int): ByteArray
    private external fun nativeReceiveAudio(timeoutMs: Int): ByteArray
    private external fun nativeSendMouseEvent(
        address: String,
        port: Int,
        x: Float,
        y: Float,
        button: Int,
        isPressed: Boolean,
    ): String

    private external fun nativeSendMouseAbs(xNorm: Int, yNorm: Int): String
    private external fun nativeSendMouseClick(button: Int, pressed: Boolean): String

    private external fun nativeSendKeyboardEvent(
        address: String,
        port: Int,
        keyCode: Int,
        text: String,
    ): String

    private external fun nativeSendGamepadEvent(axes: IntArray, buttons: Int): String
    private external fun nativeSendFile(address: String, port: Int, filePath: String): String
    private external fun nativeSendFileFd(
        address: String,
        port: Int,
        fd: Int,
        name: String,
        size: Long,
        source: String,
        folderId: String,
        folder: String,
        pathJson: String,
    ): String
    private external fun nativeTransferProgress(): LongArray?
    private external fun nativeFetchFile(
        address: String,
        port: Int,
        isOffer: Boolean,
        value: String,
        fd: Int,
        source: String,
        name: String,
        location: String,
    ): String
    private external fun nativePartialLocation(address: String, source: String): String
    private external fun nativeSweepPartials(maxAgeHours: Int): String
    private external fun nativeTakeFileOffers(): String
    private external fun nativeSendLink(address: String, port: Int, url: String): String
    private external fun nativeTakeLinks(): String
    private external fun nativeTakeMediaPresses(): String
    private external fun nativeTakeClipboardPush(): String
    private external fun nativeTakeNotificationAnswers(): String
    private external fun nativeTakePhoneFileRequests(): String
    private external fun nativeDeclineOffer(address: String, port: Int, id: String, reason: String): String
    private external fun nativeFetchFolder(address: String, port: Int, path: String): String
    private external fun nativeVideoBytes(): Long
    private external fun nativeListRemoteFiles(address: String, port: Int, remotePath: String): String
    private external fun nativeGetMonitors(address: String, port: Int): String
    private external fun nativeVirtualScreen(address: String, port: Int, action: String, width: Int, height: Int): String
    private external fun nativeGetBattery(address: String, port: Int): String
    private external fun nativeCheckSiren(): String
    private external fun nativeSendFindMyDevice(address: String, port: Int): String
    private external fun nativeRequestPairPin(address: String, port: Int): String
    private external fun nativeCheckIdentity(address: String, port: Int): String
    private external fun nativePairWithPin(address: String, port: Int, pin: String): String
    private external fun nativeCheckPairResult(waitSecs: Long): String
    private external fun nativePairedServers(): String
    private external fun nativeTakePendingNotifications(): String
    private external fun nativeDesktopPrivacy(
        address: String,
        port: Int,
        action: String,
        lock: Boolean,
    ): String
    private external fun nativeAudioControl(
        address: String,
        port: Int,
        bodyJson: String,
    ): String
    private external fun nativeSendNotificationReply(
        address: String,
        port: Int,
        id: String,
        text: String,
    ): String
    private external fun nativeGetMonitorCount(address: String, port: Int): String
    private external fun nativeGetWindows(address: String, port: Int): String
    private external fun nativeSendWindowCrop(
        x: Int,
        y: Int,
        width: Int,
        height: Int,
        address: String,
    ): String
    private external fun nativeSetViewOnly(enabled: Boolean): String
    private external fun nativeSetFullQuality(enabled: Boolean): String
    private external fun nativeSendQualityPreset(preset: Int): String
    private external fun nativeStartMic(): String
    private external fun nativeSendMicOpus(opus: ByteArray): String
    private external fun nativeStopMic(): String
    private external fun nativeSendPowerCommand(address: String, port: Int, action: String): String
    private external fun nativeExecuteRemoteCommand(
        address: String,
        port: Int,
        command: String,
    ): String

    private external fun nativeWake(mac: String, targetsJson: String): String

    private external fun nativeWakeInfo(address: String, port: Int): String

    private external fun nativeControlState(address: String, port: Int): String

    private external fun nativeWakeViaRelay(
        address: String,
        port: Int,
        mac: String,
        broadcast: String,
    ): String

    val version: String
        get() = nativeVersion()

    /**
     * Initialize logging, hand the bridge the application context (iroh reads
     * the network's DNS servers through it; without it, relay names went to
     * public resolvers), and point it at the app-private data dir.
     */
    fun start(context: Context) {
        nativeInit(context.applicationContext)
        nativeSetDataDir(context.filesDir.absolutePath)
    }

    // ---- Session / discovery ----

    val isDiscoveryActive: Boolean
        get() = nativeIsDiscoveryActive()

    fun checkTailscaleStatus(): Result<String> = envelope(nativeCheckTailscaleStatus())

    /** Raw JSON peer list: `[{name, dnsName, ips, ...}]`. */
    fun getPeers(): Result<String> = envelope(nativeGetPeers())

    /**
     * The control link to [address]: TLS over TCP, else, with the computer's
     * cached [wanIdentity], over the internet (T10a).
     */
    fun connectToPeer(address: String, port: Int, wanIdentity: String? = null): Result<String> =
        envelope(nativeConnectToPeer(address, port, wanIdentity ?: ""))

    /** One-shot JSON snapshot of the session state machine. */
    fun sessionStatusJson(): String = nativeSessionStatus()

    /** Drain pending KDE Connect packets; each element is a JSON string. */
    fun pollIncomingPackets(): List<String> = parseStringArray(nativePollIncomingPackets())

    // ---- Clipboard ----

    /**
     * Pin the desktop certificate for [address] from a scanned pairing code,
     * so the streaming (QUIC) connection is verified from the first one on.
     * Refused when this phone already trusts a different certificate there;
     * true when the pin is new.
     */
    fun pinPeerCertificate(address: String, fingerprintHex: String): Result<Boolean> =
        envelope(nativePinPeerCertificate(address, fingerprintHex)).map { it.toBooleanStrictOrNull() == true }

    /** Forget the certificate trusted for [address] (the computer was removed). */
    fun forgetPeerCertificate(address: String): Result<Unit> =
        envelope(nativeForgetPeerCertificate(address)).map { }

    fun sendClipboard(address: String, port: Int, content: String): Result<Unit> =
        envelope(nativeSendClipboard(address, port, content)).map { }

    fun getClipboard(address: String, port: Int): Result<String> =
        envelope(nativeGetClipboard(address, port))

    /** The desktop's clipboard: its text, or the id and size of its image (N4). */
    fun getClipboardState(address: String, port: Int): Result<RemoteClip> =
        envelope(nativeGetClipboardState(address, port)).mapCatching { json ->
            val obj = JSONObject(json)
            val image = obj.optJSONObject("image")
            if (image != null) {
                RemoteClip.Image(image.getString("id"), image.getLong("size"))
            } else {
                RemoteClip.Text(obj.optString("content", ""))
            }
        }

    /** Show one of this phone's notifications on the desktop (N5a); [body] is JSON. */
    fun sendNotification(address: String, port: Int, body: String): Result<Unit> =
        envelope(nativeSendNotification(address, port, body)).map { }

    /** Send the phone's texts or a reply's result to the desktop (N5b). */
    /** [sendPhonePacket] only over the control link already up to that computer: never a connection of its own. */
    fun sendPhonePacketOnLink(address: String, port: Int, packetType: String, body: String): Result<Unit> =
        envelope(nativeSendPhonePacketOnLink(address, port, packetType, body)).map { }

    fun sendPhonePacket(address: String, port: Int, packetType: String, body: String): Result<Unit> =
        envelope(nativeSendPhonePacket(address, port, packetType, body)).map { }

    /** The computer the live control connection is to, or null. */
    fun controlPeer(): Pair<String, Int>? =
        nativeControlPeer().takeIf { it.isNotBlank() }?.let { json ->
            runCatching { JSONObject(json).let { it.getString("host") to it.getInt("port") } }.getOrNull()
        }

    /** The replies the desktop asked this phone to text (N5b). */
    fun takeSmsRequests(): List<dev.pocketdesk.android.notify.SmsRequest> =
        envelope(nativeTakeSmsRequests()).mapCatching { json ->
            val array = org.json.JSONArray(json)
            List(array.length()) { i ->
                val o = array.getJSONObject(i)
                dev.pocketdesk.android.notify.SmsRequest(
                    o.getString("id"),
                    o.getString("address"),
                    o.getString("body"),
                    o.optString("fromHost"),
                    o.optInt("fromPort"),
                )
            }
        }.getOrDefault(emptyList())

    /** Put the PNG at [path] on the desktop's clipboard (N4). */
    fun sendClipboardImage(address: String, port: Int, path: String): Result<Unit> =
        envelope(nativeSendClipboardImage(address, port, path)).map { }

    /** Download the desktop's clipboard image [id] into [path] (N4). */
    fun fetchClipboardImage(address: String, port: Int, id: String, path: String): Result<Long> =
        envelope(nativeFetchClipboardImage(address, port, id, path)).mapCatching { it.toLong() }

    // ---- Trust store ----

    fun listTrustedPeers(): List<String> = parseStringArray(nativeListTrustedPeers())

    fun forgetTrustedPeer(label: String): Boolean =
        envelope(nativeForgetTrustedPeer(label)).getOrNull()?.toBooleanStrictOrNull() == true

    // ---- Streaming ----

    val isStreamingActive: Boolean
        get() = nativeIsStreamingActive()

    /** Why the desktop closed the stream (e.g. "session refused"); null while open. */
    val streamClosedReason: String?
        get() = nativeStreamClosedReason().ifEmpty { null }

    /** This session's streaming RTT in microseconds: a median of recent polls, see [StreamingStats.rttUs]. */
    val streamingRttUs: Int
        get() = nativeGetStreamingRtt()

    /** Parsed streaming stats snapshot including the live QUIC RTT. */
    fun streamingStats(): StreamingStats {
        val obj = JSONObject(nativeGetStreamingStats())
        return StreamingStats(
            fps = obj.optDouble("fps", 0.0),
            bitrateKbps = obj.optLong("bitrate_kbps", 0L),
            rateWindowMs = obj.optLong("rate_window_ms", 0L),
            rttSamples = obj.optInt("rtt_samples", 0),
            e2eLatencyMs = obj.optLong("e2e_latency_ms", 0L),
            frameDrops = obj.optLong("frame_drops", 0L),
            linkState = obj.optString("link_state", "none"),
            rttUs = nativeGetStreamingRtt().toLong(),
        )
    }

    data class StreamingStats(
        /** Received-video rate over [rateWindowMs], not over the session. */
        val fps: Double,
        val bitrateKbps: Long,
        /**
         * How much of a link [fps] and [bitrateKbps] are divided by, in ms.
         * The bridge measures over a 3 s window, so this is short right after a
         * connect — and a figure measured over 0.6 s looks exactly like one
         * measured over 3 s unless the display says otherwise (roadmap 2056).
         * 0 when there is nothing to measure yet.
         */
        val rateWindowMs: Long,
        /**
         * How many one-second RTT polls [rttUs] is the median of. The transport
         * reports no variance, so this count is the whole of the link figure's
         * confidence: 0 after a reconnect, 8 once the history is full.
         */
        val rttSamples: Int,
        /**
         * R4 E3 compositor-true end-to-end estimate, ms: capture→send age
         * measured on the desktop clock (carried in every video packet
         * header) + one transport leg (RTT/2). Excludes the phone's own
         * decode→panel latency; 0 until the first video packet arrives.
         */
        val e2eLatencyMs: Long,
        val frameDrops: Long,
        /** "lan" | "wan_direct" | "wan_relayed" | "wan" | "none" (R4 A1). */
        val linkState: String,
        /** Median of this session's last [rttSamples] transport polls, µs. */
        val rttUs: Long,
    ) {
        val rttMs: Long get() = rttUs / 1000
        /**
         * Whether the rates cover the bridge's whole 3 s window. The threshold is
         * deliberately below it: the span is the distance between two polls on the
         * display's own ~500 ms cadence, so a jittered pair lands at 2.7 s and a
         * figure that flickers between settled and not teaches nobody anything.
         */
        val ratesSettled: Boolean get() = rateWindowMs >= 2_500
    }

    /**
     * Hand the bridge one duration this device measured, in microseconds, for
     * the kind in [SAMPLE_DECODE] / [SAMPLE_RENDER]. The streaming session
     * reports what has accumulated to the desktop once a second and the desktop
     * turns it into the session record's percentile tails — encode and RTT are
     * measured there, decode and render only ever here.
     *
     * Unlike every other call here this one does not block on the Rust runtime:
     * it pushes into a bounded batch, so the decode thread can call it per
     * frame. A negative duration is dropped; a kind this device does not own
     * (the e2e probe, taken in the Rust client) is ignored.
     */
    fun recordSample(kind: Int, micros: Long) {
        if (micros >= 0) nativeRecordSample(kind, micros)
    }

    // Wire ids shared with SAMPLE_* in core's input_packet.rs.
    const val SAMPLE_DECODE = 0
    const val SAMPLE_RENDER = 1

    /** `monitorIndex = -1` selects the server default. */
    fun connectStreaming(address: String, port: Int, monitorIndex: Int = -1): Result<Unit> =
        envelope(nativeConnectStreaming(address, port, monitorIndex, videoCodecCaps())).map { }

    /**
     * Connect over the iroh WAN path using a cached endpoint identity — the
     * `kdeconnect.pocketdesk.endpoint` body JSON announced by the desktop on
     * the control channel (see [getWanIdentity]).
     */
    fun connectStreamingWan(address: String, identityJson: String, monitorIndex: Int = -1): Result<Unit> =
        envelope(nativeConnectStreamingWan(address, identityJson, monitorIndex, videoCodecCaps())).map { }

    /** Connect the phone as a remote (T10i): input only, no picture or sound. */
    fun connectRemote(address: String, port: Int): Result<Unit> = envelope(nativeConnectRemote(address, port)).map { }

    /** [connectRemote] over the iroh WAN path, with the desktop's cached endpoint identity. */
    fun connectRemoteWan(address: String, identityJson: String): Result<Unit> =
        envelope(nativeConnectRemoteWan(address, identityJson)).map { }

    /** The remote session's state as the bridge reports it: `{"state": none | waiting | ready | tooOld}`. */
    fun remoteState(): String = nativeRemoteState()

    /** The connected desktop's cached iroh WAN identity, or null if none announced. */
    fun getWanIdentity(): String? = runCatching {
        val obj = JSONObject(nativeGetWanIdentity())
        when (val v = obj.opt("ok")) {
            null, JSONObject.NULL -> null
            else -> v.toString().takeIf { it.isNotBlank() }
        }
    }.getOrNull()

    fun reconnectStreaming(address: String, port: Int, attempt: Int, monitorIndex: Int = -1): Result<Unit> =
        envelope(nativeReconnectStreaming(address, port, monitorIndex, attempt, videoCodecCaps())).map { }

    /**
     * Codec capability bits advertised to the server at stream connect
     * (R4 C1; mirrors `CODEC_CAP_HEVC` in core client.rs). Bit 0 = the
     * device exposes a H.265/HEVC decoder via MediaCodec; H.264 is assumed
     * for every device. Cached — decoder availability cannot change mid-app.
     */
    @Volatile
    private var cachedCodecCaps = -1

    private fun videoCodecCaps(): Int {
        var caps = cachedCodecCaps
        if (caps < 0) {
            caps = runCatching {
                // Instantiate-and-release probe: API 37's android.jar removed
                // MediaCodecInfo.isDecoder(), so the codec-list scan C1 used
                // no longer compiles. createDecoderByType throws exactly when
                // no decoder for the type exists.
                val hevc = runCatching {
                    MediaCodec.createDecoderByType("video/hevc").release()
                }.isSuccess
                if (hevc) 0b1 else 0
            }.getOrDefault(0)
            cachedCodecCaps = caps
        }
        return caps
    }

    fun resetReconnectBackoff() = nativeResetReconnectBackoff()

    fun connectV2(address: String, port: Int): Result<Unit> =
        envelope(nativeConnectV2(address, port)).map { }

    fun stopStreaming(): Result<Unit> = envelope(nativeStopStreaming()).map { }

    fun stopV2(): Result<Unit> = envelope(nativeStopV2()).map { }

    /**
     * Drain encoded H.264 frames, ready for MediaCodec.
     *
     * Decodes the bridge wire format (big-endian): `u32 count`, then per frame
     * `u32 len`, `u8 flags` (bit 0 = keyframe), `u64 sequence`, `len` NAL bytes.
     */
    fun receiveFrames(timeoutMs: Int = 50): List<EncodedFrame> {
        val buf = ByteBuffer.wrap(nativeReceiveFrames(timeoutMs)).order(ByteOrder.BIG_ENDIAN)
        if (buf.remaining() < 4) return emptyList()
        val count = buf.int
        val frames = ArrayList<EncodedFrame>(count.coerceAtMost(16))
        repeat(count) {
            // Per frame: u32 len + u8 flags + u64 sequence (13 bytes), then len.
            if (buf.remaining() < 13) return frames
            val len = buf.int
            if (len < 0 || buf.remaining() < 9 + len) return frames
            val flags = buf.get()
            val sequence = buf.long
            val data = ByteArray(len)
            buf.get(data)
            frames += EncodedFrame(data, flags.toInt() and 1 != 0, sequence)
        }
        return frames
    }

    /** Drain the desktop's Opus packets (20 ms @ 48 kHz stereo each), with sequences. */
    fun receiveAudio(timeoutMs: Int = 20): List<AudioChunk> =
        AudioPayload.parse(nativeReceiveAudio(timeoutMs))

    // ---- Input ----

    fun sendMouseEvent(
        address: String,
        port: Int,
        x: Float,
        y: Float,
        button: Int,
        isPressed: Boolean,
    ): Result<Unit> = envelope(nativeSendMouseEvent(address, port, x, y, button, isPressed)).map { }

    /**
     * Normalized absolute pointer position for direct-touch input. Both axes
     * are 0..=65535, independent of display resolution; compute with
     * [normalizedCoord]. Requires an active QUIC streaming connection.
     */
    fun sendMouseAbs(xNorm: Int, yNorm: Int): Result<Unit> =
        envelope(nativeSendMouseAbs(xNorm, yNorm)).map { }

    /**
     * Mouse button press/release. [button] is the wire encoding:
     * 0=Left, 1=Middle, 2=Right, 3=Back, 4=Forward. Unlike [sendMouseEvent]
     * this can address the left button (its `button=0` means "movement").
     * In direct-touch mode a left release lifts the virtual finger.
     */
    fun sendMouseClick(button: Int, isPressed: Boolean): Result<Unit> =
        envelope(nativeSendMouseClick(button, isPressed)).map { }

    /** Map a view-local pixel offset to the 0..=65535 normalized axis range. */
    fun normalizedCoord(offset: Float, size: Int): Int =
        (offset / size.coerceAtLeast(1) * NORM_COORD_MAX).roundToInt().coerceIn(0, NORM_COORD_MAX)

    /**
     * Direct-touch tap: warp the pointer to the normalized point, then press
     * and release the left button. Absolute motion alone moves the cursor and
     * puts nothing down — the server's pointer device reports the button
     * separately from the axes — so a tap that skips the press is invisible to
     * anything that acts on button-down.
     */
    fun tapAbsolute(xNorm: Int, yNorm: Int): Result<Unit> {
        val moved = sendMouseAbs(xNorm, yNorm)
        if (moved.isFailure) return moved
        val down = sendMouseClick(0, true)
        if (down.isFailure) return down
        return sendMouseClick(0, false)
    }

    fun sendKeyboardEvent(address: String, port: Int, keyCode: Int, text: String): Result<Unit> =
        envelope(nativeSendKeyboardEvent(address, port, keyCode, text)).map { }

    /**
     * Tap a key (server sends press+release, one round trip). [keyCode] is an
     * `android.view.KeyEvent.KEYCODE_*`; the bridge maps it to evdev.
     */
    fun tapKey(address: String, port: Int, keyCode: Int): Result<Unit> =
        sendKeyboardEvent(address, port, keyCode, "")

    /** Hold a modifier down / release it (combo building on the server side). */
    fun holdKey(address: String, port: Int, keyCode: Int): Result<Unit> =
        sendKeyboardEvent(address, port, keyCode + MOD_PRESS_OFFSET, "")

    fun releaseKey(address: String, port: Int, keyCode: Int): Result<Unit> =
        sendKeyboardEvent(address, port, keyCode + MOD_RELEASE_OFFSET, "")

    /**
     * Send `modifiers + key` as a combo: press modifiers in order, tap the
     * key, release modifiers in reverse. Modifiers use `KEYCODE_*` constants.
     */
    fun hotkey(address: String, port: Int, key: Int, vararg modifiers: Int): Result<Unit> {
        for ((index, mod) in modifiers.withIndex()) {
            holdKey(address, port, mod).onFailure { failure ->
                // Let go of what is already held, or it stays down on the desktop.
                for (held in modifiers.take(index).reversed()) releaseKey(address, port, held)
                return Result.failure(failure)
            }
        }
        val tapped = tapKey(address, port, key)
        for (mod in modifiers.reversed()) {
            releaseKey(address, port, mod)
        }
        return tapped
    }

    /** IME/text input path (server injects as Unicode text). */
    fun sendText(address: String, port: Int, text: String): Result<Unit> =
        sendKeyboardEvent(address, port, 0, text)

    fun sendGamepadEvent(axes: IntArray, buttons: Int): Result<Unit> =
        envelope(nativeSendGamepadEvent(axes, buttons)).map { }

    // ---- Files / device control ----

    fun sendFile(address: String, port: Int, filePath: String): Result<Unit> =
        envelope(nativeSendFile(address, port, filePath)).map { }

    /** Send [size] bytes from [fd], which the bridge takes over and closes (F7). */
    /**
     * Send [size] bytes from [fd] as [name]; [source] names the file so a
     * retry continues it (T6a). With a [part], it is one file of a folder
     * send and lands at its path under that folder (T6b).
     */
    fun sendFileFd(
        address: String,
        port: Int,
        fd: Int,
        name: String,
        size: Long,
        source: String,
        part: FolderPart? = null,
    ): Result<Unit> =
        envelope(
            nativeSendFileFd(
                address,
                port,
                fd,
                name,
                size,
                source,
                part?.folderId.orEmpty(),
                part?.folder.orEmpty(),
                org.json.JSONArray(part?.path.orEmpty()).toString(),
            ),
        ).map { }

    /** Where a file sits in a folder the phone sends (T6b). */
    data class FolderPart(val folderId: String, val folder: String, val path: List<String>)

    /** The running file transfer as (bytes sent, total). */
    fun transferProgress(): Pair<Long, Long> {
        val values = nativeTransferProgress() ?: return 0L to 0L
        return if (values.size == 2) values[0] to values[1] else 0L to 0L
    }

    /**
     * Download from the desktop into [fd], which the bridge takes over and
     * closes (F7b): the browsed path [value], or the offer id [value] when
     * [isOffer]. The bytes saved.
     */
    /**
     * Download into [fd] (opened read-write). With a [source], a broken
     * download keeps its part at [location], the place [fd] opens, and a
     * retry continues it (T6a).
     */
    fun fetchFile(
        address: String,
        port: Int,
        isOffer: Boolean,
        value: String,
        fd: Int,
        source: String,
        name: String,
        location: String,
    ): Result<Long> =
        envelope(nativeFetchFile(address, port, isOffer, value, fd, source, name, location))
            .mapCatching { it.toLong() }

    /** Where an earlier download of [address]'s file [source] kept its part, if it did. */
    fun partialLocation(address: String, source: String): String? =
        nativePartialLocation(address, source).ifEmpty { null }

    /** Forget unfinished downloads older than [maxAgeHours]; where their bytes are, to delete. */
    fun sweepPartials(maxAgeHours: Int): List<String> =
        runCatching {
            val list = org.json.JSONArray(nativeSweepPartials(maxAgeHours))
            (0 until list.length()).map { list.getString(it) }
        }.getOrDefault(emptyList())

    /** Tell the desktop this phone will not fetch offer [id], and why (F7b). */
    fun declineOffer(address: String, port: Int, id: String, reason: String): Result<Unit> =
        envelope(nativeDeclineOffer(address, port, id, reason)).map { }

    /**
     * Ask the desktop for the whole folder at [path] (as the remote browser
     * names it); its files then arrive as offers in this session (T6b).
     * Fails with the desktop's reason when it will not send it.
     */
    fun fetchFolder(address: String, port: Int, path: String): Result<Unit> =
        envelope(nativeFetchFolder(address, port, path)).map { }

    /** Video bytes the live session has received, counted as frames arrive (0 with none). */
    fun videoBytes(): Long = nativeVideoBytes()

    /** Files the desktop offered since the last call, as a JSON array of `{id, name, size}`. */
    fun takeFileOffers(): String = envelope(nativeTakeFileOffers()).getOrDefault("[]")

    /** Hand [url] to the computer, which opens it in its browser (T8a). */
    fun sendLink(address: String, port: Int, url: String): Result<Unit> =
        envelope(nativeSendLink(address, port, url)).map { }

    /** The computer's presses for the phone's player since the last call, oldest first (T8d). */
    fun takeMediaPresses(): List<String> =
        parseStringArray(envelope(nativeTakeMediaPresses()).getOrDefault("[]"))

    /** The computer's clipboard text as it last arrived (T10b), taken; null when nothing came. */
    fun takeClipboardPush(): ClipboardPushed? =
        envelope(nativeTakeClipboardPush()).getOrNull()?.takeIf { it.isNotBlank() }?.let { json ->
            runCatching {
                JSONObject(json).let { ClipboardPushed(it.getString("host"), it.getInt("port"), it.getString("content")) }
            }.getOrNull()
        }

    /** The computer's requests to browse this phone's shared folders, as a JSON array (T10g). */
    fun takePhoneFileRequests(): String = envelope(nativeTakePhoneFileRequests()).getOrDefault("[]")

    /** The computer's answers to this phone's notifications since the last call (T10c). */
    fun takeNotificationAnswers(): List<NotificationAnswer> =
        runCatching {
            val array = org.json.JSONArray(envelope(nativeTakeNotificationAnswers()).getOrDefault("[]"))
            (0 until array.length()).map { i ->
                val a = array.getJSONObject(i)
                NotificationAnswer(
                    a.getString("host"),
                    a.getInt("port"),
                    a.getString("key"),
                    a.optString("action").takeIf { a.has("action") },
                    a.optString("message").takeIf { a.has("message") },
                )
            }
        }.getOrDefault(emptyList())

    /** One answer from the computer at [host]:[port] to notification [key]: a button, or a reply. */
    data class NotificationAnswer(val host: String, val port: Int, val key: String, val action: String?, val message: String?)

    /** One `kdeconnect.clipboard` text from the computer at [host]:[port]. */
    data class ClipboardPushed(val host: String, val port: Int, val content: String)

    /** The links the computer handed over since the last call, oldest first (T8a). */
    fun takeLinks(): List<String> = parseStringArray(envelope(nativeTakeLinks()).getOrDefault("[]"))

    fun listRemoteFiles(address: String, port: Int, remotePath: String): Result<String> =
        envelope(nativeListRemoteFiles(address, port, remotePath))

    /** Ask the computer for a screen of this phone's shape (T10j): its answer as JSON, see [dev.pocketdesk.android.ui.PhoneScreen]. */
    fun addScreen(address: String, port: Int, width: Int, height: Int): Result<String> =
        envelope(nativeVirtualScreen(address, port, "add", width, height))

    /** Take this phone's screen away again (T10j). */
    fun removeScreen(address: String, port: Int): Result<String> =
        envelope(nativeVirtualScreen(address, port, "remove", 0, 0))

    fun getMonitors(address: String, port: Int): Result<String> =
        envelope(nativeGetMonitors(address, port))

    /** Desktop battery as `{currentCharge, isCharging}` or `{noBattery}` (Tier-2 #11). */
    fun getBattery(address: String, port: Int): Result<String> =
        envelope(nativeGetBattery(address, port))

    fun getMonitorCount(address: String, port: Int): Result<String> =
        envelope(nativeGetMonitorCount(address, port))

    /**
     * Desktop privacy mode (Tier-3 #15). [action]: "grab" blocks the desktop's
     * physical keyboard+mouse (and re-arms the auto-release TTL), "release"
     * gives them back, "status" queries. [lock] additionally engages the
     * desktop screen locker. Returns the plugin reply JSON.
     */
    fun desktopPrivacy(
        address: String,
        port: Int,
        action: String,
        lock: Boolean = false,
    ): Result<String> = envelope(nativeDesktopPrivacy(address, port, action, lock))

    /** Convenience: give the desktop's local keyboard+mouse back (Tier-3 #15). */
    fun releaseDesktopPrivacy(address: String, port: Int): Result<String> =
        desktopPrivacy(address, port, "release")

    /**
     * Desktop audio control (Tier-3 #16). [bodyJson] is the
     * `kdeconnect.pocketdesk.audio` request, e.g. `{"action":"status"}`,
     * `{"action":"setVolume","volume":45}`, `{"action":"setMuted","muted":true}`,
     * `{"action":"sinks"}`, `{"action":"selectSink","name":"..."}`.
     * Returns the plugin reply JSON.
     */
    fun audioControl(
        address: String,
        port: Int,
        bodyJson: String,
    ): Result<String> = envelope(nativeAudioControl(address, port, bodyJson))

    /**
     * Consume the find-my-device siren latch (Tier-2 #11). Returns true once
     * per `kdeconnect.findmydevice` `{ring:true}` pushed by the desktop.
     */
    fun checkSiren(): Boolean =
        envelope(nativeCheckSiren()).getOrNull()?.toBooleanStrictOrNull() == true

    /** Make the remote desktop ring (find-my-device siren, phone → desktop). */
    fun sendFindMyDevice(address: String, port: Int): Result<Unit> =
        envelope(nativeSendFindMyDevice(address, port)).map { }

    /**
     * The Connection Doctor's identity check (T1): who answers at the
     * control port, compared with this phone's pin for the address, as
     * "matches", "changed", "not_pinned", "not_pocketdesk" or "failed". It
     * never saves a pin.
     */
    fun checkIdentity(address: String, port: Int): Result<String> =
        envelope(nativeCheckIdentity(address, port))

    // ---- PIN pairing (Tier-2 #11b) ----

    /** Ask the desktop to display a pairing PIN: "pinSent" (read it off the
     *  desktop) or "pinReady" (a PIN is up from `pocketdesk pair`). */
    fun requestPairPin(address: String, port: Int): Result<String> =
        envelope(nativeRequestPairPin(address, port))

    /** Submit an externally-entered PIN; non-null = the desktop's deviceId once paired. */
    fun pairWithPin(address: String, port: Int, pin: String): Result<String?> =
        envelope(nativePairWithPin(address, port, pin)).map {
            it.takeIf { s -> s.isNotBlank() && s != "null" }
        }

    /**
     * Wait up to [waitSecs] for a pairing decision pushed by the desktop on
     * the control channel. Returns the desktop's deviceId once paired, null
     * while still waiting.
     */
    fun checkPairResult(waitSecs: Long = 0): String? = runCatching {
        val obj = JSONObject(nativeCheckPairResult(waitSecs))
        when (val v = obj.opt("ok")) {
            null, JSONObject.NULL -> null
            else -> v.toString().takeIf { it.isNotBlank() }
        }
    }.getOrNull()

    /** Device ids of desktops this phone has paired with (bridge-persisted). */
    fun pairedServers(): List<String> = runCatching {
        val arr = org.json.JSONArray(envelope(nativePairedServers()).getOrThrow())
        List(arr.length()) { arr.getString(it) }
    }.getOrDefault(emptyList())

    /**
     * Drain desktop notifications queued by the bridge control reader
     * (Tier-2 #11c). Each has a stable `id` used for reply correlation and
     * dedup; the queue is emptied on return.
     */
    fun takePendingNotifications(): List<DesktopNotification> = runCatching {
        val arr = org.json.JSONArray(envelope(nativeTakePendingNotifications()).getOrThrow())
        List(arr.length()) { i ->
            val o = arr.getJSONObject(i)
            DesktopNotification(
                id = o.optString("id"),
                app = o.optString("app"),
                title = o.optString("title"),
                text = o.optString("text"),
                source = o.optString("source"),
            )
        }
    }.getOrDefault(emptyList())

    /** Reply to a desktop notification from the phone (Tier-2 #11c). */
    fun sendNotificationReply(
        address: String,
        port: Int,
        id: String,
        text: String,
    ): Result<Unit> = envelope(nativeSendNotificationReply(address, port, id, text)).map { }

    /**
     * Hyprland window list (R3#7 picker). Payload JSON:
     * `[ [ {address,title,class,at:[x,y],local_at:[x,y],size:[w,h],monitor_size:[w,h],monitor,fullscreen,workspace{id,name},active}, … ], activeAddress, screenBox ]`
     * where `screenBox` is the monitor layout `[x,y,w,h]` in desktop coords
     * (or null on older servers). `local_at` is the crop-rect origin in the
     * monitor's own coordinate space; `at` stays global for input remapping.
     * Errors when the server has no Hyprland IPC.
     */
    fun getWindows(address: String, port: Int): Result<String> =
        envelope(nativeGetWindows(address, port))

    /**
     * Restrict the server's capture to one window (single-window streaming):
     * use a window's `local_at` + `size`, and pass its Hyprland `address` so
     * a Hyprland server can capture the window itself (occlusion-correct, no
     * wasted bandwidth) instead of cropping the desktop in software. The
     * rect always travels — it is what other servers use. The video stream
     * is re-encoded at the captured resolution. Zero width/height clears the
     * crop and restores the full desktop. QUIC-only.
     */
    fun sendWindowCrop(
        x: Int,
        y: Int,
        width: Int,
        height: Int,
        address: String = "",
    ): Result<Unit> = envelope(nativeSendWindowCrop(x, y, width, height, address)).map { }

    fun clearWindowCrop(): Result<Unit> = sendWindowCrop(0, 0, 0, 0)

    /**
     * R4 D1 view-only mode: while enabled the **server** drops every input
     * packet from this session (mouse, keyboard, gamepad, text) — video and
     * control-plane packets keep flowing. Server-side enforcement means a
     * forgotten tap can't leak through a stale client queue. Per-session: a
     * reconnect starts interactive again, so callers re-send on reconnect if
     * the toggle is still on. QUIC-only.
     */
    fun setViewOnly(enabled: Boolean): Result<Unit> =
        envelope(nativeSetViewOnly(enabled)).map { }

    /**
     * R4 A3: opt out of the server's relayed-path bitrate floor. While the
     * video link rides an iroh relay the server clamps the encoder bitrate
     * to a bandwidth-friendly cap; enabling this restores the full
     * configured bitrate on the relay (user accepts the congestion risk).
     * Server-enforced latch, per-session; a no-op on LAN/direct paths.
     */
    fun setFullQuality(enabled: Boolean): Result<Unit> =
        envelope(nativeSetFullQuality(enabled)).map { }

    /**
     * R4 E5: choose a named link-profile preset from the HUD. The server
     * folds it together with the A3 relay floor into the live encoder
     * bitrate (a ceiling relative to the session's configured rate, so the
     * native resolution is never dropped — only bandwidth). Use [PRESET_*].
     * Control-plane: survives view-only, never injected. QUIC-only.
     */
    fun sendQualityPreset(preset: Int): Result<Unit> =
        envelope(nativeSendQualityPreset(preset)).map { }

    // Wire ids shared with InputPacket::QualityPreset in the Rust core.
    const val PRESET_AUTO = 0
    const val PRESET_QUALITY = 1
    const val PRESET_BALANCED = 2
    const val PRESET_ECONOMY = 3

    /**
     * R4 E2 reverse audio (phone mic → desktop): open the mic channel — the
     * server creates a "Pocketdesk Mic" PipeWire source apps can select.
     * Then feed encoded frames with [sendMicOpus] (48 kHz mono, 20 ms Opus
     * packets from MediaCodec) and close with [stopMic]. QUIC-only: needs a
     * live streaming session.
     */
    fun startMic(): Result<Unit> = envelope(nativeStartMic()).map { }

    /** Push one encoded Opus frame to the desktop mic relay. */
    fun sendMicOpus(opus: ByteArray): Result<Unit> =
        envelope(nativeSendMicOpus(opus)).map { }

    /** Close the mic channel; the server removes its virtual source. */
    fun stopMic(): Result<Unit> = envelope(nativeStopMic()).map { }

    fun sendPowerCommand(address: String, port: Int, action: String): Result<Unit> =
        envelope(nativeSendPowerCommand(address, port, action)).map { }

    fun executeRemoteCommand(address: String, port: Int, command: String): Result<String> =
        envelope(nativeExecuteRemoteCommand(address, port, command))

    /**
     * Wake a computer from this phone (N6): the magic packet for [mac] to
     * each `host:port` in [targets], three times each. Returns how many
     * addresses took it.
     */
    fun wake(mac: String, targets: List<String>): Result<Int> =
        envelope(nativeWake(mac, org.json.JSONArray(targets).toString())).mapCatching { it.toInt() }

    /**
     * Whether another phone drives the computer at [address]:[port] (N7),
     * as it last said; false when it has not said.
     */
    fun anotherPhoneDrives(address: String, port: Int): Boolean =
        nativeControlState(address, port).takeIf { it.isNotBlank() }?.let { json ->
            runCatching { JSONObject(json).optBoolean("other") }.getOrNull()
        } ?: false

    /**
     * How the computer at [address]:[port] said it can be woken
     * (`wake_info` JSON), or null: never another computer's.
     */
    fun wakeInfo(address: String, port: Int): String? = nativeWakeInfo(address, port).takeIf { it.isNotBlank() }

    /**
     * Wake a sleeping desktop through an always-on LAN relay (Tier-2 #12):
     * connect to [address] (the relay peer) and ask it to emit the WoL magic
     * packet for [mac] on its LAN. [broadcast] empty = relay default
     * (255.255.255.255); pass the directed subnet broadcast (e.g.
     * 192.168.1.255) for reliability. Fire-and-forget — the packet leaves,
     * whether the machine powers on is not confirmable.
     */
    fun wakeViaRelay(
        address: String,
        port: Int,
        mac: String,
        broadcast: String = "",
    ): Result<Unit> = envelope(nativeWakeViaRelay(address, port, mac, broadcast)).map { }

    // ---- internals ----

    // Modifier encoding understood by the bridge's send_keyboard_event:
    // keyCode + 50000 = modifier press, keyCode + 100000 = modifier release.
    private const val MOD_PRESS_OFFSET = 50_000
    private const val MOD_RELEASE_OFFSET = 100_000

    // Upper bound of the normalized absolute-axis range on the wire.
    private const val NORM_COORD_MAX = 65535

    /** One Annex-B H.264 access unit plus its keyframe flag and sequence number. */
    data class EncodedFrame(
        val data: ByteArray,
        val isKeyframe: Boolean,
        val sequence: Long,
    ) {
        override fun equals(other: Any?): Boolean =
            this === other ||
                (other is EncodedFrame &&
                    isKeyframe == other.isKeyframe &&
                    sequence == other.sequence &&
                    data.contentEquals(other.data))

        override fun hashCode(): Int =
            (data.contentHashCode() * 31 + isKeyframe.hashCode()) * 31 + sequence.hashCode()
    }

    private fun envelope(json: String): Result<String> = runCatching {
        val obj = JSONObject(json)
        when {
            obj.has("error") -> error(obj.getString("error"))
            else -> obj.opt("ok")?.toString() ?: ""
        }
    }

    private fun parseStringArray(json: String): List<String> = runCatching {
        val arr = org.json.JSONArray(json)
        List(arr.length()) { arr.getString(it) }
    }.getOrDefault(emptyList())
}

/** What the desktop's clipboard holds, as its reply says (N4). */
sealed interface RemoteClip {
    data class Text(val content: String) : RemoteClip

    /** An image the phone may fetch: [id] is the hex SHA-256 of its PNG. */
    data class Image(val id: String, val size: Long) : RemoteClip
}

/** One desktop notification pushed over the control channel (Tier-2 #11c). */
data class DesktopNotification(
    val id: String,
    val app: String,
    val title: String,
    val text: String,
    val source: String,
)
