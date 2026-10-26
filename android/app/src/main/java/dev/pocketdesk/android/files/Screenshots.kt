package dev.pocketdesk.android.files

import android.Manifest
import android.content.ContentUris
import android.content.Context
import android.content.pm.PackageManager
import android.database.ContentObserver
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.provider.MediaStore
import android.util.Log
import androidx.core.content.edit
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.clip.ClipboardShared
import dev.pocketdesk.android.notify.PhoneLink
import dev.pocketdesk.android.service.TransferService
import dev.pocketdesk.android.ui.ClipboardImages
import dev.pocketdesk.android.ui.ImageClipRule
import dev.pocketdesk.android.wake.Waker
import java.io.File

/** Settings "New screenshots to the computer" (T10h). */
object ScreenshotPrefs {
    private const val PREFS = "pocketdesk-screenshots"
    private const val KEY_ON = "on"
    private const val KEY_CLIPBOARD = "clipboard"
    private const val KEY_SAVE = "save"
    private const val KEY_SINCE = "since"
    private const val KEY_LAST_ID = "last_id"
    private const val KEY_STOPPED = "stopped"

    private fun prefs(ctx: Context) = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    fun on(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_ON, false)

    /** D1: the switch turned itself off when photo access went, and the owner has not chosen since. */
    fun stopped(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_STOPPED, false)

    /** Turned off because photo access went: Home says so until the owner chooses again. */
    fun stopForAccess(ctx: Context) = prefs(ctx).edit {
        putBoolean(KEY_ON, false)
        putBoolean(KEY_STOPPED, true)
    }

    fun clipboard(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_CLIPBOARD, true)

    fun save(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_SAVE, true)

    fun setClipboard(ctx: Context, value: Boolean) = prefs(ctx).edit { putBoolean(KEY_CLIPBOARD, value) }

    fun setSave(ctx: Context, value: Boolean) = prefs(ctx).edit { putBoolean(KEY_SAVE, value) }

    /**
     * Turn the switch on or off. On, only screenshots taken from now are
     * sent: the newest image id now is where the next look starts.
     */
    fun setOn(ctx: Context, value: Boolean) {
        prefs(ctx).edit {
            putBoolean(KEY_ON, value)
            // The owner chose: no longer "turned itself off" (D1).
            remove(KEY_STOPPED)
            if (value) {
                putLong(KEY_SINCE, System.currentTimeMillis() / 1000)
                putLong(KEY_LAST_ID, Screenshots.newestId(ctx))
            }
        }
    }

    fun since(ctx: Context): Long = prefs(ctx).getLong(KEY_SINCE, Long.MAX_VALUE)

    fun lastId(ctx: Context): Long = prefs(ctx).getLong(KEY_LAST_ID, Long.MAX_VALUE)

    fun setLastId(ctx: Context, id: Long) = prefs(ctx).edit { putLong(KEY_LAST_ID, id) }

    /** The permission that lets the app see new images. */
    val permission: String =
        if (Build.VERSION.SDK_INT >= 33) Manifest.permission.READ_MEDIA_IMAGES else Manifest.permission.READ_EXTERNAL_STORAGE

    /** Whether the app sees every new image (Android 14's "selected photos only" does not). */
    fun granted(ctx: Context): Boolean = ctx.checkSelfPermission(permission) == PackageManager.PERMISSION_GRANTED
}

/**
 * New screenshots to the phone's own computer (T10h), run on the
 * notification listener's tick: a MediaStore observer marks new images, and
 * the tick sends each new screenshot to the computer's clipboard and to its
 * Downloads, as the switches under Settings "New screenshots to the
 * computer" say.
 */
class Screenshots(private val ctx: Context) {
    @Volatile private var dirty = true

    /** Until when a computer that did not answer is left alone. */
    private var restUntilMs = 0L
    private var observer: ContentObserver? = null

    /**
     * Screenshots handed to the transfer service for Downloads, each with
     * its sends so far, and when (BB4): one that fails goes again, a few
     * times, to Downloads only.
     */
    private var handed: List<Pair<Uri, Int>> = emptyList()
    private var handedAtMs = 0L
    private var again: List<Pair<Uri, Int>> = emptyList()

    /** False while the last ones are waited for. */
    private fun settle(): Boolean {
        val ago = SystemClock.elapsedRealtime() - handedAtMs
        val failed = UploadRetry.again(handed, ago) { SendOutcomes.peek(it) } ?: return false
        val given = UploadRetry.givenUp(handed) { SendOutcomes.peek(it) }
        if (given > 0) Log.w(TAG, "$given screenshots did not reach the computer's Downloads, given up")
        handed.forEach { (uri, _) -> SendOutcomes.forget(uri) }
        handed = emptyList()
        again = failed
        dirty = true
        if (failed.isNotEmpty()) restUntilMs = SystemClock.elapsedRealtime() + REST_MS
        return true
    }

    /** Hand [uris] to the transfer service for the computer's Downloads. */
    private fun toDownloads(address: String, port: Int, uris: List<Pair<Uri, Int>>) {
        uris.forEach { (uri, _) -> SendOutcomes.expect(uri) }
        runCatching { TransferService.start(ctx, address, port, uris.map { it.first }) }
            .onFailure { Log.w(TAG, "a screenshot was not sent to the computer's Downloads", it) }
        handed = handed + uris
        handedAtMs = SystemClock.elapsedRealtime()
        // Looked at again, to settle them (BB4 review: nothing else did).
        dirty = true
    }

    fun tick() {
        if (!ScreenshotPrefs.on(ctx) || !ScreenshotPrefs.granted(ctx)) {
            // Access taken back turns it off: given back, it does not start
            // again by itself with screenshots taken in between.
            // Said on Home (D1): it would otherwise stop without a word.
            if (ScreenshotPrefs.on(ctx)) ScreenshotPrefs.stopForAccess(ctx)
            stopWatching()
            forgetWaiting()
            return
        }
        startWatching()
        if (!dirty || SystemClock.elapsedRealtime() < restUntilMs) return
        // Those sent to Downloads are settled once the service is through.
        if (handed.isNotEmpty() && (TransferService.busy() || !settle() || again.isNotEmpty())) return
        val own = PhoneLink.target(ctx) ?: return
        if (again.isNotEmpty()) {
            if (!Waker.answers(own.address, own.controlPort)) {
                restUntilMs = SystemClock.elapsedRealtime() + REST_MS
                return
            }
            val retry = again.map { (uri, sends) -> uri to sends + 1 }
            again = emptyList()
            toDownloads(own.address, own.controlPort, retry)
            return
        }
        dirty = false
        val now = System.currentTimeMillis() / 1000
        val since = ScreenshotPrefs.since(ctx)
        var sent = 0
        val images = newImages(ScreenshotPrefs.lastId(ctx))
        // A rebuilt media store numbers its images from low again: start from
        // its newest, so new screenshots are seen (old ones stay too old).
        if (images.isEmpty()) {
            val newest = newestId(ctx)
            if (newest < ScreenshotPrefs.lastId(ctx)) ScreenshotPrefs.setLastId(ctx, newest)
        }
        // More waited than one look reads: the next tick goes on from here.
        if (images.size > LOOK) dirty = true
        for (image in images) {
            // One still being written is looked at again later, with what comes after it.
            if (image.pending) {
                dirty = true
                break
            }
            // Turned off while earlier ones were sent: nothing more goes.
            if (!ScreenshotPrefs.on(ctx)) break
            if (sent == ScreenshotRule.MAX_PER_TICK) {
                dirty = true
                break
            }
            if (!ScreenshotRule.inScreenshots(image.folder) ||
                !ScreenshotRule.due(image.addedSecs, since, now) ||
                !ScreenshotRule.sendable(image.size)
            ) {
                ScreenshotPrefs.setLastId(ctx, image.id)
                continue
            }
            // Both halves need the computer to dial back, so it must answer
            // here: otherwise the screenshot waits, while it is recent enough.
            if (!Waker.answers(own.address, own.controlPort)) {
                restUntilMs = SystemClock.elapsedRealtime() + REST_MS
                dirty = true
                break
            }
            ScreenshotPrefs.setLastId(ctx, image.id)
            sent++
            send(own.address, own.controlPort, image.uri)
        }
    }

    private fun send(address: String, port: Int, uri: Uri) {
        if (ScreenshotPrefs.clipboard(ctx)) {
            val png = ClipboardImages.pngOf(ctx, uri)
            // Named before it goes, so a session sync does not fetch it back.
            png?.let { ClipboardShared.sentImageId = ImageClipRule.idOf(it) }
            val result =
                png?.let { ClipboardImages.withStaged(ctx, it) { file -> RustCore.sendClipboardImage(address, port, file.absolutePath) } }
            if (result == null || result.isFailure) Log.w(TAG, "a screenshot did not reach the computer's clipboard: ${result?.exceptionOrNull()}")
        }
        if (ScreenshotPrefs.save(ctx)) toDownloads(address, port, listOf(uri to 1))
    }

    private data class Image(val id: Long, val uri: Uri, val folder: String?, val addedSecs: Long, val size: Long, val pending: Boolean)

    /** The images added after [afterId], oldest first. */
    private fun newImages(afterId: Long): List<Image> {
        val folderColumn = if (Build.VERSION.SDK_INT >= 29) MediaStore.Images.Media.RELATIVE_PATH else DATA
        val columns =
            buildList {
                add(MediaStore.Images.Media._ID)
                add(folderColumn)
                add(MediaStore.Images.Media.DATE_ADDED)
                add(MediaStore.Images.Media.SIZE)
                if (Build.VERSION.SDK_INT >= 29) add(MediaStore.Images.Media.IS_PENDING)
            }.toTypedArray()
        val found = mutableListOf<Image>()
        runCatching {
            ctx.contentResolver.query(
                MediaStore.Images.Media.EXTERNAL_CONTENT_URI,
                columns,
                "${MediaStore.Images.Media._ID} > ?",
                arrayOf(afterId.toString()),
                "${MediaStore.Images.Media._ID} ASC",
            )?.use { c ->
                while (c.moveToNext() && found.size <= LOOK) {
                    val id = c.getLong(0)
                    val folder = c.getString(1)?.let { if (Build.VERSION.SDK_INT >= 29) it else File(it).parent }
                    found +=
                        Image(
                            id,
                            ContentUris.withAppendedId(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, id),
                            folder,
                            c.getLong(2),
                            c.getLong(3),
                            Build.VERSION.SDK_INT >= 29 && c.getInt(4) != 0,
                        )
                }
            }
        }.onFailure { Log.w(TAG, "could not look for new screenshots", it) }
        return found
    }

    /** Set once the listener goes: no observer is made after. */
    private var closed = false

    @Synchronized
    private fun startWatching() {
        if (observer != null || closed) return
        val watch =
            object : ContentObserver(Handler(Looper.getMainLooper())) {
                override fun onChange(selfChange: Boolean) {
                    dirty = true
                }
            }
        runCatching {
            ctx.contentResolver.registerContentObserver(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, true, watch)
            observer = watch
            dirty = true
        }.onFailure { Log.w(TAG, "could not watch for new screenshots", it) }
    }

    @Synchronized
    private fun stopWatching() {
        observer?.let { runCatching { ctx.contentResolver.unregisterContentObserver(it) } }
        observer = null
    }

    /** Stop watching, when the listener goes; a tick still running makes no observer after. */
    @Synchronized
    fun close() {
        closed = true
        stopWatching()
        forgetWaiting()
    }

    /** Turned off, or gone: nothing that waited goes later (BB4 review). */
    private fun forgetWaiting() {
        (handed + again).forEach { (uri, _) -> SendOutcomes.forget(uri) }
        handed = emptyList()
        again = emptyList()
    }

    companion object {
        private const val TAG = "Screenshots"

        /** How long a computer that did not answer is left before a waiting screenshot is tried again. */
        private const val REST_MS = 30_000L

        /** New images one look reads, at most (one more says that more wait). */
        private const val LOOK = ScreenshotRule.MAX_PER_TICK * 4

        @Suppress("DEPRECATION")
        private const val DATA = MediaStore.Images.Media.DATA

        /** The newest image's id now, 0 when none can be read. */
        fun newestId(ctx: Context): Long =
            runCatching {
                ctx.contentResolver.query(
                    MediaStore.Images.Media.EXTERNAL_CONTENT_URI,
                    arrayOf(MediaStore.Images.Media._ID),
                    null,
                    null,
                    "${MediaStore.Images.Media._ID} DESC",
                )?.use { c -> if (c.moveToFirst()) c.getLong(0) else 0L }
            }.getOrNull() ?: 0L
    }
}
