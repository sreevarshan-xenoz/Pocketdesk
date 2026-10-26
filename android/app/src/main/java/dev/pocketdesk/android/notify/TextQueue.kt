package dev.pocketdesk.android.notify

/**
 * Which kept texts go to the computer now (BC4): each was kept for the own
 * computer of the moment it arrived (its paired id, "" when none was
 * paired), and goes to that computer alone. Texts kept for one since
 * removed are dropped, not handed to the next. Pure, for JVM tests.
 */
object TextQueue {
    /** The key a kept text names its computer by. */
    const val FOR = "for"

    /** Texts sent at once, at most. */
    const val BATCH = 50

    /** Indexes of the kept texts to send now and to drop. */
    data class Split(val send: List<Int>, val drop: List<Int>)

    /**
     * From each kept text's computer ([fors]), the own computer now
     * ([own], its paired id, null when there is none) and the paired ids
     * still saved ([saved]).
     */
    fun split(fors: List<String>, own: String?, saved: Set<String>): Split {
        val send = mutableListOf<Int>()
        val drop = mutableListOf<Int>()
        fors.forEachIndexed { i, kept ->
            when {
                kept.isNotEmpty() && kept !in saved -> drop += i
                own != null && (kept.isEmpty() || kept == own) && send.size < BATCH -> send += i
            }
        }
        return Split(send, drop)
    }
}
