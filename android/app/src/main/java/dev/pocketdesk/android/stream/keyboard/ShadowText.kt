package dev.pocketdesk.android.stream.keyboard

/** One change to send: delete [backspaces] code points before the cursor, then type [text]. */
data class Edit(val backspaces: Int, val text: String) {
    val isEmpty: Boolean get() = backspaces == 0 && text.isEmpty()
}

/**
 * What the phone's keyboard believes the desktop field holds: the text
 * committed so far (a bounded tail) plus the word being composed, with the
 * cursor always at the end. Every IME call becomes an [Edit] against the
 * composing word, so typing is live, and a swipe or autocorrect that rewrites
 * the word becomes backspaces plus the new tail. Only text this model itself
 * sent is ever diffed, so it cannot drift from the desktop the way a diff of
 * a whole hidden field does.
 *
 * Positions the IME passes in ([recompose]) count from the start of
 * everything ever typed, so trimming the front keeps a count of what it
 * dropped.
 */
class ShadowText(private val keep: Int = 1024) {
    private val committed = StringBuilder()
    private var composing = ""
    private var dropped = 0

    fun all(): String = committed.toString() + composing

    /** The last [n] characters before the cursor. */
    fun before(n: Int): String {
        val text = all()
        return text.substring((text.length - n).coerceAtLeast(0))
    }

    fun setComposing(text: String): Edit {
        val edit = diff(composing, text)
        composing = text
        return edit
    }

    fun commit(text: String): Edit {
        val edit = diff(composing, text)
        composing = ""
        append(text)
        return edit
    }

    fun finishComposing() {
        append(composing)
        composing = ""
    }

    /**
     * Delete [chars] UTF-16 units before the cursor. Deleting past what the
     * model knows still sends a backspace for each: the field may hold text
     * typed before the keyboard opened.
     */
    fun deleteBefore(chars: Int): Edit {
        val text = all()
        var known = chars.coerceIn(0, text.length)
        // Never split a surrogate pair.
        if (known in 1 until text.length && Character.isLowSurrogate(text[text.length - known])) known++
        val removed = text.substring(text.length - known)
        truncate(text.length - known)
        val extra = (chars - known).coerceAtLeast(0)
        return Edit(removed.codePointCount(0, removed.length) + extra, "")
    }

    /** Delete [count] code points before the cursor. */
    fun deleteBeforeCodePoints(count: Int): Edit {
        val text = all()
        var index = text.length
        var known = 0
        while (known < count && index > 0) {
            index = text.offsetByCodePoints(index, -1)
            known++
        }
        truncate(index)
        return Edit(count, "")
    }

    /**
     * Make `[start, end)` the composing word again, as an IME does when the
     * user backspaces into the last word. Only a region that ends at the
     * cursor is taken; anything else is ignored.
     */
    fun recompose(start: Int, end: Int): Boolean {
        val text = all()
        val localStart = start - dropped
        if (end - dropped != text.length || localStart !in 0..text.length) return false
        committed.setLength(0)
        committed.append(text, 0, localStart)
        composing = text.substring(localStart)
        return true
    }

    /**
     * Forget everything. Always paired with an IME restart, which counts its
     * positions from 0 again, so the offset starts over too.
     */
    fun clear() {
        dropped = 0
        committed.setLength(0)
        composing = ""
    }

    /** The cursor, counted the way the IME counts it. */
    fun cursor(): Int = dropped + committed.length + composing.length

    private fun append(text: String) {
        committed.append(text)
        if (committed.length > keep) {
            var cut = committed.length - keep
            if (Character.isLowSurrogate(committed[cut])) cut++
            committed.delete(0, cut)
            dropped += cut
        }
    }

    /** Keep the first [length] characters of everything. */
    private fun truncate(length: Int) {
        if (length >= committed.length) {
            composing = composing.substring(0, length - committed.length)
        } else {
            committed.setLength(length)
            composing = ""
        }
    }

    private fun diff(old: String, new: String): Edit {
        var common = 0
        val max = minOf(old.length, new.length)
        while (common < max && old[common] == new[common]) common++
        // A shared high surrogate is not a shared character.
        if (common > 0 && Character.isHighSurrogate(old[common - 1])) common--
        val removed = old.substring(common)
        return Edit(removed.codePointCount(0, removed.length), new.substring(common))
    }
}
