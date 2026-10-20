package dev.pocketdesk.android.calls

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.SystemClock
import android.util.Log
import androidx.core.net.toUri
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.notify.NotificationActionRule
import java.util.concurrent.atomic.AtomicInteger

/**
 * Numbers the computer asks this phone to dial (L4): the dialer opens with
 * one (in front), or a notification offers it (in the background, where
 * Android blocks opening it). Never a call: the owner presses Call.
 */
object Dials {
    private const val CHANNEL = "pocketdesk-dials"
    private val ids = AtomicInteger(0)
    private val budget = NotificationActionRule.Budget(DialRule.PER_MINUTE)

    /**
     * Take the numbers waiting; [own] is the computer the phone works with.
     * [inFront] opens the dialer, else each is offered in a notification.
     */
    fun take(context: Context, own: HostStore.Host?, inFront: Boolean) {
        val dials = RustCore.takeDials()
        if (dials.isEmpty()) return
        val on = Prefs.callsFromComputer(context)
        for (dial in dials) {
            val ownComputer = own != null && own.address == dial.host && own.controlPort == dial.port
            val number = DialRule.number(dial.number)
            var decision = DialRule.decide(on, ownComputer, number, withinBudget = true)
            if (decision == DialRule.Decision.Open && !budget.take(SystemClock.elapsedRealtime())) {
                decision = DialRule.Decision.TooMany
            }
            if (decision != DialRule.Decision.Open || number == null) {
                Log.i("Dials", "a number from the computer was not taken: $decision")
                continue
            }
            if (inFront) open(context, number) else offer(context, own!!.address, number)
        }
    }

    private fun intent(number: String): Intent =
        Intent(Intent.ACTION_DIAL, DialRule.telUri(number).toUri()).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

    private fun open(context: Context, number: String) {
        runCatching { context.startActivity(intent(number)) }
            .onFailure { Log.w("Dials", "no dialer: ${it.message}") }
    }

    private fun offer(context: Context, address: String, number: String) {
        val manager = context.getSystemService(NotificationManager::class.java)
        if (manager.getNotificationChannel(CHANNEL) == null) {
            manager.createNotificationChannel(
                NotificationChannel(CHANNEL, context.getString(R.string.channel_dials), NotificationManager.IMPORTANCE_HIGH),
            )
        }
        val id = ids.incrementAndGet()
        val pending =
            PendingIntent.getActivity(
                context,
                id,
                intent(number),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )
        val notification =
            Notification.Builder(context, CHANNEL)
                .setSmallIcon(R.drawable.ic_stat_pocketdesk)
                .setContentTitle(context.getString(R.string.dial_from_computer, number))
                .setContentText(context.getString(R.string.dial_from_computer_text, HostStore.shownName(context, address)))
                .setContentIntent(pending)
                .setAutoCancel(true)
                .build()
        runCatching { manager.notify("dial", id, notification) }
    }
}
