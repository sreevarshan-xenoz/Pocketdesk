package dev.linuxlink.android.stream

import android.os.Build
import android.view.SurfaceView
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.layout
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.viewinterop.AndroidView
import dev.linuxlink.android.HostStore
import dev.linuxlink.android.bridge.RustCore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlin.math.hypot
import kotlin.math.roundToInt

/** Normalized absolute-input axis range shared with the wire protocol. */
private const val NORM_RANGE = 65535

/**
 * How touch on the video surface is translated into remote input.
 * [DirectTouch] maps the screen 1:1 to the desktop (tap where you see);
 * [Trackpad] is indirect control: drag moves the cursor, tap clicks.
 */
enum class InputMode {
    DirectTouch,
    Trackpad,
}

/**
 * Where the video frame sits in desktop space, for crop-accurate direct
 * touch. The server normalizes absolute input across the whole monitor
 * layout (`screen*`), but a window-cropped video covers only part of it:
 * `desktop*` is the desktop rect the video shows (the selected window's
 * global geometry), and the video's own encoded size supplies the scale.
 * Built from the windows payload in [dev.linuxlink.android.ui.WindowPickerSheet].
 */
data class DesktopMapping(
    val desktopX: Int,
    val desktopY: Int,
    val desktopW: Int,
    val desktopH: Int,
    val screenX: Int,
    val screenY: Int,
    val screenW: Int,
    val screenH: Int,
)

/** Letterboxed placement of the video inside the composable container (px). */
private data class VideoLayout(
    val offsetX: Float,
    val offsetY: Float,
    val displayW: Float,
    val displayH: Float,
    val videoW: Int,
    val videoH: Int,
)

/**
 * Pinch-zoom transform over the video surface (Tier 1 #6). The whole desktop
 * fits the viewport at scale 1; zooming in magnifies HiDPI text and
 * [ViewportZoom.toContent] maps viewport touch points back to desktop space so
 * direct-touch still hits what the user sees.
 *
 * Rendering is a graphicsLayer on the SurfaceView, which composes correctly
 * on Android 10+ (API 29); on older devices the surface may not follow the
 * transform cleanly.
 */
private class ViewportZoom {
    var scale by mutableFloatStateOf(1f)
        private set
    var offsetX by mutableFloatStateOf(0f)
        private set
    var offsetY by mutableFloatStateOf(0f)
        private set

    val isZoomed: Boolean get() = scale > 1.01f

    fun applyGesture(zoomDelta: Float, pan: Offset, containerW: Float, containerH: Float) {
        val newScale = (scale * zoomDelta).coerceIn(1f, 8f)
        offsetX = (offsetX + pan.x).coerceIn(containerW * (1f - newScale), 0f)
        offsetY = (offsetY + pan.y).coerceIn(containerH * (1f - newScale), 0f)
        scale = newScale
    }

    fun reset() {
        scale = 1f
        offsetX = 0f
        offsetY = 0f
    }

    /** Viewport point -> desktop point in scale-1 content space. */
    fun toContent(pos: Offset): Offset =
        Offset((pos.x - offsetX) / scale, (pos.y - offsetY) / scale)
}

/**
 * Remote desktop video surface: connects a streaming session on attach,
 * decodes with [H264Decoder] straight into the [SurfaceView], and tears the
 * session down on dispose. A transparent overlay turns touches into input
 * packets according to [inputMode].
 *
 * Renders via the SurfaceView overlay path (never ImageReader) so decoded
 * frames go to the display without a GPU copy. On API 31+ the surface
 * declares the panel's native refresh rate so frame presentation aligns with
 * vsync instead of the default 60 Hz voting.
 */
@Composable
fun RemoteDesktopView(
    address: String,
    port: Int,
    width: Int,
    height: Int,
    inputMode: InputMode = InputMode.DirectTouch,
    mapping: DesktopMapping? = null,
    monitorIndex: Int = -1,
    onStatus: (StreamStatus) -> Unit = {},
    onVideoSizeChanged: (Int, Int) -> Unit = { _, _ -> },
    /** R4 C2: fires when the server's encoder-fallback ladder changes the
     * wire codec mid-session ("H.264" / "H.265"). */
    onCodecChanged: (String) -> Unit = {},
    /** Mark the video surface secure (Tier-3 #15 blackout mode) so decoded
     * frames never appear in screenshots, recordings, or Recents. */
    secure: Boolean = false,
    modifier: Modifier = Modifier,
) {
    val decoderHost = remember { DecoderHost() }
    val zoom = remember { ViewportZoom() }
    // Real encoded frame size, reported by the decoder (the initial width/
    // height are only a guess until the first SPS).
    var videoSize by remember { mutableStateOf<IntSize?>(null) }
    // Pinch-to-zoom only in DirectTouch: Trackpad already owns two-finger
    // gestures for remote scrolling.
    val zoomGesturesEnabled = inputMode == InputMode.DirectTouch

    BoxWithConstraints(modifier) {
        val containerW = constraints.maxWidth.toFloat()
        val containerH = constraints.maxHeight.toFloat()
        val videoLayout =
            remember(containerW, containerH, videoSize) {
                val vw = videoSize?.width ?: containerW.toInt()
                val vh = videoSize?.height ?: containerH.toInt()
                if (vw <= 0 || vh <= 0 || containerW <= 0f || containerH <= 0f) {
                    VideoLayout(0f, 0f, containerW, containerH, containerW.toInt().coerceAtLeast(1), containerH.toInt().coerceAtLeast(1))
                } else {
                    val fit = minOf(containerW / vw, containerH / vh)
                    val dw = vw * fit
                    val dh = vh * fit
                    VideoLayout((containerW - dw) / 2f, (containerH - dh) / 2f, dw, dh, vw, vh)
                }
            }
        Box(
            modifier =
                Modifier
                    .layout { measurable, constraints ->
                        val w = videoLayout.displayW.roundToInt().coerceAtLeast(1)
                        val h = videoLayout.displayH.roundToInt().coerceAtLeast(1)
                        val placeable = measurable.measure(Constraints.fixed(w, h))
                        layout(constraints.maxWidth, constraints.maxHeight) {
                            placeable.place(
                                videoLayout.offsetX.roundToInt(),
                                videoLayout.offsetY.roundToInt(),
                            )
                        }
                    }
                    .graphicsLayer {
                        scaleX = zoom.scale
                        scaleY = zoom.scale
                        translationX = zoom.offsetX
                        translationY = zoom.offsetY
                    },
        ) {
            AndroidView(
                factory = { context ->
                    SurfaceView(context).apply {
                        holder.addCallback(
                            object : android.view.SurfaceHolder.Callback {
                                override fun surfaceCreated(holder: android.view.SurfaceHolder) {
                                    declareFrameRate(holder, display)
                                    decoderHost.start(
                                        context,
                                        holder.surface,
                                        address,
                                        port,
                                        width,
                                        height,
                                        HostStore.wanIdentity(context, address),
                                        monitorIndex,
                                        onStatus,
                                        onCodecChanged,
                                    ) { w, h ->
                                        videoSize = IntSize(w, h)
                                        onVideoSizeChanged(w, h)
                                    }
                                }

                                override fun surfaceChanged(
                                    holder: android.view.SurfaceHolder,
                                    format: Int,
                                    width: Int,
                                    height: Int,
                                ) {
                                }

                                override fun surfaceDestroyed(holder: android.view.SurfaceHolder) {
                                    decoderHost.stop()
                                }
                            },
                        )
                    }
                },
                update = { view -> (view as SurfaceView).setSecure(secure) },
                modifier = Modifier.matchParentSize(),
            )
        }

        InputOverlay(
            address = address,
            port = port,
            mode = inputMode,
            zoom = zoom,
            layout = videoLayout,
            mapping = mapping,
            modifier = Modifier
                .matchParentSize()
                .pointerInput(zoomGesturesEnabled) {
                    if (zoomGesturesEnabled) zoomGestureDetector(zoom)
                },
        )
    }

    DisposableEffect(Unit) {
        onDispose { decoderHost.stop() }
    }
}

/**
 * Two-finger pinch + pan. Ignores single-pointer events (those stay input
 * passthrough) and consumes everything while two or more fingers are down.
 */
private suspend fun androidx.compose.ui.input.pointer.PointerInputScope.zoomGestureDetector(
    zoom: ViewportZoom,
) {
    awaitEachGesture {
        var prevCentroid: Offset? = null
        var prevSpan = 0f
        while (true) {
            val event = awaitPointerEvent()
            val pressed = event.changes.filter { it.pressed }
            if (pressed.size >= 2) {
                val centroid = pressed.map { it.position }
                    .reduce { a, b -> Offset((a.x + b.x) / 2f, (a.y + b.y) / 2f) }
                val dx = pressed[0].position.x - pressed[1].position.x
                val dy = pressed[0].position.y - pressed[1].position.y
                val span = hypot(dx, dy)
                val pc = prevCentroid
                if (pc != null && prevSpan > 0f && span > 0f) {
                    zoom.applyGesture(
                        zoomDelta = span / prevSpan,
                        pan = centroid - pc,
                        containerW = size.width.toFloat(),
                        containerH = size.height.toFloat(),
                    )
                }
                prevCentroid = centroid
                prevSpan = span
                pressed.forEach { it.consume() }
            } else if (pressed.isEmpty()) {
                break
            } else {
                // One finger left on the surface: back to passthrough.
                prevCentroid = null
                prevSpan = 0f
            }
        }
    }
}

/**
 * Full-surface gesture catcher. All sends go through a single-perm
 * dispatcher so packets leave in gesture order (down -> move -> up).
 *
 * One-finger gestures drive remote input; a second finger cancels the
 * direct-touch action (it belongs to a pinch-zoom gesture) before anything
 * is sent. Touch positions are mapped through the current [ViewportZoom] so
 * a tap hits the desktop point the user actually sees.
 */
@Composable
private fun InputOverlay(
    address: String,
    port: Int,
    mode: InputMode,
    zoom: ViewportZoom,
    layout: VideoLayout,
    mapping: DesktopMapping?,
    modifier: Modifier = Modifier,
) {
    val scope = rememberCoroutineScope()
    val ordered = remember { Dispatchers.IO.limitedParallelism(1) }

    fun send(block: () -> Unit) = scope.launch(ordered) { block() }

    Box(
        modifier.pointerInput(address to port to mode to layout to mapping) {
            fun mappedPoint(pos: Offset): Pair<Int, Int> {
                // Viewport -> scale-1 content space -> letterboxed video
                // pixel -> desktop coordinate -> normalized axis value.
                // Without the video-pixel step a touch outside the fitted
                // image (or with a crop active) hit the wrong desktop point.
                val content = zoom.toContent(pos)
                val vx = (content.x - layout.offsetX) / layout.displayW.coerceAtLeast(1f) * layout.videoW
                val vy = (content.y - layout.offsetY) / layout.displayH.coerceAtLeast(1f) * layout.videoH
                val xNorm: Int
                val yNorm: Int
                val m = mapping
                if (m != null && m.screenW > 0 && m.screenH > 0 && layout.videoW > 0 && layout.videoH > 0) {
                    val sx = m.desktopW.toFloat() / layout.videoW
                    val sy = m.desktopH.toFloat() / layout.videoH
                    val dx = m.desktopX + vx * sx - m.screenX
                    val dy = m.desktopY + vy * sy - m.screenY
                    xNorm = (dx / m.screenW * NORM_RANGE).roundToInt().coerceIn(0, NORM_RANGE)
                    yNorm = (dy / m.screenH * NORM_RANGE).roundToInt().coerceIn(0, NORM_RANGE)
                } else {
                    xNorm = RustCore.normalizedCoord(vx.coerceIn(0f, layout.videoW.toFloat()), layout.videoW)
                    yNorm = RustCore.normalizedCoord(vy.coerceIn(0f, layout.videoH.toFloat()), layout.videoH)
                }
                return xNorm to yNorm
            }

            fun sendAbs(pos: Offset) {
                val (xNorm, yNorm) = mappedPoint(pos)
                send { RustCore.sendMouseAbs(xNorm, yNorm) }
            }

            awaitEachGesture {
                val down = awaitFirstDown(requireUnconsumed = false)
                when (mode) {
                    InputMode.DirectTouch -> {
                        // Defer the first packet until we know this is a
                        // one-finger gesture, not the start of a pinch.
                        var cancelled = false
                        var buttonDown = false
                        var lastPos = down.position
                        var done = false
                        while (!done) {
                            val event = awaitPointerEvent()
                            val held = event.changes.filter { it.pressed }
                            when {
                                cancelled -> {
                                    if (held.isEmpty()) {
                                        // A pinch that started as a drag has already
                                        // put the button down; leaving it pressed is a
                                        // stuck drag on the desktop.
                                        if (buttonDown) send { RustCore.sendMouseClick(0, false) }
                                        done = true
                                    }
                                }

                                held.size >= 2 -> {
                                    cancelled = true
                                }

                                held.isEmpty() -> {
                                    val (xNorm, yNorm) = mappedPoint(lastPos)
                                    if (buttonDown) {
                                        send {
                                            RustCore.sendMouseAbs(xNorm, yNorm)
                                            RustCore.sendMouseClick(0, false)
                                        }
                                    } else {
                                        // Never moved far enough to be a drag: a tap is
                                        // warp, press, release at one point.
                                        send { RustCore.tapAbsolute(xNorm, yNorm) }
                                    }
                                    done = true
                                }

                                else -> {
                                    lastPos = held.first().position
                                    if (!buttonDown) {
                                        buttonDown = true
                                        // Warp before pressing: a drag belongs to the
                                        // touched point, not wherever the cursor was.
                                        sendAbs(lastPos)
                                        send { RustCore.sendMouseClick(0, true) }
                                    } else {
                                        sendAbs(lastPos)
                                    }
                                }
                            }
                        }
                    }

                    InputMode.Trackpad -> {
                        var last = down.position
                        var dragged = 0f
                        var pending = Offset.Zero
                        var twoFinger = false
                        var done = false
                        while (!done) {
                            val event = awaitPointerEvent()
                            if (event.changes.size >= 2) twoFinger = true
                            val change = event.changes.firstOrNull { it.pressed }
                            if (change == null) {
                                if (!twoFinger && dragged <= viewConfiguration.touchSlop) {
                                    send {
                                        RustCore.sendMouseClick(0, true)
                                        RustCore.sendMouseClick(0, false)
                                    }
                                }
                                done = true
                                continue
                            }
                            val pos = change.position
                            // Divide by zoom so cursor travel matches the
                            // on-screen distance the finger covered, then
                            // convert view px to video px (letterbox fit may
                            // scale them apart; relative motion is unaffected
                            // by the letterbox offset itself).
                            val travel =
                                if (layout.displayW > 0f) layout.videoW / layout.displayW else 1f
                            val delta = (pos - last) / zoom.scale
                            last = pos
                            dragged += delta.getDistance()
                            if (twoFinger) {
                                pending += delta
                                val sx = pending.x.roundToInt()
                                // Finger down == content down == negative wheel.
                                val sy = (-pending.y).roundToInt()
                                if (sx != 0 || sy != 0) {
                                    pending = Offset(pending.x - sx, pending.y + sy)
                                    send {
                                        RustCore.sendMouseEvent(
                                            address,
                                            port,
                                            sx.toFloat(),
                                            sy.toFloat(),
                                            2,
                                            false,
                                        )
                                    }
                                }
                            } else if (dragged > viewConfiguration.touchSlop) {
                                val dx = (delta.x * travel).roundToInt()
                                val dy = (delta.y * travel).roundToInt()
                                if (dx != 0 || dy != 0) {
                                    send {
                                        RustCore.sendMouseEvent(
                                            address,
                                            port,
                                            dx.toFloat(),
                                            dy.toFloat(),
                                            0,
                                            false,
                                        )
                                    }
                                }
                            }
                        }
                    }
                }
            }
        },
    )
}

private fun declareFrameRate(holder: android.view.SurfaceHolder, display: android.view.Display?) {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return
    val surface = holder.surface ?: return
    runCatching {
        val refresh = display?.mode?.refreshRate ?: 0f
        if (refresh > 0f) {
            surface.setFrameRate(refresh, android.view.Surface.FRAME_RATE_COMPATIBILITY_DEFAULT)
        }
    }
}

/** Which link a live video session came up over (session UI status chip). */
enum class StreamTransportKind { Lan, Wan }

/** Lifecycle of the video connect attempt, reported to the session UI. */
sealed interface StreamStatus {
    data object Connecting : StreamStatus

    data class Up(val kind: StreamTransportKind) : StreamStatus

    data class Down(val reason: String) : StreamStatus
}

/** Owns the decoder thread; start/stop are idempotent. */
private class DecoderHost {
    @Volatile
    private var decoder: H264Decoder? = null
    @Volatile
    private var thread: Thread? = null

    fun start(
        context: android.content.Context,
        surface: android.view.Surface,
        address: String,
        port: Int,
        width: Int,
        height: Int,
        wanIdentity: String?,
        monitorIndex: Int = -1,
        onStatus: (StreamStatus) -> Unit = {},
        onCodec: (String) -> Unit = {},
        /** Last so call sites keep the size-reporting lambda trailing. */
        onVideoSize: (Int, Int) -> Unit,
    ) {
        if (thread != null) return
        val active = H264Decoder(surface, width, height, onVideoSize, onCodec)
        decoder = active
        // Stale reports from a stopped generation must not clobber a newer
        // attempt's status.
        val report: (StreamStatus) -> Unit = { if (decoder === active) onStatus(it) }
        // The decode thread noticed the link went silent; drop the UI onto the
        // error card (Retry / Exit) instead of leaving it on a frozen frame.
        active.onStalled = {
            report(StreamStatus.Down("video feed stalled — connection closed"))
        }
        report(StreamStatus.Connecting)
        thread = Thread({
            // The Rust bridge blocks, so connect + drain share this thread.
            // LAN first; fall back to dialing the cached iroh WAN identity
            // when the desktop is off-network (R1 stage 3).
            val lan = RustCore.connectStreaming(address, port, monitorIndex)
            var connected = lan.isSuccess
            val status = when {
                connected -> StreamStatus.Up(StreamTransportKind.Lan)
                wanIdentity == null -> StreamStatus.Down(
                    "LAN connect failed (${lan.exceptionOrNull()?.message ?: "no error"}); " +
                        "no cached WAN identity for $address — pair/connect on LAN first.",
                )
                else -> {
                    val wan = RustCore.connectStreamingWan(address, wanIdentity, monitorIndex)
                    connected = wan.isSuccess
                    if (wan.isSuccess) {
                        StreamStatus.Up(StreamTransportKind.Wan)
                    } else {
                        StreamStatus.Down(
                            "LAN: ${lan.exceptionOrNull()?.message ?: "?"} · " +
                                "WAN: ${wan.exceptionOrNull()?.message ?: "?"}",
                        )
                    }
                }
            }
            report(status)
            if (connected) {
                active.start()
                // Not for a generation that stop() already ended.
                if (decoder === active) DesktopAudio.streamUp(context)
            }
        }, "h264-decode").apply {
            isDaemon = true
            priority = Thread.MAX_PRIORITY
            start()
        }
    }

    fun stop() {
        val active = decoder ?: return
        decoder = null
        DesktopAudio.streamDown()
        active.stop()
        runCatching { RustCore.stopStreaming() }
        thread?.let { t ->
            thread = null
            runCatching { t.join(2000) }
        }
    }
}
