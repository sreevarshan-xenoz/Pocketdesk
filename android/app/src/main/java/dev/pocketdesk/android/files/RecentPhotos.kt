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

    private fun prefs(ctx: Context) = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    fun on(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_ON, false)

    fun setOn(ctx: Context, value: Boolean) = prefs(ctx).edit { putBoolean(KEY_ON, value) }

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
    /** A request as the bridge kept it. */
    private data class Request(val host: String, val port: Int, val fetch: Boolean, val id: Long)

    /** The ids the last answer listed: the only ones a fetch may name. */
    @Volatile
    private var lastListed: Set<Long> = emptySet()

    private var lastConnectMs = 0L

    fun tick() {
        val requests = takeRequests()
        val on = RecentPhotoPrefs.on(ctx)
        // Nothing turned on and nothing asked: nothing to do (N7).
        if (!on && requests.isEmpty()) return
        val own = PhoneLink.target(ctx)
        // The computer's requests come on the control link: kept while on.
        if (on && RecentPhotoPrefs.granted(ctx) && own != null && RustCore.controlPeer() == null) {
            val now = SystemClock.elapsedRealtime()
            if (now - lastConnectMs >= RECONNECT_MS) {
                lastConnectMs = now
                RustCore.connectToPeer(own.address, own.controlPort, HostStore.wanIdentity(ctx, own.address))
            }
        }
        for (request in requests) {
            if (!PhoneFilesRule.fromOwnComputer(request.host, request.port, own?.address, own?.controlPort)) continue
            runCatching { if (request.fetch) fetch(request) else list(request) }
                .onFailure { Log.w(TAG, "a request for this phone's photos failed", it) }
        }
    }

    private fun takeRequests(): List<Request> =
        runCatching {
            val array = JSONArray(RustCore.takePhotoRequests())
            (0 until array.length()).map { i ->
                val o = array.getJSONObject(i)
                Request(o.getString("host"), o.getInt("port"), o.getBoolean("fetch"), o.optLong("id", -1))
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
        if (why != null) {
            lastListed = emptySet()
            answer(request, JSONObject().put("photos", JSONArray()).put("error", why))
            return
        }
        val listed = RecentPhotoRule.listed(rows())
        val photos = JSONArray()
        listed.forEach { row ->
            val photo =
                JSONObject()
                    .put("id", row.id)
                    .put("name", row.name)
                    .put("taken", row.timeMs)
                    .put("size", row.size)
            thumbnail(uri(row.id))?.let { photo.put("thumb", it) }
            photos.put(photo)
        }
        lastListed = listed.map { it.id }.toSet()
        answer(request, JSONObject().put("photos", photos))
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
        TransferService.start(ctx, request.host, request.port, listOf(uri(request.id)))
    }

    private fun uri(id: Long): Uri = ContentUris.withAppendedId(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, id)

    /** The newest image rows (or the one [only] names), as the rule sees them. */
    private fun rows(only: Long? = null): List<RecentPhotoRule.Row> {
        val modern = Build.VERSION.SDK_INT >= 29
        val folderColumn = if (modern) MediaStore.Images.Media.RELATIVE_PATH else DATA
        val columns =
            buildList {
                add(MediaStore.Images.Media._ID)
                add(MediaStore.Images.Media.DISPLAY_NAME)
                add(folderColumn)
                add(MediaStore.Images.Media.DATE_TAKEN)
                add(MediaStore.Images.Media.DATE_ADDED)
                add(MediaStore.Images.Media.SIZE)
                if (modern) add(MediaStore.Images.Media.IS_PENDING)
            }.toTypedArray()
        val (selection, args) =
            if (only != null) {
                "${MediaStore.Images.Media._ID} = ?" to arrayOf(only.toString())
            } else if (modern) {
                "${MediaStore.Images.Media.RELATIVE_PATH} LIKE ?" to arrayOf("DCIM/%")
            } else {
                "$DATA LIKE ?" to arrayOf("%/DCIM/%")
            }
        val found = mutableListOf<RecentPhotoRule.Row>()
        ctx.contentResolver.query(
            MediaStore.Images.Media.EXTERNAL_CONTENT_URI,
            columns,
            selection,
            args,
            "${MediaStore.Images.Media.DATE_ADDED} DESC",
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
                        pending = modern && c.getInt(6) != 0,
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
    }
}
