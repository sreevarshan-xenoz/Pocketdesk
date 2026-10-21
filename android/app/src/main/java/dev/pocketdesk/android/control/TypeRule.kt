package dev.pocketdesk.android.control

/**
 * Typing from the computer's keyboard (M4): whether a batch the computer
 * sent is typed here.
 */
object TypeRule {
    /** How long Stop on the notification refuses typing. */
    const val STOP_MS = 10 * 60_000L

    /** How long after the last typing its notification stays. */
    const val NOTICE_MS = 30_000L

    enum class Decision { TYPE, OFF, SERVICE_OFF, NOT_OWN_COMPUTER, STOPPED }

    /**
     * Type it? With "Typing from the computer's keyboard" [on] (its own
     * switch, M4 review: C8's lets a cast's window drive the phone, which is
     * not this), C8's Accessibility [service] running, from the phone's [own] computer, and
     * not within Stop's ten minutes.
     */
    fun decide(on: Boolean, service: Boolean, own: Boolean, stoppedUntilMs: Long, nowMs: Long): Decision = when {
        !on -> Decision.OFF
        !own -> Decision.NOT_OWN_COMPUTER
        !service -> Decision.SERVICE_OFF
        nowMs < stoppedUntilMs -> Decision.STOPPED
        else -> Decision.TYPE
    }
}
