package dev.pocketdesk.android.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.platform.LocalContext
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
        withContext(Dispatchers.IO) {
            RustCore.connectToPeer(address, controlPort)
            var lastLocal = localClipboardText(context)
            var lastRemote: String? = null
            lastLocal?.let {
                if (RustCore.sendClipboard(address, controlPort, it).isSuccess) lastRemote = it
            }
            val images = ImageClipRule()
            var lastLocalImage: android.net.Uri? = null
            while (isActive) {
                delay(4000)

                val localImage = ClipboardImages.localImageUri(context)
                if (localImage != null) {
                    if (localImage != lastLocalImage) {
                        lastLocalImage = localImage
                        sendLocalImage(context, address, controlPort, localImage, images)
                    }
                } else {
                    lastLocalImage = null
                    val local = localClipboardText(context)
                    if (local != null && local != lastLocal) {
                        lastLocal = local
                        if (RustCore.sendClipboard(address, controlPort, local).isSuccess) {
                            ClipHistory.add(context, local)
                            lastRemote = local
                        } else {
                            RustCore.connectToPeer(address, controlPort)
                        }
                    }
                }

                when (val remote = RustCore.getClipboardState(address, controlPort).getOrNull()) {
                    is RemoteClip.Image -> if (images.shouldFetch(remote.id, remote.size)) {
                        val file = ClipboardImages.receivedFile(context, remote.id)
                        if (RustCore.fetchClipboardImage(address, controlPort, remote.id, file.path).isSuccess) {
                            images.moved(remote.id)
                            ClipboardImages.writeLocal(context, file)
                        }
                    }
                    is RemoteClip.Text -> {
                        val text = remote.content
                        if (text != lastRemote) {
                            lastRemote = text
                            ClipHistory.add(context, text)
                            if (text.isNotBlank() && text != lastLocal) {
                                writeLocalClipboard(context, text)
                                lastLocal = text
                            }
                        }
                    }
                    null -> {}
                }
            }
        }
    }
}

/** Send the image at [uri] to the desktop's clipboard, unless it just came from there. */
private fun sendLocalImage(
    context: android.content.Context,
    address: String,
    controlPort: Int,
    uri: android.net.Uri,
    images: ImageClipRule,
) {
    val ours = ClipboardImages.isOurs(context, uri)
    if (ours) return
    val png = ClipboardImages.pngOf(context, uri) ?: return
    val id = ImageClipRule.idOf(png)
    if (!images.shouldSend(id, ours)) return
    val file = ClipboardImages.stage(context, png)
    if (RustCore.sendClipboardImage(address, controlPort, file.path).isSuccess) {
        images.moved(id)
    } else {
        RustCore.connectToPeer(address, controlPort)
    }
}
