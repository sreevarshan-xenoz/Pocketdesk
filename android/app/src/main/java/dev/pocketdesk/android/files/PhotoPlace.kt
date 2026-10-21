package dev.pocketdesk.android.files

import android.content.Context
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.MediaStore
import androidx.core.content.edit

/**
 * Photos keep where they were taken (M5): since Android 10 a photo read
 * from MediaStore by another app comes without its GPS tags, unless the
 * app holds `ACCESS_MEDIA_LOCATION` and asks for the original.
 */
object PhotoPlaceRule {
    /** MediaStore's authority: only its items can be asked for whole. */
    const val MEDIA_AUTHORITY = "media"

    /** Ask for the original of an item at [authority]? */
    fun asksOriginal(on: Boolean, granted: Boolean, sdk: Int, authority: String?): Boolean =
        on && granted && sdk >= Build.VERSION_CODES.Q && authority == MEDIA_AUTHORITY
}

/** Settings "Photos keep where they were taken" (M5): off by default. */
object PhotoPlacePrefs {
    private const val PREFS = "pocketdesk-photo-place"
    private const val KEY_ON = "on"

    private fun prefs(ctx: Context) = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    fun on(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_ON, false)

    fun setOn(ctx: Context, value: Boolean) = prefs(ctx).edit { putBoolean(KEY_ON, value) }

    /** The permission it needs (Android 10+); on Android 9 and older nothing is taken out. */
    const val PERMISSION = "android.permission.ACCESS_MEDIA_LOCATION"

    fun granted(ctx: Context): Boolean =
        Build.VERSION.SDK_INT < Build.VERSION_CODES.Q ||
            ctx.checkSelfPermission(PERMISSION) == PackageManager.PERMISSION_GRANTED

    /** [uri] to open for sending: the original, place and all, when the switch and Android allow it. */
    fun readable(ctx: Context, uri: Uri): Uri =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q &&
            PhotoPlaceRule.asksOriginal(on(ctx), granted(ctx), Build.VERSION.SDK_INT, uri.authority)
        ) {
            runCatching { MediaStore.setRequireOriginal(uri) }.getOrDefault(uri)
        } else {
            uri
        }

    /**
     * Open [uri] with [opener], the original first (M5): a share's grant may
     * cover only the URI it named, so an original it does not cover is
     * opened as named, as before.
     */
    fun <T> open(ctx: Context, uri: Uri, opener: (Uri) -> T): T {
        val original = readable(ctx, uri)
        if (original == uri) return opener(uri)
        return runCatching { opener(original) }.getOrElse { opener(uri) }
    }
}
