package dev.pocketdesk.android.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.os.Build
import android.os.SystemClock
import android.util.Log
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.notify.PhoneLink
import dev.pocketdesk.android.ui.folderPartOf
import org.json.JSONArray

/**
 * Files the computer sends with no session open (T10d). Run on the
 * notification listener's tick while Settings "Files from the computer in
 * the background" is on: it keeps the control link to the phone's computer
 * up and downloads each offer. Where Android lets a background app start
 * the download (Android 11 and older, or an exemption), it starts at once;
 * otherwise a notification offers it, and its Save starts it.
 */
class BackgroundFiles(private val ctx: Context) {
    private var lastConnectMs = 0L
    private var nextCode = 0

    fun tick() {
        if (!Prefs.filesInBackground(ctx)) return
        val own = PhoneLink.target(ctx) ?: return
        val now = SystemClock.elapsedRealtime()
        if (RustCore.controlPeer() == null && now - lastConnectMs >= RECONNECT_MS) {
            lastConnectMs = now
            RustCore.connectToPeer(own.address, own.controlPort, HostStore.wanIdentity(ctx, own.address))
        }
        // A session takes its own offers; and an offer is fetched from the
        // computer that made it, which must be this phone's own.
        if (RustCore.isStreamingActive) return
        if (RustCore.controlPeer() != (own.address to own.controlPort)) return
        val offers = runCatching { JSONArray(RustCore.takeFileOffers()) }.getOrNull() ?: return
        for (i in 0 until offers.length()) {
            val offer = offers.optJSONObject(i) ?: continue
            val id = offer.optString("id").ifEmpty { continue }
            // Offered again while in hand, or waiting for a tap (S2 review):
            // not started or offered for a tap twice.
            if (!OfferHand.shared.claim(id, now)) continue
            val refused = offer.optString("folderRefused")
            if (refused.isNotEmpty()) {
                RustCore.declineOffer(own.address, own.controlPort, id, refused)
                continue
            }
            val name = offer.optString("name").ifEmpty { "download" }
            val intent =
                TransferService.receiveIntent(
                    ctx,
                    own.address,
                    own.controlPort,
                    true,
                    id,
                    name,
                    offer.optLong("size"),
                    offer.optString("source"),
                    folderPartOf(offer),
                    offer.optLong("lastModified"),
                )
            val started = runCatching { ctx.startForegroundService(intent) }
            if (started.isSuccess) continue
            // Not allowed from the background: offered for a tap instead.
            Log.i(TAG, "a file from the computer waits for a tap: ${started.exceptionOrNull()?.javaClass?.simpleName}")
            // Each its own request code and notification: two offers whose
            // ids hash alike must not share one intent.
            val code = nextCode++
            val save =
                PendingIntent.getForegroundService(
                    ctx,
                    code,
                    intent,
                    PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                )
            notify(code, name, save)
        }
    }

    private fun notify(code: Int, name: String, save: PendingIntent) {
        val manager = ctx.getSystemService(NotificationManager::class.java) ?: return
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, ctx.getString(R.string.channel_offers), NotificationManager.IMPORTANCE_DEFAULT),
        )
        val notification =
            Notification.Builder(ctx, CHANNEL)
                .setSmallIcon(R.drawable.ic_stat_pocketdesk)
                .setContentTitle(ctx.getString(R.string.offer_waiting_title))
                .setContentText(name)
                .setContentIntent(save)
                .setAutoCancel(true)
                .setGroup(CHANNEL)
                // The offer lasts 10 minutes on the computer.
                .setTimeoutAfter(OFFER_LIFETIME_MS)
                .addAction(Notification.Action.Builder(null, ctx.getString(R.string.offer_save), save).build())
                .build()
        runCatching { manager.notify(NOTIFICATION_BASE + code, notification) }
    }

    private companion object {
        const val TAG = "BackgroundFiles"
        const val CHANNEL = "offers"
        const val RECONNECT_MS = 60_000L
        const val OFFER_LIFETIME_MS = 10 * 60_000L
        const val NOTIFICATION_BASE = 40_000
    }
}
