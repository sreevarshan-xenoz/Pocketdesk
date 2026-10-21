package dev.pocketdesk.android.clip

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.notify.PhoneLink
import dev.pocketdesk.android.ui.ClipHistory
import dev.pocketdesk.android.ui.ClipboardImages
import dev.pocketdesk.android.ui.ImageClipRule
import dev.pocketdesk.android.ui.writeLocalClipboard

/**
 * The text that last crossed between this phone and its computer, either
 * way (T10b). The session's clipboard sync, the background applier and
 * "Send clipboard" all note it, so none of them moves the same text back.
 */
object ClipboardShared {
    @Volatile
    var lastExchanged: String? = null

    /** Session clipboard syncs running: while one runs, it alone applies the computer's text. */
    val sessionSyncs = java.util.concurrent.atomic.AtomicInteger(0)

    /**
     * The id of the last image this phone put on the computer's clipboard
     * outside a session sync (a screenshot, T10h): a session sync never
     * fetches it back onto the phone's own clipboard.
     */
    @Volatile
    var sentImageId: String? = null
}

/** The choices of the background applier (T10b). */
object ClipboardPushRule {
    /** Put [incoming] on the phone's clipboard? Not blank, and not what just crossed. */
    fun shouldApply(incoming: String, lastExchanged: String?): Boolean =
        incoming.isNotBlank() && incoming != lastExchanged

    /** Text is taken only from the computer the phone sends to. */
    fun fromOwnComputer(host: String, own: String?): Boolean = own != null && host == own

    /**
     * Fetch the computer's image [id] (M3)? With the switch on, on Wi-Fi or
     * Ethernet, and not the image this phone just sent there.
     */
    fun mayFetchImage(on: Boolean, onWifi: Boolean, id: String, sentImageId: String?): Boolean =
        on && onWifi && id != sentImageId

    /**
     * Whether the network is Wi-Fi or Ethernet, never mobile data (M3 review:
     * an unmetered 5G plan is still mobile data). A VPN over Wi-Fi often
     * reads as metered, so one counts as unmetered; its own capabilities
     * carry the network under it.
     */
    fun onWifi(wifi: Boolean, ethernet: Boolean, notMetered: Boolean, vpn: Boolean): Boolean =
        (wifi || ethernet) && (notMetered || vpn)
}

/**
 * The computer's clipboard on the phone without a session (T10b): run on
 * the notification listener's 5 s tick, it keeps the control link to the
 * phone's computer up (over the internet when it must) and puts the text
 * the computer pushed on the phone's clipboard. Android lets a background
 * app write the clipboard, not read it. An image the computer tells of
 * (M3) is fetched on a thread of its own, and kept waiting until it is, so
 * a failure or a push on mobile data is tried again (M3 review).
 */
class BackgroundClipboard(private val ctx: Context) {
    private var lastConnectMs = 0L
    private val main = Handler(Looper.getMainLooper())

    /** The images that crossed here, and a failed fetch's wait (M3, as N4's session); guarded by itself. */
    private val images = ImageClipRule()

    /** The computer's image waiting to be fetched, until it is or another push replaces it. */
    private data class Pending(val host: String, val port: Int, val image: RustCore.PushedImage)

    @Volatile
    private var pending: Pending? = null

    /** One fetch at a time, off the listener's tick: a large image must not hold files and battery up. */
    private val fetcher = java.util.concurrent.Executors.newSingleThreadExecutor()
    private val fetching = java.util.concurrent.atomic.AtomicBoolean(false)

    fun tick() {
        if (!Prefs.clipboardInBackground(ctx) || ClipboardShared.sessionSyncs.get() > 0) {
            // Not this applier's to apply: dropped, so it is not applied
            // later, when it is old.
            RustCore.takeClipboardPush()
            pending = null
            return
        }
        val own = PhoneLink.target(ctx) ?: return
        val now = SystemClock.elapsedRealtime()
        if (RustCore.controlPeer() == null && now - lastConnectMs >= RECONNECT_MS) {
            lastConnectMs = now
            RustCore.connectToPeer(own.address, own.controlPort, HostStore.wanIdentity(ctx, own.address))
        }
        RustCore.takeClipboardPush()?.takeIf { ClipboardPushRule.fromOwnComputer(it.host, own.address) }?.let { pushed ->
            val image = pushed.image
            if (image != null) {
                pending = Pending(pushed.host, pushed.port, image)
            } else {
                pending = null
                pushed.content?.let { apply(it) }
            }
        }
        pending?.let { maybeFetch(it) }
    }

    /** Stop the fetcher with the listener. */
    fun close() {
        fetcher.shutdownNow()
    }

    private fun apply(content: String) {
        // Any text from the computer: its image is gone there, so the same
        // image copied again is fetched again (M3 review).
        synchronized(images) { images.textMoved() }
        if (!ClipboardPushRule.shouldApply(content, ClipboardShared.lastExchanged)) return
        ClipboardShared.lastExchanged = content
        main.post {
            writeLocalClipboard(ctx, content)
            ClipHistory.add(ctx, content)
        }
    }

    private fun wifi(): Boolean {
        val cm = ctx.getSystemService(ConnectivityManager::class.java) ?: return false
        val caps = cm.getNetworkCapabilities(cm.activeNetwork) ?: return false
        return ClipboardPushRule.onWifi(
            wifi = caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI),
            ethernet = caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET),
            notMetered = caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED),
            vpn = caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN),
        )
    }

    /** Fetch [wait]'s image now, if it may be and none is under way. */
    private fun maybeFetch(wait: Pending) {
        val on = Prefs.clipboardImagesInBackground(ctx)
        if (!on || wait.image.id == ClipboardShared.sentImageId) {
            pending = null
            return
        }
        if (!ClipboardPushRule.mayFetchImage(on, wifi(), wait.image.id, ClipboardShared.sentImageId)) return
        val now = SystemClock.elapsedRealtime()
        val (nowOk, ever) = synchronized(images) {
            images.shouldFetch(wait.image.id, wait.image.size, now) to
                images.shouldFetch(wait.image.id, wait.image.size, now + ImageClipRule.RETRY_MS)
        }
        if (!ever) {
            // Fetched already, or over the limit: nothing to wait for.
            pending = null
            return
        }
        if (!nowOk || !fetching.compareAndSet(false, true)) return
        runCatching {
            fetcher.execute {
                try {
                    fetch(wait)
                } finally {
                    fetching.set(false)
                }
            }
        }.onFailure { fetching.set(false) }
    }

    /** Fetch the computer's copied image onto the phone's clipboard (M3), as N4's session does. */
    private fun fetch(wait: Pending) {
        // A session started meanwhile fetches it itself.
        if (ClipboardShared.sessionSyncs.get() > 0) return
        val id = wait.image.id
        val now = SystemClock.elapsedRealtime()
        val part = ClipboardImages.partFile(ctx, id)
        RustCore.fetchClipboardImage(wait.host, wait.port, id, part.path)
            .mapCatching { ClipboardImages.keepReceived(ctx, part, id) }
            .onSuccess { file ->
                synchronized(images) { images.moved(id) }
                if (pending?.image?.id == id) pending = null
                ClipboardShared.lastExchanged = null
                main.post { runCatching { ClipboardImages.writeLocal(ctx, file) } }
            }
            .onFailure {
                part.delete()
                synchronized(images) { images.fetchFailed(id, now) }
            }
    }

    private companion object {
        const val RECONNECT_MS = 60_000L
    }
}
