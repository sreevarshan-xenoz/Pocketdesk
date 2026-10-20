package dev.pocketdesk.android.notify

import android.content.Context
import androidx.core.content.edit
import org.json.JSONObject

/**
 * Every computer of mine, part 1 (L8): the phone's notifications and texts
 * also go to the other paired computers the owner picks, as well as to the
 * one it uses most recently. Those get them to see, never to answer: the
 * phone takes answers and clears from its own computer only, so what would
 * offer one is left out. Kept apart from Android so it is tested.
 */
object AlsoTo {
    /** The most other computers a notification also goes to. */
    const val MAX = 3

    /** What is copied to them. */
    private val COPIED = setOf("kdeconnect.notification", "kdeconnect.sms.messages")

    /** What would offer an answer the phone would refuse from them. */
    val ANSWERS = listOf("actions", "requestReplyId", "messages")

    /**
     * The other computers [packetType] goes to: saved (in [saved]'s order),
     * paired, picked, not the phone's own ([own]), at most [MAX].
     */
    fun targets(
        packetType: String,
        saved: List<String>,
        paired: (String) -> Boolean,
        picked: Set<String>,
        own: String?,
    ): List<String> {
        if (packetType !in COPIED) return emptyList()
        return saved.distinct().filter { it != own && it in picked && paired(it) }.take(MAX)
    }

    /** [body] as another computer gets it: without what offers an answer. */
    fun copyFor(body: JSONObject): JSONObject {
        val copy = JSONObject(body.toString())
        ANSWERS.forEach { copy.remove(it) }
        return copy
    }

    /**
     * What tells one copy from another, so a notification sent again after
     * a wait (C3) is copied once: the body without how long it waited.
     */
    fun key(packetType: String, body: JSONObject): String {
        val copy = JSONObject(body.toString())
        copy.remove("waited")
        return packetType + copy.toString()
    }
}

/** Which other computers get the phone's notifications and texts too (L8), off for each by default. */
object AlsoToPrefs {
    private const val PREFS = "pocketdesk-also-to"
    private const val KEY = "addresses"

    fun picked(ctx: Context): Set<String> =
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getStringSet(KEY, emptySet()).orEmpty().toSet()

    fun set(ctx: Context, address: String, on: Boolean) {
        val now = picked(ctx).let { if (on) it + address else it - address }
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit { putStringSet(KEY, now) }
    }
}
