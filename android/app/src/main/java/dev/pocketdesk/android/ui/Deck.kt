package dev.pocketdesk.android.ui

import dev.pocketdesk.android.R
import org.json.JSONObject

/**
 * The computer's deck (F1), as the phone shows it: the buttons its owner
 * set up, by index, label and kind. The rules are kept apart from Android
 * so they are tested; [parse] and [outcome] only read the JSON into them.
 */
object Deck {
    /** One button: [index] in the computer's list, pressed with [label]. */
    data class Button(val index: Int, val label: String, val kind: String)

    /**
     * How a press went: [ok], or the computer's [reason]; [waiting] when it
     * waits for the computer's owner (a task that asks first).
     */
    data class Outcome(val ok: Boolean, val reason: String?, val waiting: Boolean = false)

    /** The kinds a button may have; any other is shown as keys. */
    val KINDS = listOf("keys", "task", "media", "link", "text")

    /**
     * Each kind's icon, on the deck sheet and the widget (I2): keys a
     * keyboard, a task a gear, media play, a link the globe, text the clipboard.
     */
    fun iconRes(kind: String): Int = when (kind) {
        "task" -> R.drawable.ic_ll_settings
        "media" -> R.drawable.ic_ll_play
        "link" -> R.drawable.ic_ll_language
        "text" -> R.drawable.ic_ll_clipboard
        else -> R.drawable.ic_ll_keyboard
    }

    /** Buttons a row holds: the owner's choice, 2 to 4. */
    const val DEFAULT_COLUMNS = 3
    fun columns(choice: Int): Int = choice.coerceIn(2, 4)

    /** A listed entry as a button, or null when it cannot be one. */
    fun button(index: Int, label: String, kind: String): Button? {
        if (index < 0 || label.isBlank()) return null
        return Button(index, label, kind.takeIf { it in KINDS } ?: "keys")
    }

    /** A press's answer: ok, or why not (null when the computer gave none). */
    fun outcome(ok: Boolean, reason: String?, waiting: Boolean = false): Outcome =
        Outcome(ok, if (ok) null else reason?.takeIf { it.isNotBlank() }, ok && waiting)

    /** The buttons in rows of [columns], the last row short. */
    fun rows(buttons: List<Button>, columns: Int): List<List<Button>> =
        buttons.chunked(columns(columns))

    /** The buttons in the computer's answer, in its order; bad entries left out. */
    fun parse(json: String): List<Button> {
        val array = JSONObject(json).optJSONArray("buttons") ?: return emptyList()
        return (0 until array.length()).mapNotNull { i ->
            val entry = array.optJSONObject(i) ?: return@mapNotNull null
            button(entry.optInt("index", -1), entry.optString("label"), entry.optString("kind"))
        }
    }

    /** A press's answer as JSON. */
    fun outcome(json: String): Outcome {
        val body = JSONObject(json)
        return outcome(body.optBoolean("ok", false), body.optString("reason"), body.optBoolean("waiting", false))
    }
}
