package dev.pocketdesk.android.notify

/**
 * Which of the phone's notifications are a ringing alarm or a finished timer
 * (P5): one the app marks as an alarm (`CATEGORY_ALARM`), or one of the clock
 * app's that cannot be swiped away (ColorOS's ringing alarm has no category,
 * only `FLAG_NO_CLEAR` with Snooze and Stop). The clock app's upcoming-alarm
 * note can be cleared, so it is not one, nor is a running timer or
 * stopwatch, which counts on its screen (P5 review: it would ring the
 * computer at every tick).
 */
object AlarmRule {
    const val CATEGORY_ALARM = "alarm"

    fun isAlarm(
        category: String?,
        pkg: String,
        clockPackages: Set<String>,
        noClear: Boolean,
        counting: Boolean = false,
    ): Boolean = category == CATEGORY_ALARM || (pkg in clockPackages && noClear && !counting)
}
