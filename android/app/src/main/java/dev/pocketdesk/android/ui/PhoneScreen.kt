package dev.pocketdesk.android.ui

/**
 * The computer's answer to "add a screen for this phone" (T10j): the
 * screen's monitor index, or why not. Pure, for JVM tests.
 */
object PhoneScreen {
    sealed interface Answer {
        data class Added(val index: Int) : Answer

        data class Refused(val reason: String) : Answer
    }

    private val INDEX = Regex("\"index\"\\s*:\\s*(\\d+)")
    private val ERROR = Regex("\"error\"\\s*:\\s*\"((?:[^\"\\\\]|\\\\.)*)\"")

    fun parse(json: String): Answer {
        ERROR.find(json)?.let { return Answer.Refused(it.groupValues[1].replace("\\\"", "\"")) }
        val index = INDEX.find(json)?.groupValues?.get(1)?.toIntOrNull()
        return if (index != null) Answer.Added(index) else Answer.Refused("")
    }
}
