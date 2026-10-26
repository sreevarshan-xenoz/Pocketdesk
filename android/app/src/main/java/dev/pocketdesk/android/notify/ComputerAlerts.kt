package dev.pocketdesk.android.notify

import android.content.Context
import android.util.Log
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.ui.postDesktopNotification

/**
 * Alerts from `pocketdesk done` (BF1): the computer says a command it
 * watched has ended. Shown as its other notifications are, but with no
 * session open, from the background link; with the switch off, dropped.
 */
object ComputerAlerts {
    private const val TAG = "ComputerAlerts"

    /** Show the alerts waiting from the computer at [address]. Blocking: off the main thread. */
    fun take(context: Context, address: String, controlPort: Int) {
        val alerts = RustCore.takePendingAlerts()
        if (alerts.isEmpty()) return
        if (!Prefs.computerAlerts(context)) {
            Log.i(TAG, "${alerts.size} alert(s) from the computer dropped: the switch is off")
            return
        }
        Log.i(TAG, "Showing ${alerts.size} alert(s) from the computer")
        for (alert in alerts) {
            postDesktopNotification(context, address, controlPort, alert, replyable = false)
        }
    }
}
