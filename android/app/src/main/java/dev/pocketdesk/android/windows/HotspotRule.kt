package dev.pocketdesk.android.windows

import dev.pocketdesk.android.status.SwitchReport

/**
 * Let my computers turn on my hotspot: whether this phone turns its own Wi-Fi
 * hotspot on or off when its computer asks, and what the helper process may
 * run to do it. Pure, for JVM tests; [dev.pocketdesk.android.control.PhoneActions]
 * and [PhoneHotspot] do it.
 *
 * It is optional and off by default, and rides Tier 2's requirement: Shizuku
 * installed and running and this app allowed in it (the owner's choice, never
 * asked for here). It adds no Android permission and the computer sends nothing
 * but the word "on" or "off": no network name, password or band. The phone
 * starts its hotspot with the settings the owner already saved in Android.
 *
 * A locked phone may still turn it on: a hotspot shows no screen content and
 * opens nothing on the phone, Android shows its own hotspot indicator, and
 * wanting internet on the computer is most often just when the phone is in a
 * pocket. The rest of the gate still applies (the switch, Shizuku, the phone's
 * own computer and a rate limit).
 */
object HotspotRule {
    /** Android 11: the first with the tethering manager the helper calls. */
    const val MIN_SDK = 30

    /** How many asks the phone takes in [LIMIT_WINDOW_MS]; on and off both count. */
    const val LIMIT_ASKS = 8
    const val LIMIT_WINDOW_MS = 10 * 60_000L

    /** The auto-off choices in minutes (0 = never), and the default. */
    val OFF_CHOICES = listOf(15, 30, 60, 120, 0)
    const val DEFAULT_OFF_MINUTES = 30

    /**
     * What the helper runs to stop the hotspot when the tethering manager
     * does not answer: fixed words, no shell, and nothing from any caller. It
     * stops the soft access point and nothing else. (Starting needs the
     * tethering manager: `cmd wifi start-softap` wants a name and password
     * on its command line, which this app never passes.)
     */
    val STOP_ARGV: List<String> = listOf("/system/bin/cmd", "wifi", "stop-softap")

    /** Where hotspots stand on this phone. */
    enum class Mode { TOO_OLD, NO_SHIZUKU, NEEDS_PERMISSION, OFF, ON }

    fun mode(sdk: Int, shizukuRunning: Boolean, permitted: Boolean, switchOn: Boolean): Mode = when {
        sdk < MIN_SDK -> Mode.TOO_OLD
        !shizukuRunning -> Mode.NO_SHIZUKU
        !switchOn -> Mode.OFF
        !permitted -> Mode.NEEDS_PERMISSION
        else -> Mode.ON
    }

    /** Whether the Settings row is shown: with Shizuku running, or while the switch is on so it can be turned off. */
    fun rowShown(sdk: Int, shizukuRunning: Boolean, switchOn: Boolean): Boolean =
        sdk >= MIN_SDK && (shizukuRunning || switchOn)

    /** What the computer is told in the switches report (feature `hotspot`): the owner's switch, and whether Shizuku and Android let it work. */
    fun report(sdk: Int, shizukuRunning: Boolean, permitted: Boolean, switchOn: Boolean): SwitchReport.Switch =
        SwitchReport.Switch(switchOn, sdk >= MIN_SDK && shizukuRunning && permitted)

    /** The asks the computer may make. */
    enum class Ask { ON, OFF }

    enum class Decision {
        /** Do it. */
        DO,

        /** The owner's switch is off (for an ask to turn it on). */
        OFF,

        /** Not this phone's own computer. */
        NOT_OWN,

        /** Android is too old, or Shizuku is not running or not allowed. */
        UNAVAILABLE,

        /** More asks than [LIMIT_ASKS] in [LIMIT_WINDOW_MS]. */
        TOO_MANY,
    }

    /**
     * What to do with [ask]: [mode] where hotspots stand, [own] whether it is
     * the phone's own computer, [withinLimit] whether the rate limit allows
     * another. The lock is not asked (see the class). Turning it off is let
     * through with the switch off, so an owner who turned the switch off is
     * never left with a hotspot the computer cannot end; it still needs
     * Shizuku.
     */
    fun decide(ask: Ask, mode: Mode, own: Boolean, withinLimit: Boolean): Decision = when {
        !own -> Decision.NOT_OWN
        mode == Mode.TOO_OLD || mode == Mode.NO_SHIZUKU -> Decision.UNAVAILABLE
        ask == Ask.ON && mode == Mode.OFF -> Decision.OFF
        mode == Mode.NEEDS_PERMISSION -> Decision.UNAVAILABLE
        !withinLimit -> Decision.TOO_MANY
        else -> Decision.DO
    }

    /** Whether to turn it off now: it was turned on [sinceMs] ago and [minutes] (0 = never) have passed. */
    fun offDue(onAtMs: Long, nowMs: Long, minutes: Int): Boolean =
        onAtMs > 0 && minutes > 0 && nowMs - onAtMs >= minutes * 60_000L

    /** The next choice after [minutes], going round. */
    fun nextChoice(minutes: Int): Int {
        val at = OFF_CHOICES.indexOf(minutes)
        return OFF_CHOICES[(at + 1) % OFF_CHOICES.size]
    }

    /** A sliding window of recent asks, for the limit. Not thread-safe by itself: [take] is synchronized. */
    class Limiter(private val max: Int = LIMIT_ASKS, private val windowMs: Long = LIMIT_WINDOW_MS) {
        private val times = ArrayDeque<Long>()

        @Synchronized
        fun take(nowMs: Long): Boolean {
            while (times.isNotEmpty() && nowMs - times.first() >= windowMs) times.removeFirst()
            if (times.size >= max) return false
            times.addLast(nowMs)
            return true
        }
    }
}
