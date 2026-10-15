package dev.pocketdesk.android.ui

import android.content.Context
import android.net.Uri
import android.os.SystemClock
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.platform.LocalContext
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.clip.ClipboardShared
import dev.pocketdesk.android.bridge.RemoteClip
import dev.pocketdesk.android.bridge.RustCore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.isActive
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext

/**
 * Two-way clipboard sync over the KDE Connect control channel (Tier 1 #4),
 * text and images (N4). An image's bytes go through the bridge's transfer
 * path, not the control channel; [ImageClipRule] keeps one from bouncing back.
 *
 * Push: local clipboard changes go out via sendClipboard. Pull: the server
 * never broadcasts clipboard changes on its own, so the remote value is
 * polled with getClipboard. lastLocal/lastRemote tracking prevents the
 * classic echo loop (our push arriving as a "remote change" and bouncing
 * back). Runs only while [enabled]; the control connection is (re)established
 * whenever a send fails.
 *
 * An image already on the phone's clipboard when sync starts is not sent: it
 * may be days old, and would replace whatever the desktop holds.
 */
@Composable
fun ClipboardSyncEffect(
    address: String,
    controlPort: Int,
    enabled: Boolean,
) {
    val context = LocalContext.current
    LaunchedEffect(enabled, address, controlPort) {
        if (!enabled) return@LaunchedEffect
        ClipboardShared.sessionSyncs.incrementAndGet()
        try {
            withContext(Dispatchers.IO) {
                RustCore.connectToPeer(address, controlPort, HostStore.wanIdentity(context, address))
                var lastLocal = localClipboardText(context)
                var lastRemote: String? = null
                lastLocal?.let {
                    if (RustCore.sendClipboard(address, controlPort, it).isSuccess) {
                        lastRemote = it
                        ClipboardShared.lastExchanged = it
                    }
                }
                val images = ImageClipRule()
                // The image handled last (sent, skipped, or there at the start).
                var lastLocalImage: Uri? = ClipboardImages.localImageUri(context)
                // One that failed to send, and when, so it is retried after a while.
                var failedLocalImage: Uri? = null
                var failedAtMs = 0L
                while (isActive) {
                    delay(4000)
                    val now = SystemClock.elapsedRealtime()

                    val localImage = ClipboardImages.localImageUri(context)
                    if (localImage != null) {
                        val retryDue = localImage != failedLocalImage || now - failedAtMs >= ImageClipRule.RETRY_MS
                        if (localImage != lastLocalImage && retryDue) {
                            when (sendLocalImage(context, address, controlPort, localImage, images)) {
                                Outcome.Sent -> {
                                    lastLocalImage = localImage
                                    // The texts are no longer on either clipboard.
                                    lastLocal = null
                                    lastRemote = null
                                }
                                Outcome.Skipped -> lastLocalImage = localImage
                                Outcome.Failed -> {
                                    failedLocalImage = localImage
                                    failedAtMs = now
                                }
                            }
                        }
                    } else {
                        lastLocalImage = null
                        val local = localClipboardText(context)
                        if (local != null && local != lastLocal) {
                            lastLocal = local
                            if (RustCore.sendClipboard(address, controlPort, local).isSuccess) {
                                ClipboardShared.lastExchanged = local
                                ClipHistory.add(context, local)
                                lastRemote = local
                                images.textMoved()
                            } else {
                                RustCore.connectToPeer(address, controlPort, HostStore.wanIdentity(context, address))
                            }
                        }
                    }

                    when (val remote = RustCore.getClipboardState(address, controlPort).getOrNull()) {
                        // The phone's own screenshot (T10h) stays off the phone's clipboard.
                        is RemoteClip.Image -> if (remote.id == ClipboardShared.sentImageId) {
                            images.moved(remote.id)
                        } else if (images.shouldFetch(remote.id, remote.size, now)) {
                            val part = ClipboardImages.partFile(context, remote.id)
                            val fetched = RustCore.fetchClipboardImage(address, controlPort, remote.id, part.path)
                            val kept = fetched.mapCatching { ClipboardImages.keepReceived(context, part, remote.id) }
                            kept.onSuccess { file ->
                                images.moved(remote.id)
                                ClipboardImages.writeLocal(context, file)
                                lastLocalImage = ClipboardImages.localImageUri(context)
                                lastLocal = null
                                lastRemote = null
                            }.onFailure {
                                part.delete()
                                images.fetchFailed(remote.id, now)
                            }
                        }
                        is RemoteClip.Text -> {
                            val text = remote.content
                            if (text != lastRemote) {
                                lastRemote = text
                                ClipboardShared.lastExchanged = text
                                ClipHistory.add(context, text)
                                if (text.isNotBlank() && text != lastLocal) {
                                    writeLocalClipboard(context, text)
                                    lastLocal = text
                                    images.textMoved()
                                }
                            }
                        }
                        null -> {}
                    }
                }
            }
        } finally {
            ClipboardShared.sessionSyncs.decrementAndGet()
        }
    }
}

private enum class Outcome { Sent, Skipped, Failed }

/**
 * Send the image at [uri] to the desktop's clipboard, unless it is this
 * app's own, cannot be read, or is the one that just moved.
 */
private fun sendLocalImage(
    context: Context,
    address: String,
    controlPort: Int,
    uri: Uri,
    images: ImageClipRule,
): Outcome {
    val ours = ClipboardImages.isOurs(context, uri)
    if (ours) return Outcome.Skipped
    val png = ClipboardImages.pngOf(context, uri) ?: return Outcome.Skipped
    val id = ImageClipRule.idOf(png)
    if (!images.shouldSend(id, ours)) return Outcome.Skipped
    val sent = ClipboardImages.withStaged(context, png) { file ->
        RustCore.sendClipboardImage(address, controlPort, file.path)
    }
    return if (sent.isSuccess) {
        images.moved(id)
        Outcome.Sent
    } else {
        RustCore.connectToPeer(address, controlPort, HostStore.wanIdentity(context, address))
        Outcome.Failed
    }
}
