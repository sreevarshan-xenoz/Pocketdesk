package dev.pocketdesk.android.ask

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.util.Log
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore

/**
 * Questions the computer puts to this phone (BF3): each is offered in a
 * notification that opens [AskActivity], only from the computer the phone
 * works with, while the question waits. Its words show on the lock screen
 * only as "A question from the computer".
 */
object Asks {
    private const val TAG = "Asks"
    private const val CHANNEL = "pocketdesk-questions"
    private const val NOTIFICATION_TAG = "ask"

    const val EXTRA_HOST = "host"
    const val EXTRA_PORT = "port"
    const val EXTRA_ID = "id"
    const val EXTRA_QUESTION = "question"
    const val EXTRA_EXPIRES = "expires_unix"

    /** Take the questions waiting; [own] is the computer the phone works with. */
    fun take(context: Context, own: HostStore.Host?) {
        val asks = RustCore.takeAsks()
        if (asks.isEmpty()) return
        val on = Prefs.questionsFromComputer(context)
        val now = System.currentTimeMillis() / 1000
        for (ask in asks) {
            val ownComputer = own != null && own.address == ask.host && own.controlPort == ask.port
            val decision = AskRule.decide(on, ownComputer, ask.expiresUnix, now)
            if (decision != AskRule.Decision.Offer) {
                Log.i(TAG, "a question from the computer was not offered: $decision")
                continue
            }
            offer(context, ask, now)
            // For the device checks; never the question.
            Log.i(TAG, "a question from the computer was offered")
        }
    }

    /** Take the question [id]'s notification away: it was answered. */
    fun cancel(context: Context, id: String) {
        context.getSystemService(NotificationManager::class.java).cancel(NOTIFICATION_TAG, id.hashCode())
    }

    private fun offer(context: Context, ask: RustCore.AskQuestion, now: Long) {
        val manager = context.getSystemService(NotificationManager::class.java)
        if (manager.getNotificationChannel(CHANNEL) == null) {
            manager.createNotificationChannel(
                NotificationChannel(CHANNEL, context.getString(R.string.channel_questions), NotificationManager.IMPORTANCE_HIGH),
            )
        }
        val computer = HostStore.shownName(context, ask.host)
        val open = Intent(context, AskActivity::class.java)
            .putExtra(EXTRA_HOST, ask.host)
            .putExtra(EXTRA_PORT, ask.port)
            .putExtra(EXTRA_ID, ask.id)
            .putExtra(EXTRA_QUESTION, ask.question)
            .putExtra(EXTRA_EXPIRES, ask.expiresUnix)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        val pending = PendingIntent.getActivity(
            context,
            ask.id.hashCode(),
            open,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val title = context.getString(R.string.ask_from_computer, computer)
        val hidden = Notification.Builder(context, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat_pocketdesk)
            .setContentTitle(title)
            .build()
        val notification = Notification.Builder(context, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat_pocketdesk)
            .setContentTitle(title)
            .setContentText(ask.question)
            .setStyle(Notification.BigTextStyle().bigText(ask.question))
            .setContentIntent(pending)
            .setVisibility(Notification.VISIBILITY_PRIVATE)
            .setPublicVersion(hidden)
            .setTimeoutAfter(((ask.expiresUnix - now) * 1000).coerceAtLeast(1_000))
            .build()
        runCatching { manager.notify(NOTIFICATION_TAG, ask.id.hashCode(), notification) }
            .onFailure { Log.w(TAG, "a question was not offered: ${it.message}") }
        if (!manager.areNotificationsEnabled()) Log.w(TAG, "a question was not offered: notifications are off")
    }
}
