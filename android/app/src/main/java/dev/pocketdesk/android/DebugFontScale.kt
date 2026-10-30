package dev.pocketdesk.android

import android.content.Context
import android.content.pm.ApplicationInfo
import android.content.res.Configuration
import java.io.File

/**
 * UX2: a font scale for checking layouts at 0.85 and 1.3 on a phone whose
 * system refuses `settings put system font_scale` from adb (ColorOS). Only
 * in a debuggable build: there, a number in the app's own
 * `files/debug_font_scale` (written with `adb shell run-as`) replaces the
 * system's font scale in MainActivity (the app's screens; services and the
 * share target keep the system's). Release builds never read it, so it is
 * not a setting.
 */
object DebugFontScale {
    const val FILE = "debug_font_scale"

    /** The scale a file's text asks for, or null for none or nonsense. */
    fun parse(text: String?): Float? =
        text?.trim()?.toFloatOrNull()?.takeIf { it in 0.5f..2.0f }

    /** [base] with the file's scale, or [base] itself: always so in a release build. */
    fun wrap(base: Context): Context {
        val scale = scaleFor(base) ?: return base
        val config = Configuration(base.resources.configuration).apply { fontScale = scale }
        return base.createConfigurationContext(config)
    }

    /** The scale asked for, only in a debuggable build (the release guard). */
    internal fun scaleFor(base: Context): Float? {
        if (base.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE == 0) return null
        return parse(runCatching { File(base.filesDir, FILE).readText() }.getOrNull())
    }
}
