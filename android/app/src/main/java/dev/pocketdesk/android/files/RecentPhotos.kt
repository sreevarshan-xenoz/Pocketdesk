package dev.pocketdesk.android.files

import android.content.ContentUris
import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.os.Build
import android.provider.MediaStore
import android.util.Base64
import android.util.Log
import android.util.Size
import android.os.SystemClock
import androidx.core.content.edit
import androidx.core.graphics.scale
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.clip.ClipboardShared
import dev.pocketdesk.android.ui.ClipboardImages
import dev.pocketdesk.android.ui.ImageClipRule
import dev.pocketdesk.android.notify.PhoneLink
import dev.pocketdesk.android.service.TransferService
import org.json.JSONArray
import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.io.File

/** Settings "Recent photos on the computer" (C7): off by default. */
object RecentPhotoPrefs {
    private const val PREFS = "pocketdesk-recent-photos"
    private const val KEY_ON = "on"
    private const val KEY_VIDEOS = "videos"

    private fun prefs(ctx: Context) = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    fun on(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_ON, false)

    fun setOn(ctx: Context, value: Boolean) = prefs(ctx).edit { putBoolean(KEY_ON, value) }

    /** T4: the camera's videos listed too, off by default; with video access. */
    fun videos(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_VIDEOS, false) && CameraPrefs.videosGranted(ctx)

    fun setVideos(ctx: Context, value: Boolean) = prefs(ctx).edit { putBoolean(KEY_VIDEOS, value) }

    /** The same photo access new screenshots to the computer asks for. */
    val permission: String get() = ScreenshotPrefs.permission

    fun granted(ctx: Context): Boolean = ScreenshotPrefs.granted(ctx)
}

/**
 * The computer asking for this phone's recent photos (C7), run on the
 * notification listener's tick: a list is answered with the newest camera
 * photos and a small thumbnail of each, a fetch by sending that photo the
 * way the share sheet does. Only for the phone's own computer, with the
 * switch on and photo access granted; a fetch only for a photo the last
 * answer listed.
 */
class RecentPhotos(private val ctx: Context) {
    /** A request as the bridge kept it: T4's older page ([before], -1 for the newest) and copy onto the clipboard. */
    private data class Request(
        val host: String,
        val port: Int,
        val fetch: Boolean,
        val id: Long,
        val take: Boolean = false,
        val before: Long = -1,
        val clipboard: Boolean = false,
    )

    /** The ids the last answer listed: the only ones a fetch may name. */
    @Volatile
    private var lastListed: Set<Long> = emptySet()

    private var lastConnectMs = 0L

    fun tick() {
        val requests = takeRequests()
        val on = RecentPhotoPrefs.on(ctx)
        // F3: a photo asked for comes on the same link (F3 review: it was kept
        // only for Recent photos, so an ask never reached a phone without it).
        val camera = CameraAsk.enabled(ctx)
        // Nothing turned on and nothing asked: nothing to do (N7).
        if (!on && !camera && requests.isEmpty()) return
        val own = PhoneLink.target(ctx)
        // The computer's requests come on the control link: kept while on.
        if (((on && RecentPhotoPrefs.granted(ctx)) || camera) && own != null && RustCore.controlPeer() == null) {
            val now = SystemClock.elapsedRealtime()
            if (now - lastConnectMs >= RECONNECT_MS) {
                lastConnectMs = now
                RustCore.connectToPeer(own.address, own.controlPort, HostStore.wanIdentity(ctx, own.address))
            }
        }
        for (request in requests) {
            val ownComputer = PhoneFilesRule.fromOwnComputer(request.host, request.port, own?.address, own?.controlPort)
            // F3: a photo asked for is offered to the owner, whatever Recent photos says.
            if (request.take) {
                if (CameraAsk.shouldOffer(CameraAsk.enabled(ctx), ownComputer)) CameraAsk.offer(ctx, request.host, request.port)
                continue
            }
            if (!ownComputer) continue
            runCatching { if (request.fetch) fetch(request) else list(request) }
                .onFailure { Log.w(TAG, "a request for this phone's photos failed", it) }
        }
    }

    private fun takeRequests(): List<Request> =
        runCatching {
            val array = JSONArray(RustCore.takePhotoRequests())
            (0 until array.length()).map { i ->
                val o = array.getJSONObject(i)
                Request(
                    o.getString("host"),
                    o.getInt("port"),
                    o.getBoolean("fetch"),
                    o.optLong("id", -1),
                    o.optBoolean("take", false),
                    o.optLong("before", -1),
                    o.optBoolean("clipboard", false),
                )
            }
        }.getOrDefault(emptyList())

    /** Why the phone does not answer with photos, or null when it may. */
    private fun refusal(): String? =
        when {
            !RecentPhotoPrefs.on(ctx) -> "Recent photos are turned off in Pocketdesk's Settings on the phone"
            !RecentPhotoPrefs.granted(ctx) -> "Pocketdesk on the phone has no access to photos: turn Recent photos on again there"
            else -> null
        }

    private fun answer(request: Request, body: JSONObject) {
        RustCore.sendPhonePacket(request.host, request.port, PHOTOS, body.toString())
            .onFailure { Log.w(TAG, "could not send this phone's photos", it) }
    }

    private fun list(request: Request) {
        val why = refusal()
        val older = request.before >= 0
        if (why != null) {
            lastListed = emptySet()
            answer(request, JSONObject().put("photos", JSONArray()).put("error", why).also { if (older) it.put("before", request.before) })
            return
        }
        val rows = rows(before = request.before.takeIf { older })
        val page = RecentPhotoRule.page(rows, lookedAll = rows.size < LOOK)
        val photos = JSONArray()
        page.listed.forEach { row ->
            val photo =
                JSONObject()
                    .put("id", row.id)
                    .put("name", row.name)
                    .put("taken", row.timeMs)
                    .put("size", row.size)
            if (row.video) photo.put("video", true)
            thumbnail(uri(row.id, row.video))?.let { photo.put("thumb", it) }
            photos.put(photo)
        }
        lastListed = RecentPhotoRule.remember(lastListed, page.listed.map { it.id }, older)
        val body = JSONObject().put("photos", photos).put("more", page.more)
        page.next?.let { body.put("next", it) }
        if (older) body.put("before", request.before)
        answer(request, body)
    }

    private fun fetch(request: Request) {
        if (refusal() != null || !RecentPhotoRule.mayFetch(request.id, lastListed)) {
            // Not sent: the computer gets a fresh list (or why not) to choose
            // from again, rather than waiting for nothing (C7 review).
            Log.i(TAG, "a photo the last answer did not list was not sent")
            list(request.copy(fetch = false))
            return
        }
        // Still a camera photo: it may have been moved or deleted since.
        val still = rows(request.id).firstOrNull()?.takeIf(RecentPhotoRule::camera)
        if (still == null) {
            list(request.copy(fetch = false))
            return
        }
        val uri = uri(request.id, still.video)
        if (request.clipboard) {
            // T4: onto the computer's clipboard as a PNG (a HEIC photo the
            // computer cannot decode), as a new screenshot goes.
            if (still.video) return
            val png = ClipboardImages.pngOf(ctx, uri)
            png?.let { ClipboardShared.sentImageId = ImageClipRule.idOf(it) }
            val result = png?.let { ClipboardImages.withStaged(ctx, it) { file -> RustCore.sendClipboardImage(request.host, request.port, file.absolutePath) } }
            if (result == null || result.isFailure) Log.w(TAG, "a photo did not reach the computer's clipboard: ${result?.exceptionOrNull()}")
            return
        }
        TransferService.start(ctx, request.host, request.port, listOf(uri))
    }

    private fun uri(id: Long, video: Boolean): Uri =
        ContentUris.withAppendedId(
            if (video) MediaStore.Video.Media.EXTERNAL_CONTENT_URI else MediaStore.Images.Media.EXTERNAL_CONTENT_URI,
            id,
        )

    /**
     * The newest camera rows (or the one [only] names; or T4's page of those
     * older than [before]), as the rule sees them: photos, and videos too
     * with video access, from the one media table, newest first by id.
     */
    private fun rows(only: Long? = null, before: Long? = null): List<RecentPhotoRule.Row> {
        val modern = Build.VERSION.SDK_INT >= 29
        val folderColumn = if (modern) MediaStore.MediaColumns.RELATIVE_PATH else DATA
        val columns =
            buildList {
                add(MediaStore.MediaColumns._ID)
                add(MediaStore.MediaColumns.DISPLAY_NAME)
                add(folderColumn)
                add(DATE_TAKEN)
                add(MediaStore.MediaColumns.DATE_ADDED)
                add(MediaStore.MediaColumns.SIZE)
                add(MediaStore.Files.FileColumns.MEDIA_TYPE)
                if (modern) add(MediaStore.MediaColumns.IS_PENDING)
            }.toTypedArray()
        val types =
            buildList {
                add(MediaStore.Files.FileColumns.MEDIA_TYPE_IMAGE)
                if (RecentPhotoPrefs.videos(ctx)) add(MediaStore.Files.FileColumns.MEDIA_TYPE_VIDEO)
            }.joinToString(",")
        val clauses = mutableListOf("${MediaStore.Files.FileColumns.MEDIA_TYPE} IN ($types)")
        val args = mutableListOf<String>()
        if (only != null) {
            clauses += "${MediaStore.MediaColumns._ID} = ?"
            args += only.toString()
        } else {
            clauses += if (modern) "${MediaStore.MediaColumns.RELATIVE_PATH} LIKE ?" else "$DATA LIKE ?"
            args += if (modern) "DCIM/%" else "%/DCIM/%"
            if (before != null) {
                clauses += "${MediaStore.MediaColumns._ID} < ?"
                args += before.toString()
            }
        }
        val found = mutableListOf<RecentPhotoRule.Row>()
        ctx.contentResolver.query(
            MediaStore.Files.getContentUri("external"),
            columns,
            clauses.joinToString(" AND "),
            args.toTypedArray(),
            "${MediaStore.MediaColumns._ID} DESC",
        )?.use { c ->
            while (c.moveToNext() && found.size < LOOK) {
                val folder = c.getString(2)?.let { if (modern) it else File(it).parent?.let(::relativeOf) }
                found +=
                    RecentPhotoRule.Row(
                        id = c.getLong(0),
                        name = c.getString(1).orEmpty(),
                        folder = folder,
                        takenMs = c.getLong(3),
                        addedSecs = c.getLong(4),
                        size = c.getLong(5),
                        pending = modern && c.getInt(7) != 0,
                        video = c.getInt(6) == MediaStore.Files.FileColumns.MEDIA_TYPE_VIDEO,
                    )
            }
        }
        return found
    }

    /**
     * A legacy absolute folder as a path below the storage's root
     * (`/storage/emulated/0/DCIM/Camera` → `DCIM/Camera/`): only a `DCIM`
     * right under a storage volume's root (`/storage/<volume>/` or
     * `/storage/emulated/<user>/`), never one deeper (C7 review).
     */
    private fun relativeOf(absolute: String): String? {
        val parts = absolute.trim('/').split('/')
        if (parts.firstOrNull() != "storage") return null
        val root = if (parts.getOrNull(1) == "emulated") 3 else 2
        val rest = parts.drop(root)
        return if (rest.firstOrNull() == "DCIM") rest.joinToString("/") + "/" else null
    }

    /** [uri]'s thumbnail as base64 JPEG, at most [RecentPhotoRule.MAX_THUMB_BYTES], or null. */
    private fun thumbnail(uri: Uri): String? =
        runCatching {
            val side = RecentPhotoRule.THUMB_SIDE
            val bitmap =
                if (Build.VERSION.SDK_INT >= 29) {
                    ctx.contentResolver.loadThumbnail(uri, Size(side, side), null)
                } else {
                    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
                    ctx.contentResolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, bounds) }
                    // Unread bounds would decode the whole photo.
                    if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return@runCatching null
                    val options = BitmapFactory.Options().apply { inSampleSize = RecentPhotoRule.sampleSize(bounds.outWidth, bounds.outHeight) }
                    ctx.contentResolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, options) }
                } ?: return@runCatching null
            val scale = side.toFloat() / maxOf(bitmap.width, bitmap.height)
            val sized =
                if (scale < 1f) {
                    bitmap.scale((bitmap.width * scale).toInt().coerceAtLeast(1), (bitmap.height * scale).toInt().coerceAtLeast(1))
                } else {
                    bitmap
                }
            for (quality in intArrayOf(80, 65, 50, 35)) {
                val out = ByteArrayOutputStream()
                sized.compress(Bitmap.CompressFormat.JPEG, quality, out)
                if (out.size() <= RecentPhotoRule.MAX_THUMB_BYTES) return@runCatching Base64.encodeToString(out.toByteArray(), Base64.NO_WRAP)
            }
            null
        }.onFailure { Log.w(TAG, "no thumbnail for a photo", it) }.getOrNull()

    private companion object {
        const val TAG = "RecentPhotos"
        const val PHOTOS = "kdeconnect.pocketdesk.phone_photos"

        const val RECONNECT_MS = 60_000L

        /** Rows looked at, at most: the rule keeps the newest camera photos of them. */
        const val LOOK = 200

        @Suppress("DEPRECATION")
        val DATA: String = MediaStore.Images.Media.DATA

        /** When it was taken: the media table's own column on every version (T4). */
        @Suppress("InlinedApi")
        val DATE_TAKEN: String = MediaStore.MediaColumns.DATE_TAKEN
    }
}
