package dev.pocketdesk.android.wake

import org.json.JSONObject

/**
 * A spare phone at home as the wake helper: the rules, pure, for JVM tests.
 * The wire and the listener are the bridge's (`wake_helper.rs`); this decides
 * when the phone serves, what it tells its computer, and which of the
 * computer's words it keeps.
 */
object WakeHelperRule {
    /** Helper phone → its computer: it offers itself, or says it stopped. */
    const val OFFER = "kdeconnect.pocketdesk.wake_helper"

    /** Computer → its helper phone: what to do, for whom. */
    const val DUTY = "kdeconnect.pocketdesk.wake_helper_duty"

    /** Computer → an owner phone: where the helper is. */
    const val INFO = "kdeconnect.pocketdesk.wake_helper_info"

    /** The phone tells its computer again that it is the helper this often. */
    const val OFFER_EVERY_MS = 10 * 60_000L

    /** What the service looks at, and does, this often while it serves. */
    const val TICK_MS = 20_000L

    /** What the helper is doing, for its notification and Settings. */
    enum class State {
        /** The switch is off. */
        OFF,

        /** On, but not on a charger or not on Wi-Fi: nothing listens. */
        WAITING,

        /** Listening for the owner's other phones. */
        SERVING,
    }

    /**
     * [State.SERVING] only with the switch on, on a charger and on Wi-Fi (or
     * Ethernet): mobile data alone never serves, nor does a phone running
     * on its battery.
     */
    fun state(on: Boolean, charging: Boolean, wifi: Boolean): State = when {
        !on -> State.OFF
        charging && wifi -> State.SERVING
        else -> State.WAITING
    }

    /** Whether the foreground service is wanted at all: the switch is on. */
    fun wanted(on: Boolean): Boolean = on

    /**
     * Whether the phone tells its computer about itself now: the first time,
     * when what it says changed ([key] is the helper's key and relays), every
     * [OFFER_EVERY_MS], and when the clock went back.
     */
    fun offerDue(now: Long, lastSentAt: Long?, lastKey: String?, key: String): Boolean =
        lastSentAt == null || lastKey != key || now < lastSentAt || now - lastSentAt >= OFFER_EVERY_MS

    /** The key an offer is compared by. */
    fun offerKey(iroh: String, relays: List<String>): String = iroh + "|" + relays.sorted().joinToString(",")

    /**
     * Whether a duty or an info that arrived from the computer saved at an
     * address is kept: its `computer` must be the id this phone paired that
     * address with. A body naming another computer is dropped, so one
     * computer cannot hand this phone another's duty.
     */
    fun keep(bodyComputer: String?, pairedId: String?): Boolean =
        !bodyComputer.isNullOrEmpty() && pairedId != null && bodyComputer == pairedId

    /** A duty not heard again in this long is dropped: a week. */
    const val DUTY_KEEP_MS = 7 * 24 * 60 * 60_000L

    /**
     * Whether a duty last heard at [at] (ms since the epoch, 0 for never) is
     * still held at [now]. The computer says it again every hour while the
     * phone is linked, so a duty this old is from a computer long out of
     * reach, whose list of phones may be out of date.
     */
    fun dutyHeld(now: Long, at: Long): Boolean = at > 0 && now - at <= DUTY_KEEP_MS

    /** Whether a computer's word says it can give this phone no duty now. */
    fun withdrawn(duty: JSONObject): Boolean = duty.optBoolean("withdrawn", false)

    /** Whether a computer's word says it refuses this phone as its helper. */
    fun refused(duty: JSONObject): Boolean = duty.optBoolean("refused", false)

    /** Whether [infoJson] (a `wake_helper_info` body) names a helper. */
    fun helperKnown(infoJson: String?): Boolean =
        infoJson != null && runCatching { JSONObject(infoJson).optJSONObject("helper") != null }.getOrDefault(false)

    /** The helper's name in [infoJson], as the computer shows it; blank when none. */
    fun helperName(infoJson: String?): String =
        infoJson?.let { json ->
            runCatching { JSONObject(json).optJSONObject("helper")?.optString("name").orEmpty() }.getOrNull()
        }.orEmpty()

    /** What a helper's answer word means for the Wake button. */
    fun outcome(answer: String): Result<Int> = when (answer) {
        "sent" -> Result.success(1)
        "slow_down" ->
            Result.failure(IllegalStateException("the helper was asked a moment ago: try again in half a minute"))
        "unknown" ->
            Result.failure(
                IllegalStateException("the helper does not know this computer: it is told while the computer is awake"),
            )
        else -> Result.failure(IllegalStateException("the helper could not send it"))
    }
}
