package dev.pocketdesk.android.ui

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.RemoteInput
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.platform.LocalContext
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.DesktopNotification
import dev.pocketdesk.android.bridge.RustCore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withContext
import java.util.concurrent.atomic.AtomicInteger

/**
 * Desktop → phone notification relay with inline reply (R3 Tier-2 #11c).
 * The bridge queues `kdeconnect.notification` pushes off the control
 * channel; this poller posts them as Android notifications whose reply
 * action sends `kdeconnect.pocketdesk.notification_reply` back to the desktop.
 */

const val CHANNEL_DESKTOP_NOTIFICATIONS = "pocketdesk-desktop-notifications"
const val EXTRA_REPLY_ADDRESS = "reply_address"
const val EXTRA_REPLY_PORT = "reply_port"
const val EXTRA_REPLY_ID = "reply_id"
const val EXTRA_REPLY_TAG = "reply_tag"
const val KEY_REPLY = "reply_text"
const val TAG_DESKTOP_NOTIFICATION = "desktop-notification"
private const val ACTION_DESKTOP_NOTIFICATION_REPLY =
    "dev.pocketdesk.android.action.REPLY_DESKTOP_NOTIFICATION"

private val replyRequestCounter = AtomicInteger(0)

@Composable
fun DesktopNotificationRelay(address: String, controlPort: Int) {
    val context = LocalContext.current
    LaunchedEffect(address, controlPort) {
        ensureDesktopNotificationChannel(context)
        while (isActive) {
            val notes = withContext(Dispatchers.IO) { RustCore.takePendingNotifications() }
            for (note in notes) {
                postDesktopNotification(context, address, controlPort, note)
            }
            // BF1: alerts too, as the background link does without a session.
            withContext(Dispatchers.IO) {
                dev.pocketdesk.android.notify.ComputerAlerts.take(context, address, controlPort)
            }
            delay(2_000)
        }
    }
}

private fun ensureDesktopNotificationChannel(context: Context) {
    val manager = context.getSystemService(NotificationManager::class.java)
    if (manager.getNotificationChannel(CHANNEL_DESKTOP_NOTIFICATIONS) != null) return
    val channel = NotificationChannel(
        CHANNEL_DESKTOP_NOTIFICATIONS,
        context.getString(R.string.channel_desktop_notifications),
        NotificationManager.IMPORTANCE_DEFAULT,
    )
    manager.createNotificationChannel(channel)
}

internal fun postDesktopNotification(
    context: Context,
    address: String,
    controlPort: Int,
    note: DesktopNotification,
) {
    // Posted from the background link too (BF1), before any session made it.
    ensureDesktopNotificationChannel(context)
    val title = note.title.ifBlank { note.app.ifBlank { note.source } }
    val key = note.id.hashCode()

    val reply = Intent(context, NotificationReplyReceiver::class.java)
        .setAction(ACTION_DESKTOP_NOTIFICATION_REPLY)
        .putExtra(EXTRA_REPLY_ADDRESS, address)
        .putExtra(EXTRA_REPLY_PORT, controlPort)
        .putExtra(EXTRA_REPLY_ID, note.id)
        .putExtra(EXTRA_REPLY_TAG, TAG_DESKTOP_NOTIFICATION)
    val replyIntent = PendingIntent.getBroadcast(
        context,
        replyRequestCounter.incrementAndGet(),
        reply,
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )
    val remoteInput = RemoteInput.Builder(KEY_REPLY)
        .setLabel(context.getString(R.string.reply_to, note.source))
        .build()
    val action = Notification.Action.Builder(
        null,
        context.getString(R.string.reply),
        replyIntent,
    )
        .addRemoteInput(remoteInput)
        .setAllowGeneratedReplies(true)
        .build()

    val notification = Notification.Builder(context, CHANNEL_DESKTOP_NOTIFICATIONS)
        .setSmallIcon(android.R.drawable.stat_notify_chat)
        .setContentTitle(title)
        .setContentText(note.text)
        .setStyle(Notification.BigTextStyle().bigText(note.text))
        .setTicker(note.text)
        .setAutoCancel(true)
        .addAction(action)
        .build()

    context.getSystemService(NotificationManager::class.java)
        .notify(TAG_DESKTOP_NOTIFICATION, key, notification)
}

/** Handles the RemoteInput result: sends the reply to the desktop. */
class NotificationReplyReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val reply = RemoteInput.getResultsFromIntent(intent)?.getCharSequence(KEY_REPLY)
        val address = intent.getStringExtra(EXTRA_REPLY_ADDRESS)
        val port = intent.getIntExtra(EXTRA_REPLY_PORT, 1716)
        val id = intent.getStringExtra(EXTRA_REPLY_ID)
        val tag = intent.getStringExtra(EXTRA_REPLY_TAG) ?: TAG_DESKTOP_NOTIFICATION
        if (reply == null || address == null || id == null) return
        val text = reply.toString()
        // Cancel the notification immediately; the reply itself is fire-and-
        // forget on the one-shot control connection (the desktop confirms
        // delivery on its own screen).
        val manager = context.getSystemService(NotificationManager::class.java)
        manager.cancel(tag, id.hashCode())
        Thread {
            runCatching { RustCore.sendNotificationReply(address, port, id, text) }
        }.start()
    }
}
