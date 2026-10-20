package dev.pocketdesk.android.calls

/**
 * Whether a number the computer sent opens the dialer (L4): "Calls from the
 * computer" on, from the phone's own computer, a number (as the computer's
 * rule: digits, * and #, a + only at the start, 3 to 32), within
 * [PER_MINUTE]. The dialer only shows it; the owner presses Call. Pure, for
 * JVM tests.
 */
object DialRule {
    /** Numbers taken in any minute, at most. */
    const val PER_MINUTE = 10

    enum class Decision { Open, Off, NotOwnComputer, NotANumber, TooMany }

    private val PUNCTUATION = Regex("[ .()-]")
    private val NUMBER = Regex("^\\+?[0-9*#]{2,31}$")

    /** [raw] cleaned as the computer cleans it, or null when it is not a number. */
    fun number(raw: String): String? {
        val kept = raw.replace(PUNCTUATION, "")
        val ok = kept.length in 3..32 && NUMBER.matches(kept) && kept.any { it.isDigit() }
        return kept.takeIf { ok }
    }

    fun decide(settingOn: Boolean, ownComputer: Boolean, number: String?, withinBudget: Boolean): Decision =
        when {
            !settingOn -> Decision.Off
            !ownComputer -> Decision.NotOwnComputer
            number == null -> Decision.NotANumber
            !withinBudget -> Decision.TooMany
            else -> Decision.Open
        }

    /** The dialer's address for [number]: a `#` would end it, so it is escaped. */
    fun telUri(number: String): String = "tel:" + number.replace("#", "%23")
}
