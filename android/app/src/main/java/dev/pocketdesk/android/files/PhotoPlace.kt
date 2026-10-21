package dev.pocketdesk.android.files

import android.content.Context
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.MediaStore
import android.util.Log
import androidx.core.content.edit
import java.io.FileNotFoundException

/**
 * Photos keep where they were taken (M5): since Android 10 a photo read
 * from MediaStore by another app comes without its GPS tags, unless the
 * app holds photo access and `ACCESS_MEDIA_LOCATION` and asks for the
 * original.
 */
object PhotoPlaceRule {
    /** MediaStore's authority: only its items can be asked for whole. */
    const val MEDIA_AUTHORITY = "media"

    /** The documents providers whose items map to MediaStore's (`MediaStore.getMediaUri`). */
    const val MEDIA_DOCUMENTS = "com.android.providers.media.documents"
    const val EXTERNAL_STORAGE = "com.android.externalstorage.documents"

    /** How an item at an authority is asked for whole. */
    enum class Route { AS_NAMED, ORIGINAL, MAPPED_ORIGINAL }

    fun route(on: Boolean, granted: Boolean, sdk: Int, authority: String?): Route = when {
        !on || !granted || sdk < Build.VERSION_CODES.Q -> Route.AS_NAMED
        authority == MEDIA_AUTHORITY -> Route.ORIGINAL
        // A picked file or folder (the Files app, a folder send, the drive)
        // is the same photo under another name (M5 review).
        authority == MEDIA_DOCUMENTS -> Route.MAPPED_ORIGINAL
        authority == EXTERNAL_STORAGE && sdk >= Build.VERSION_CODES.R -> Route.MAPPED_ORIGINAL
        else -> Route.AS_NAMED
    }

    /** What was opened, and whether it was the original. */
    data class Opened<T>(val value: T, val original: Boolean)

    /**
     * Open [original] with [opener] when there is one, else [plain]; an
     * original Android will not open (a share's grant covers only the item
     * it named, or the photo access went) falls back to [plain]. Only those
     * refusals: anything else, a cancel among them, is the send's own.
     */
    fun <K, T> firstThatOpens(original: K?, plain: K, opener: (K) -> T): Opened<T> {
        if (original == null) return Opened(opener(plain), false)
        return try {
            Opened(opener(original), true)
        } catch (e: SecurityException) {
            Opened(opener(plain), false)
        } catch (e: UnsupportedOperationException) {
            Opened(opener(plain), false)
        } catch (e: FileNotFoundException) {
            Opened(opener(plain), false)
        } catch (e: IllegalArgumentException) {
            Opened(opener(plain), false)
        }
    }
}

/** Settings "Photos keep where they were taken" (M5): off by default. */
object PhotoPlacePrefs {
    private const val TAG = "PhotoPlace"
    private const val PREFS = "pocketdesk-photo-place"
    private const val KEY_ON = "on"

    private fun prefs(ctx: Context) = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    fun on(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_ON, false)

    fun setOn(ctx: Context, value: Boolean) = prefs(ctx).edit { putBoolean(KEY_ON, value) }

    /** Android 10+'s media location permission. */
    const val PERMISSION = "android.permission.ACCESS_MEDIA_LOCATION"

    /**
     * What it asks for, together (M5 review): Android grants the media
     * location only to an app with photo access, without a word otherwise.
     */
    val permissions: Array<String> get() = arrayOf(ScreenshotPrefs.permission, PERMISSION)

    /** Whether it may work: photo access (all photos, not some) and the media location. */
    fun granted(ctx: Context): Boolean =
        Build.VERSION.SDK_INT < Build.VERSION_CODES.Q ||
            (ScreenshotPrefs.granted(ctx) && ctx.checkSelfPermission(PERMISSION) == PackageManager.PERMISSION_GRANTED)

    /** The original of [uri] to open, place and all, or null to open it as named. */
    private fun original(ctx: Context, uri: Uri): Uri? {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return null
        return when (PhotoPlaceRule.route(on(ctx), granted(ctx), Build.VERSION.SDK_INT, uri.authority)) {
            PhotoPlaceRule.Route.AS_NAMED -> null
            PhotoPlaceRule.Route.ORIGINAL -> runCatching { MediaStore.setRequireOriginal(uri) }.getOrNull()
            PhotoPlaceRule.Route.MAPPED_ORIGINAL -> runCatching {
                MediaStore.getMediaUri(ctx, uri)?.let { MediaStore.setRequireOriginal(it) }
            }.getOrNull()
        }
    }

    /** Open [uri] with [opener], its original first when the switch and Android allow it. */
    fun <T> open(ctx: Context, uri: Uri, opener: (Uri) -> T): PhotoPlaceRule.Opened<T> {
        val original = original(ctx, uri)
        val opened = PhotoPlaceRule.firstThatOpens(original, uri, opener)
        if (original != null && !opened.original) {
            Log.i(TAG, "Sent without its place: the original could not be opened")
        }
        return opened
    }
}
