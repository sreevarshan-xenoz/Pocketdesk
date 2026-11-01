package dev.pocketdesk.android.location

import kotlin.math.abs
import kotlin.math.max

/**
 * What this phone tells its own computer about where it is (GeoClue): the
 * rules the answer is held to, apart from the Android calls so they are
 * tested on the JVM. Nothing here stores or logs a place.
 */
object PhoneLocationRule {
    /** A city fix is rounded to this many degrees: about 1.1 km north to south. */
    const val CITY_STEP_DEGREES = 0.01

    /** The least accuracy a city fix claims, in metres. */
    const val CITY_ACCURACY_M = 1000

    /** The most a fix may say it is off by, in metres. */
    const val WORST_ACCURACY_M = 100_000

    /** The least time between two fixes sent, in milliseconds (the computer asks for one every ten minutes). */
    const val ANSWER_EVERY_MS = 30_000L

    /** How close a fix is given, as the computer's ask and the wire name it. */
    enum class Precision(val wire: String) {
        CITY("city"),
        EXACT("exact"),
        ;

        companion object {
            /** City for anything but "exact", so a word this version does not know never gives more. */
            fun of(name: String?): Precision = if (name?.trim().equals("exact", ignoreCase = true)) EXACT else CITY
        }
    }

    /** One place as it goes in the answer: degrees and how far off it may be. */
    data class Fix(val lat: Double, val lon: Double, val accuracyM: Int, val precision: Precision)

    /**
     * The precision to give: exact only when the computer asked for it and
     * this phone's owner allowed it (and, for the caller, the permission
     * to be exact is granted).
     */
    fun heldTo(asked: Precision, exactAllowed: Boolean): Precision =
        if (asked == Precision.EXACT && exactAllowed) Precision.EXACT else Precision.CITY

    /** [value] rounded to [CITY_STEP_DEGREES], the nearest number to the decimal. */
    fun round(value: Double): Double {
        val perDegree = Math.round(1.0 / CITY_STEP_DEGREES).toDouble()
        return Math.round(value * perDegree) / perDegree
    }

    /**
     * The fix to send for a reading: city rounds both numbers and claims at
     * least [CITY_ACCURACY_M]; exact keeps the reading. A reading that is
     * not a place (not a number, past the poles or the date line) is null.
     */
    fun fixFor(lat: Double, lon: Double, accuracyM: Float, precision: Precision): Fix? {
        if (!lat.isFinite() || !lon.isFinite() || abs(lat) > 90.0 || abs(lon) > 180.0) return null
        val accuracy =
            if (accuracyM.isFinite() && accuracyM >= 0f) {
                Math.ceil(accuracyM.toDouble()).coerceIn(1.0, WORST_ACCURACY_M.toDouble()).toInt()
            } else {
                WORST_ACCURACY_M
            }
        return when (precision) {
            Precision.EXACT -> Fix(lat, lon, accuracy, precision)
            Precision.CITY -> Fix(round(lat), round(lon), max(accuracy, CITY_ACCURACY_M), precision)
        }
    }

    /**
     * Why this phone does not answer with a place, as the computer is told
     * ("off", "permission"), or null when it may. Only the phone's own
     * computer is answered at all, which the caller checks.
     */
    fun refusal(on: Boolean, coarseGranted: Boolean, fineGranted: Boolean): String? =
        when {
            !on -> "off"
            !coarseGranted && !fineGranted -> "permission"
            else -> null
        }

    /** Whether a fix is given at the closer precision: it needs the owner's choice and the fine permission. */
    fun exactAllowed(ownerAllows: Boolean, fineGranted: Boolean): Boolean = ownerAllows && fineGranted

    /**
     * What to tell the computer when the system gave no place: "background"
     * when Android hides location from an app that is not on screen and
     * has no all-the-time permission, else "unavailable".
     */
    fun noFix(backgroundGranted: Boolean, appOnScreen: Boolean): String =
        if (!backgroundGranted && !appOnScreen) "background" else "unavailable"

    /** Whether another fix may go at [now]: not within [ANSWER_EVERY_MS] of the last one. */
    fun mayAnswer(lastMs: Long, now: Long): Boolean = lastMs == 0L || now - lastMs >= ANSWER_EVERY_MS
}
