package dev.pocketdesk.android.notify

import android.content.Context
import androidx.core.content.edit

/**
 * Every computer of mine (L8): the phone's notifications also go to the
 * other paired computers the owner picks, as well as to the one it uses
 * most recently. A computer the phone keeps a link to (part 2) may answer
 * and clear them; one it does not gets a copy without what offers an answer.
 * Kept apart from Android so it is tested.
 */
object AlsoTo {
    /** The most other computers a notification also goes to. */
    const val MAX = 3

    /** The most picks: the own computer's, which waits for when another is own, and [MAX] more. */
    const val MAX_PICKS = MAX + 1

    /** What in a notification would offer an answer the phone would refuse from them (D4's thread among them). */
    // N3 review: an open too, which a copy's computer cannot send back.
    val ANSWERS = listOf("actions", "requestReplyId", "messages", "canOpen")

    /** A saved computer: its address and the paired desktop id it has, if paired. */
    data class Saved(val address: String, val desktopId: String?)

    /**
     * The other computers a notification goes to: saved (in [saved]'s
     * order), paired, picked by their paired id (so a computer later at the
     * same address is never one), never the phone's own ([own], by address
     * or by its paired id: one computer saved at two addresses is one), each
     * computer once, at most [MAX].
     */
    fun targets(saved: List<Saved>, picked: Set<String>, own: String?): List<String> {
        val ownId = saved.firstOrNull { it.address == own }?.desktopId
        return saved
            .filter { it.address != own && it.desktopId != null && it.desktopId != ownId && it.desktopId in picked }
            .distinctBy { it.desktopId }
            .map { it.address }
            .take(MAX)
    }

    /** Whether one more computer may be picked, with [picked] already. */
    fun mayPickMore(picked: Int): Boolean = picked < MAX_PICKS

    /**
     * The picks of computers still saved ([savedIds], their paired ids):
     * what counts toward the limit (AX3: a removed computer's, unlisted,
     * held one of the four; so does an id a computer paired anew left).
     */
    fun pickedSaved(picked: Set<String>, savedIds: Set<String>): Set<String> = picked intersect savedIds

    /**
     * The picks once the card of the computer [removed] (its paired id) is
     * gone, [stillSaved] the paired ids of the cards left: its pick goes,
     * unless another card is the same computer (saved at two addresses).
     */
    fun pickedAfterRemove(picked: Set<String>, removed: String?, stillSaved: Set<String>): Set<String> =
        if (removed == null || removed in stillSaved) picked else picked - removed
}

/** Which computers get the phone's notifications too (L8), by paired id, off for each by default. */
object AlsoToPrefs {
    private const val PREFS = "pocketdesk-also-to"
    private const val KEY = "desktop_ids"

    fun picked(ctx: Context): Set<String> =
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getStringSet(KEY, emptySet()).orEmpty().toSet()

    fun set(ctx: Context, desktopId: String, on: Boolean) {
        val now = picked(ctx).let { if (on) it + desktopId else it - desktopId }
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit { putStringSet(KEY, now) }
    }

    /** A card removed (AX3): its computer's pick goes, as [AlsoTo.pickedAfterRemove] says. */
    fun forgetRemoved(ctx: Context, removed: String?, stillSaved: Set<String>) {
        val before = picked(ctx)
        val now = AlsoTo.pickedAfterRemove(before, removed, stillSaved)
        if (now != before) ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit { putStringSet(KEY, now) }
    }
}
