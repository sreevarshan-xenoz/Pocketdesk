package dev.pocketdesk.android.control

import org.json.JSONObject

/**
 * What the phone says of an ask to list its apps or open one (Apps part 1),
 * and how an open that asked for the screen goes. Pure, for JVM tests;
 * [PhoneActions] does it. The words are the computer's
 * `core::protocol::phone_action::AppResult`.
 */
object AppOpenRule {
    const val OPENED = "opened"
    const val NEEDS_SCREEN = "needs_screen"
    const val CONTROL_OFF = "control_off"
    const val LOCKED = "locked"
    const val ASLEEP = "asleep"
    const val OFF = "off"
    const val NOT_LISTED = "not_listed"
    const val NOT_FOUND = "not_found"
    const val TOO_MANY = "too_many"
    const val FAILED = "failed"

    /**
     * The word for a refused ask, or null when nothing is said: a computer
     * that is not the phone's own is never answered, so it learns nothing
     * of the switch or the lock.
     */
    fun refusal(decision: PhoneActionRule.Decision): String? = when (decision) {
        PhoneActionRule.Decision.DO, PhoneActionRule.Decision.NOT_OWN -> null
        PhoneActionRule.Decision.OFF -> OFF
        PhoneActionRule.Decision.LOCKED -> LOCKED
        PhoneActionRule.Decision.ASLEEP -> ASLEEP
        PhoneActionRule.Decision.NOT_LISTED -> NOT_LISTED
        PhoneActionRule.Decision.TOO_MANY -> TOO_MANY
        else -> FAILED
    }

    /** [refusal], said only to the phone's own computer ([own]). */
    fun tell(decision: PhoneActionRule.Decision, own: Boolean): String? = if (own) refusal(decision) else null

    /** What an allowed open does. */
    enum class Plan {
        /** Start the app now. */
        START,

        /** The screen is not shown there yet: offer it once, then start. */
        SHOW_SCREEN_FIRST,

        /** Pocketdesk's control service is off, and the computer asked for it. */
        CONTROL_OFF,

        /** As P2: start it at once with the service, else offer "Tap to open". */
        TAP,
    }

    /**
     * How an open that [PhoneActionRule] allowed goes. [screenWanted]: the
     * computer asked for the phone's screen too; [service]: the control
     * service is bound; [casting]: the screen is already shown to that
     * computer, so it is reused and no new consent is asked.
     */
    fun plan(screenWanted: Boolean, service: Boolean, casting: Boolean): Plan = when {
        !screenWanted -> Plan.TAP
        !service -> Plan.CONTROL_OFF
        !casting -> Plan.SHOW_SCREEN_FIRST
        else -> Plan.START
    }

    /** The body of the report to the computer: `{about, app?, result}`. */
    fun report(about: String, app: String?, result: String): JSONObject =
        JSONObject().put("about", about).apply { if (app != null) put("app", app) }.put("result", result)
}
