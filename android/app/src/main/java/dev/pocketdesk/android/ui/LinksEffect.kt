package dev.pocketdesk.android.ui

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.platform.LocalContext
import androidx.core.net.toUri
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.links.LinkRule
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withContext
import java.util.concurrent.atomic.AtomicInteger

/**
 * Links the computer hands over (T8a): each becomes a notification that
 * opens it, never a page opened by itself. With "Links from the computer"
 * off they are dropped.
 */

const val CHANNEL_LINKS = "pocketdesk-links"

private val linkNotificationIds = AtomicInteger(0)

@Composable
fun LinksEffect(address: String) {
    val context = LocalContext.current
    LaunchedEffect(address) {
        while (isActive) {
            val links = withContext(Dispatchers.IO) { RustCore.takeLinks() }
            if (Prefs.linksFromComputer(context)) {
                // The bridge checked them; checked again where they open.
                links.filter { LinkRule.openable(it) }.forEach { postLink(context, address, it) }
            }
            delay(1_000)
        }
    }
}

private fun postLink(context: Context, address: String, url: String) {
    val manager = context.getSystemService(NotificationManager::class.java)
    if (manager.getNotificationChannel(CHANNEL_LINKS) == null) {
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL_LINKS,
                context.getString(R.string.channel_links),
                NotificationManager.IMPORTANCE_DEFAULT,
            ),
        )
    }
    val id = linkNotificationIds.incrementAndGet()
    val open = Intent(Intent.ACTION_VIEW, url.toUri())
        .addCategory(Intent.CATEGORY_BROWSABLE)
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    val pending = PendingIntent.getActivity(
        context,
        id,
        open,
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )
    val notification = Notification.Builder(context, CHANNEL_LINKS)
        .setSmallIcon(R.drawable.ic_stat_pocketdesk)
        .setContentTitle(context.getString(R.string.link_from_computer, address))
        .setContentText(url)
        .setContentIntent(pending)
        .setAutoCancel(true)
        .build()
    manager.notify(TAG_LINK, id, notification)
}

private const val TAG_LINK = "link"
