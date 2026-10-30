package dev.pocketdesk.android

import android.content.Context
import androidx.annotation.StringRes

/**
 * The Pocketdesk release a saved computer runs (item 4), as it says in its
 * identity, and how it stands to this app's: the same rules as the core's
 * `version.rs`. Releases compare by their numbers alone, so a `-dev` or
 * `+<git hash>` build is the release it names. Pure, for JVM tests.
 */
object PeerVersion {
    /** The longest release kept, as the core's `MAX_VERSION_CHARS`. */
    const val MAX_CHARS = 32

    private val ALLOWED = Regex("[A-Za-z0-9.+_-]+")

    /**
     * A release as a computer says it, cleaned: its first word, letters,
     * digits and `.+-_` only, at most [MAX_CHARS], starting with a number;
     * null when nothing usable is left.
     */
    fun clean(raw: String?): String? {
        val word = raw?.trim()?.split(Regex("\\s+"))?.firstOrNull()?.takeIf { it.isNotEmpty() } ?: return null
        if (!ALLOWED.matches(word)) return null
        val cut = word.take(MAX_CHARS)
        return cut.takeIf { release(it) != null }
    }

    /** Its release numbers (`major.minor.patch`, missing ones 0), or null. */
    fun release(version: String): List<Long>? {
        val bare = version.trim().removePrefix("v").removePrefix("V")
        val numbers = bare.split('-', '+', '_', ' ').first().takeIf { it.isNotEmpty() } ?: return null
        val parts = numbers.split('.')
        if (parts.size > 3) return null
        val parsed = parts.map { part ->
            if (part.isEmpty() || part.length > 9 || !part.all { it in '0'..'9' }) return null
            part.toLong()
        }
        return parsed + List(3 - parsed.size) { 0L }
    }

    /** How [peer] stands to [mine]: negative older, 0 the same, positive newer; null when either is no release. */
    fun compare(mine: String, peer: String): Int? {
        val a = release(peer) ?: return null
        val b = release(mine) ?: return null
        for (i in 0 until 3) {
            val c = a[i].compareTo(b[i])
            if (c != 0) return c
        }
        return 0
    }

    /** A card's line: the words to show with [version], and whether it asks for an update. */
    data class Line(@param:StringRes val words: Int, val version: String, val update: Boolean)

    /**
     * What the card says of [peer] (null: the computer said none), with
     * [hints] ("Tell me when my devices run different versions") which to
     * update when it is not [mine]'s release. Never an error.
     */
    fun line(peer: String?, mine: String, hints: Boolean): Line? {
        val version = peer ?: return null
        val c = if (hints) compare(mine, version) else null
        return when {
            c == null || c == 0 -> Line(R.string.version_runs, version, false)
            c < 0 -> Line(R.string.version_runs_older, version, true)
            else -> Line(R.string.version_runs_newer, version, true)
        }
    }

    /** This app's release, as Android knows it ("" when it cannot say). */
    fun mine(context: Context): String =
        runCatching { context.packageManager.getPackageInfo(context.packageName, 0).versionName }
            .getOrNull()
            .orEmpty()
}
