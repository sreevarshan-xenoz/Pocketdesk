package dev.pocketdesk.android.automation

/**
 * Whether a command from the computer goes to the phone's automation apps
 * (L6): "Commands from the computer to my automation apps" on, from the
 * phone's own computer, within [PER_MINUTE]. Pure, for JVM tests.
 */
object CommandRule {
    /** Commands taken in any minute, at most. */
    const val PER_MINUTE = 30

    /** The broadcast's action. */
    const val ACTION = "dev.pocketdesk.android.event.COMMAND"

    /** The automation apps "Only to" may name, in the order a default is picked. */
    val KNOWN = listOf(
        "net.dinglisch.android.taskerm",
        "com.llamalab.automate",
        "com.arlosoft.macrodroid",
    )

    enum class Decision { Send, Off, NotOwnComputer, TooMany }

    fun decide(settingOn: Boolean, ownComputer: Boolean, withinBudget: Boolean): Decision =
        when {
            !settingOn -> Decision.Off
            !ownComputer -> Decision.NotOwnComputer
            !withinBudget -> Decision.TooMany
            else -> Decision.Send
        }

    /**
     * The package the broadcast goes to: the one chosen if it is installed,
     * else the first known one installed; null for none (any app may hear it).
     */
    fun onlyTo(chosen: String?, installed: Set<String>): String? =
        chosen?.takeIf { it in installed } ?: KNOWN.firstOrNull { it in installed }
}
