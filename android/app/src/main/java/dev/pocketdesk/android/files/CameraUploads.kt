package dev.pocketdesk.android.files

import android.Manifest
import android.content.ContentUris
import android.content.Context
import android.content.pm.PackageManager
import android.database.ContentObserver
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.provider.MediaStore
import android.util.Log
import androidx.core.content.edit
import dev.pocketdesk.android.clip.ClipboardPushRule
import dev.pocketdesk.android.notify.PhoneLink
import dev.pocketdesk.android.service.TransferService
import dev.pocketdesk.android.wake.Waker
import java.io.File

/** Settings "Send new camera photos" (T3). */
object CameraPrefs {
    private const val PREFS = "pocketdesk-camera-uploads"
    private const val KEY_ON = "on"
    private const val KEY_VIDEOS = "videos"
    private const val KEY_WIFI_ONLY = "wifi_only"
    private const val KEY_SINCE = "since"
    private const val KEY_LAST_PHOTO = "last_photo"
    private const val KEY_LAST_VIDEO = "last_video"
    private const val KEY_STOPPED = "stopped"

    private fun prefs(ctx: Context) = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    fun on(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_ON, false)

    fun videos(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_VIDEOS, false)

    fun wifiOnly(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_WIFI_ONLY, true)

    /** The switch turned itself off when photo access went, and the owner has not chosen since (as D1's). */
    fun stopped(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_STOPPED, false)

    fun stopForAccess(ctx: Context) = prefs(ctx).edit {
        putBoolean(KEY_ON, false)
        putBoolean(KEY_STOPPED, true)
    }

    /**
     * Turn the switch on or off. On, only what is taken from now goes: the
     * newest ids now are where the next look starts.
     */
    fun setOn(ctx: Context, value: Boolean) {
        prefs(ctx).edit {
            putBoolean(KEY_ON, value)
            remove(KEY_STOPPED)
            if (value) {
                putLong(KEY_SINCE, System.currentTimeMillis() / 1000)
                // Unread, nothing before now goes: the newest id is not known,
                // so the mark is set at the first look that reads it.
                putLong(KEY_LAST_PHOTO, CameraUploads.newestId(ctx, Kind.PHOTO) ?: Long.MAX_VALUE)
                putLong(KEY_LAST_VIDEO, CameraUploads.newestId(ctx, Kind.VIDEO) ?: Long.MAX_VALUE)
            }
        }
    }

    /** Videos too: from now, as the switch's own start. */
    fun setVideos(ctx: Context, value: Boolean) = prefs(ctx).edit {
        putBoolean(KEY_VIDEOS, value)
        if (value) putLong(KEY_LAST_VIDEO, CameraUploads.newestId(ctx, Kind.VIDEO) ?: Long.MAX_VALUE)
    }

    fun setWifiOnly(ctx: Context, value: Boolean) = prefs(ctx).edit { putBoolean(KEY_WIFI_ONLY, value) }

    fun since(ctx: Context): Long = prefs(ctx).getLong(KEY_SINCE, Long.MAX_VALUE)

    fun lastId(ctx: Context, kind: Kind): Long =
        prefs(ctx).getLong(if (kind == Kind.PHOTO) KEY_LAST_PHOTO else KEY_LAST_VIDEO, Long.MAX_VALUE)

    fun setLastId(ctx: Context, kind: Kind, id: Long) =
        prefs(ctx).edit { putLong(if (kind == Kind.PHOTO) KEY_LAST_PHOTO else KEY_LAST_VIDEO, id) }

    /** Videos need their own access from Android 13. */
    val videoPermission: String =
        if (Build.VERSION.SDK_INT >= 33) Manifest.permission.READ_MEDIA_VIDEO else Manifest.permission.READ_EXTERNAL_STORAGE

    fun photosGranted(ctx: Context): Boolean = ScreenshotPrefs.granted(ctx)

    fun videosGranted(ctx: Context): Boolean = ctx.checkSelfPermission(videoPermission) == PackageManager.PERMISSION_GRANTED

    enum class Kind { PHOTO, VIDEO }
}

/**
 * New camera photos (and videos, with that switch) to the phone's own
 * computer, into its received files (T3), run on the notification
 * listener's tick as new screenshots are: a MediaStore observer marks new
 * ones, and the tick hands each to the transfer service, which retries
 * and resumes a broken one. On Wi-Fi only unless the owner says otherwise.
 */
class CameraUploads(private val ctx: Context) {
    @Volatile private var dirty = true

    /** Until when a computer that did not answer is left alone. */
    private var restUntilMs = 0L
    private val observers = mutableListOf<ContentObserver>()

    /**
     * The last batch handed over, each with its sends so far, and when
     * (BB4): one that fails goes again, a few times.
     */
    private var handed: List<Pair<Item, Int>> = emptyList()
    private var handedAtMs = 0L

    /** Files that failed, to go again before new ones, with their sends so far. */
    private var again: List<Pair<Item, Int>> = emptyList()

    /**
     * Hand [items] (each with its sends so far, this one included) to the
     * transfer service; whether they went to it.
     */
    private fun hand(address: String, port: Int, items: List<Pair<Item, Int>>): Boolean {
        items.forEach { (item, _) -> SendOutcomes.expect(item.uri) }
        val went = runCatching { TransferService.start(ctx, address, port, items.map { it.first.uri }) }
            .onFailure { Log.w(TAG, "new camera photos were not sent to the computer", it) }
            .isSuccess
        handed = items
        handedAtMs = SystemClock.elapsedRealtime()
        return went
    }

    /**
     * How the last batch went, once the service is through (BB4): those
     * that failed go again after a rest. False while they are waited for.
     */
    private fun settle(): Boolean {
        val ago = SystemClock.elapsedRealtime() - handedAtMs
        val failed = UploadRetry.again(handed, ago) { SendOutcomes.peek(it.uri) } ?: return false
        val given = UploadRetry.givenUp(handed) { SendOutcomes.peek(it.uri) }
        handed.forEach { (item, _) -> SendOutcomes.forget(item.uri) }
        if (given > 0) Log.w(TAG, "$given new camera photos did not reach the computer, given up")
        handed = emptyList()
        again = failed
        dirty = true
        if (failed.isNotEmpty()) restUntilMs = SystemClock.elapsedRealtime() + REST_MS
        return true
    }

    fun tick() {
        if (!CameraPrefs.on(ctx) || !CameraPrefs.photosGranted(ctx)) {
            // Access taken back turns it off, said on Home (as screenshots).
            if (CameraPrefs.on(ctx)) CameraPrefs.stopForAccess(ctx)
            stopWatching()
            forgetWaiting()
            return
        }
        // Video access taken back turns "Videos too" off: given back, the
        // owner turns it on again, and only what is taken from then goes
        // (T3 review: the ones between went).
        if (CameraPrefs.videos(ctx) && !CameraPrefs.videosGranted(ctx)) CameraPrefs.setVideos(ctx, false)
        startWatching()
        if (!dirty || SystemClock.elapsedRealtime() < restUntilMs) return
        // One batch at a time, each handed over only on Wi-Fi when that is
        // asked (T3 review: a backlog queued at once went on over mobile
        // data when Wi-Fi went).
        if (TransferService.busy()) return
        if (handed.isNotEmpty() && (!settle() || again.isNotEmpty())) return
        if (!CameraRule.mayGo(CameraPrefs.wifiOnly(ctx), onWifi())) return
        val own = PhoneLink.target(ctx) ?: return
        // Those that failed go first, once the computer answers (BB4).
        if (again.isNotEmpty()) {
            if (!Waker.answers(own.address, own.controlPort)) {
                restUntilMs = SystemClock.elapsedRealtime() + REST_MS
                return
            }
            val retry = again.map { (item, sends) -> item to sends + 1 }
            again = emptyList()
            hand(own.address, own.controlPort, retry)
            return
        }
        dirty = false
        val kinds =
            buildList {
                add(CameraPrefs.Kind.PHOTO)
                if (CameraPrefs.videos(ctx)) add(CameraPrefs.Kind.VIDEO)
            }
        val since = CameraPrefs.since(ctx)
        val now = System.currentTimeMillis() / 1000
        val ready = mutableListOf<Pair<CameraPrefs.Kind, Item>>()
        for (kind in kinds) {
            // A failed read moves no mark (T3 review: it was taken for an
            // empty store and sent everything again).
            val items = newItems(kind, CameraPrefs.lastId(ctx, kind))
            if (items == null) {
                dirty = true
                continue
            }
            // A rebuilt media store numbers from low again: start from its newest.
            if (items.isEmpty()) {
                val newest = newestId(ctx, kind)
                if (newest != null && newest < CameraPrefs.lastId(ctx, kind)) CameraPrefs.setLastId(ctx, kind, newest)
            }
            if (items.size > LOOK) dirty = true
            // Once one waits to go, the mark stays before it: one skipped
            // after it is looked at again, not passed with it unsent.
            var holding = false
            for (item in items) {
                // One still being written (pending, or not written yet) waits,
                // with what comes after it; an empty one is given up after a while.
                if (item.pending || (item.size <= 0 && CameraRule.mayStillBeWritten(item.addedSecs, now))) {
                    dirty = true
                    break
                }
                if (!CameraRule.inCamera(item.folder) || !CameraRule.due(item.addedSecs, since) || item.size <= 0) {
                    if (!holding) CameraPrefs.setLastId(ctx, kind, item.id)
                    continue
                }
                if (ready.size == CameraRule.MAX_PER_TICK) {
                    dirty = true
                    break
                }
                ready += kind to item
                holding = true
            }
        }
        if (ready.isEmpty()) return
        // The transfer needs the computer to dial back, so it must answer
        // here: otherwise they wait, however long, for the next look.
        if (!Waker.answers(own.address, own.controlPort)) {
            restUntilMs = SystemClock.elapsedRealtime() + REST_MS
            dirty = true
            return
        }
        // Turned off meanwhile: nothing goes.
        if (!CameraPrefs.on(ctx)) return
        if (hand(own.address, own.controlPort, ready.map { it.second to 1 })) {
            // Marked as handed over: one that fails goes again by itself (BB4).
            ready.forEach { (kind, item) -> CameraPrefs.setLastId(ctx, kind, item.id) }
            // What waits behind them is looked at once they are through.
            dirty = true
        } else {
            handed = emptyList()
            ready.forEach { (_, item) -> SendOutcomes.forget(item.uri) }
            restUntilMs = SystemClock.elapsedRealtime() + REST_MS
            dirty = true
        }
    }

    private fun onWifi(): Boolean {
        val cm = ctx.getSystemService(ConnectivityManager::class.java) ?: return false
        val caps = cm.getNetworkCapabilities(cm.activeNetwork) ?: return false
        return ClipboardPushRule.onWifi(
            wifi = caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI),
            ethernet = caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET),
            notMetered = caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED),
            vpn = caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN),
        )
    }

    private data class Item(val id: Long, val uri: Uri, val folder: String?, val addedSecs: Long, val size: Long, val pending: Boolean)

    /**
     * The photos or videos added after [afterId], oldest first, ones still
     * pending included (T3 review: hidden, a later one seen first moved the
     * mark past them); `null` when the store could not be read.
     */
    private fun newItems(kind: CameraPrefs.Kind, afterId: Long): List<Item>? {
        val collection = collection(kind)
        val folderColumn = if (Build.VERSION.SDK_INT >= 29) MediaStore.MediaColumns.RELATIVE_PATH else DATA
        val columns =
            buildList {
                add(MediaStore.MediaColumns._ID)
                add(folderColumn)
                add(MediaStore.MediaColumns.DATE_ADDED)
                add(MediaStore.MediaColumns.SIZE)
                if (Build.VERSION.SDK_INT >= 29) add(MediaStore.MediaColumns.IS_PENDING)
            }.toTypedArray()
        val found = mutableListOf<Item>()
        return runCatching {
            query(collection, columns, afterId)?.use { c ->
                while (c.moveToNext() && found.size <= LOOK) {
                    val id = c.getLong(0)
                    val folder = c.getString(1)?.let { if (Build.VERSION.SDK_INT >= 29) it else File(it).parent }
                    found +=
                        Item(
                            id,
                            ContentUris.withAppendedId(collection, id),
                            folder,
                            c.getLong(2),
                            c.getLong(3),
                            Build.VERSION.SDK_INT >= 29 && c.getInt(4) != 0,
                        )
                }
            } ?: error("no cursor")
            found.toList()
        }.onFailure { Log.w(TAG, "could not look for new camera photos", it) }.getOrNull()
    }

    /** The rows after [afterId], pending ones too where Android keeps them apart (10 and newer). */
    private fun query(collection: Uri, columns: Array<String>, afterId: Long): android.database.Cursor? {
        val where = "${MediaStore.MediaColumns._ID} > ?"
        val args = arrayOf(afterId.toString())
        val order = "${MediaStore.MediaColumns._ID} ASC"
        return when {
            Build.VERSION.SDK_INT >= 30 ->
                ctx.contentResolver.query(
                    collection,
                    columns,
                    android.os.Bundle().apply {
                        putString(android.content.ContentResolver.QUERY_ARG_SQL_SELECTION, where)
                        putStringArray(android.content.ContentResolver.QUERY_ARG_SQL_SELECTION_ARGS, args)
                        putString(android.content.ContentResolver.QUERY_ARG_SQL_SORT_ORDER, order)
                        putInt(MediaStore.QUERY_ARG_MATCH_PENDING, MediaStore.MATCH_INCLUDE)
                    },
                    null,
                )
            Build.VERSION.SDK_INT == 29 ->
                @Suppress("DEPRECATION")
                ctx.contentResolver.query(MediaStore.setIncludePending(collection), columns, where, args, order)
            else -> ctx.contentResolver.query(collection, columns, where, args, order)
        }
    }

    /** Set once the listener goes: no observer is made after. */
    private var closed = false

    @Synchronized
    private fun startWatching() {
        if (observers.isNotEmpty() || closed) return
        for (kind in CameraPrefs.Kind.entries) {
            val watch =
                object : ContentObserver(Handler(Looper.getMainLooper())) {
                    override fun onChange(selfChange: Boolean) {
                        dirty = true
                    }
                }
            runCatching {
                ctx.contentResolver.registerContentObserver(collection(kind), true, watch)
                observers += watch
            }.onFailure { Log.w(TAG, "could not watch for new camera photos", it) }
        }
        dirty = true
    }

    @Synchronized
    private fun stopWatching() {
        observers.forEach { runCatching { ctx.contentResolver.unregisterContentObserver(it) } }
        observers.clear()
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
        (handed + again).forEach { (item, _) -> SendOutcomes.forget(item.uri) }
        handed = emptyList()
        again = emptyList()
    }

    companion object {
        private const val TAG = "CameraUploads"

        /** How long a computer that did not answer is left before waiting photos are tried again. */
        private const val REST_MS = 30_000L

        /** New items one look reads, at most (one more says that more wait). */
        private const val LOOK = CameraRule.MAX_PER_TICK * 8

        @Suppress("DEPRECATION")
        private const val DATA = MediaStore.MediaColumns.DATA

        private fun collection(kind: CameraPrefs.Kind): Uri =
            if (kind == CameraPrefs.Kind.PHOTO) MediaStore.Images.Media.EXTERNAL_CONTENT_URI else MediaStore.Video.Media.EXTERNAL_CONTENT_URI

        /** The newest photo's or video's id now: 0 for none, `null` when the store could not be read. */
        fun newestId(ctx: Context, kind: CameraPrefs.Kind): Long? =
            runCatching {
                ctx.contentResolver.query(
                    collection(kind),
                    arrayOf(MediaStore.MediaColumns._ID),
                    null,
                    null,
                    "${MediaStore.MediaColumns._ID} DESC",
                )?.use { c -> if (c.moveToFirst()) c.getLong(0) else 0L }
            }.getOrNull()
    }
}
