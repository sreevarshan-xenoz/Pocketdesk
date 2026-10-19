package dev.pocketdesk.android.cast

import android.annotation.SuppressLint
import android.content.Context
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.media.MediaCodec
import android.os.Handler
import android.os.HandlerThread
import android.util.Log
import android.view.Surface

/**
 * The phone's back camera as the cast's picture (F6): Camera2 draws into
 * the cast encoder's surface, as the virtual display does for the screen.
 * The frames are the sensor's, landscape: the phone is held on its side.
 */
class CameraFeed(private val ctx: Context, private val onLost: () -> Unit) {
    private val manager = ctx.getSystemService(CameraManager::class.java)
    private val thread = HandlerThread("cast-camera").apply { start() }
    private val handler = Handler(thread.looper)
    // Guarded by `this`: a stop while the camera opens must close it (F6 review).
    private var device: CameraDevice? = null
    private var session: CameraCaptureSession? = null
    private var closed = false

    /** The back camera's id, or the first one when there is no back camera. */
    private fun cameraId(): String? {
        val ids = runCatching { manager.cameraIdList.toList() }.getOrDefault(emptyList())
        return ids.firstOrNull { id ->
            runCatching { manager.getCameraCharacteristics(id).get(CameraCharacteristics.LENS_FACING) }
                .getOrNull() == CameraCharacteristics.LENS_FACING_BACK
        } ?: ids.firstOrNull()
    }

    /** The size to encode at: what the camera offers an encoder, as [choose] picks. */
    fun size(): Pair<Int, Int>? {
        val id = cameraId() ?: return null
        val map = runCatching {
            manager.getCameraCharacteristics(id).get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)
        }.getOrNull() ?: return null
        val sizes = map.getOutputSizes(MediaCodec::class.java)?.map { it.width to it.height }.orEmpty()
        return choose(sizes)
    }

    /** Open the camera drawing into [surface]; false when there is none to open. */
    @SuppressLint("MissingPermission") // The service starts only with the camera permission.
    fun open(surface: Surface): Boolean {
        val id = cameraId() ?: return false
        return runCatching {
            manager.openCamera(
                id,
                object : CameraDevice.StateCallback() {
                    override fun onOpened(camera: CameraDevice) {
                        val keep = synchronized(this@CameraFeed) {
                            if (!closed) device = camera
                            !closed
                        }
                        if (!keep) {
                            camera.close()
                            return
                        }
                        startSession(camera, surface)
                    }

                    override fun onDisconnected(camera: CameraDevice) {
                        camera.close()
                        lost("the camera was taken by another app")
                    }

                    override fun onError(camera: CameraDevice, error: Int) {
                        camera.close()
                        lost("the camera failed ($error)")
                    }
                },
                handler,
            )
        }.onFailure { Log.w(TAG, "could not open the camera", it) }.isSuccess
    }

    @Suppress("DEPRECATION") // The executor form needs API 28; this one serves 26 and up.
    private fun startSession(camera: CameraDevice, surface: Surface) {
        runCatching {
            camera.createCaptureSession(
                listOf(surface),
                object : CameraCaptureSession.StateCallback() {
                    override fun onConfigured(configured: CameraCaptureSession) {
                        val keep = synchronized(this@CameraFeed) {
                            if (!closed) session = configured
                            !closed
                        }
                        if (!keep) {
                            configured.close()
                            return
                        }
                        // The camera may close meanwhile: its request then throws (F6 review).
                        runCatching {
                            val request = camera.createCaptureRequest(CameraDevice.TEMPLATE_RECORD).apply { addTarget(surface) }
                            configured.setRepeatingRequest(request.build(), null, handler)
                        }.onFailure { lost("the camera would not start") }
                    }

                    override fun onConfigureFailed(failed: CameraCaptureSession) = lost("the camera would not start")
                },
                handler,
            )
        }.onFailure { lost("the camera would not start") }
    }

    private fun lost(why: String) {
        Log.w(TAG, "camera lost: $why")
        if (!synchronized(this) { closed }) onLost()
    }

    fun close() {
        val (open, camera) = synchronized(this) {
            closed = true
            val held = session to device
            session = null
            device = null
            held
        }
        runCatching { open?.close() }
        runCatching { camera?.close() }
        // Left running until a camera still opening has said so and been closed.
        handler.post { thread.quitSafely() }
    }

    companion object {
        private const val TAG = "CameraFeed"

        /**
         * The largest of [sizes] within 1920 by 1080, a 16:9 one first; the
         * smallest offered when none fits.
         */
        fun choose(sizes: List<Pair<Int, Int>>): Pair<Int, Int>? {
            if (sizes.isEmpty()) return null
            val fits = sizes.filter { (w, h) -> w <= 1920 && h <= 1080 }
            if (fits.isEmpty()) return sizes.minBy { (w, h) -> w.toLong() * h }
            val wide = fits.filter { (w, h) -> w * 9 == h * 16 }
            return (wide.ifEmpty { fits }).maxBy { (w, h) -> w.toLong() * h }
        }
    }
}
