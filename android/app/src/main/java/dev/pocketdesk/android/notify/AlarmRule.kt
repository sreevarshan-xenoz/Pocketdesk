package dev.pocketdesk.android.notify

/**
 * Which of the phone's notifications are a ringing alarm or a finished timer
 * (P5): one the app marks as an alarm (`CATEGORY_ALARM`), or one of the clock
 * app's that cannot be swiped away (ColorOS's ringing alarm has no category,
 * only `FLAG_NO_CLEAR` with Snooze and Stop). The clock app's upcoming-alarm
 * note can be cleared, so it is not one.
 */
object AlarmRule {
    const val CATEGORY_ALARM = "alarm"

    fun isAlarm(category: String?, pkg: String, clockPackages: Set<String>, noClear: Boolean): Boolean =
        category == CATEGORY_ALARM || (pkg in clockPackages && noClear)
}
