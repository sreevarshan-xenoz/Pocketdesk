package dev.pocketdesk.android.media

import android.util.Log
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.repeatOnLifecycle
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.notify.PhoneNotificationPrefs
import dev.pocketdesk.android.ui.ControlLink
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withContext

private const val CHECK_MS = 2_000L
private const val TAG = "PhoneMedia"

/**
 * Tells the computer of this session what the phone plays (T8d) and acts on
 * its presses, while "My media on the computer" is on and Android lets this
 * app read media sessions (notification access). It checks every 2 s while
 * the app is started, and sends only on the session's own control link.
 * While the listener's background reporter runs ([BackgroundMedia.active],
 * N2), it steps aside: one reporter, one taker of presses.
 */
@Composable
fun PhoneMediaEffect(address: String, controlPort: Int) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    LaunchedEffect(address, controlPort, lifecycleOwner) {
        var sent = MediaRule.Sent()
        val covers = MediaArt.Cache()
        // The cover the computer holds: the last one sent (N2 review).
        var heldArt: String? = null
        lifecycleOwner.repeatOnLifecycle(Lifecycle.State.STARTED) {
            while (isActive) {
                withContext(Dispatchers.IO) {
                    if (BackgroundMedia.active) {
                        // Heard again in full when it takes over once more.
                        sent = MediaRule.Sent()
                        heldArt = null
                        return@withContext
                    }
                    val on = Prefs.phoneMediaOnComputer(context) &&
                        PhoneNotificationPrefs.accessGranted(context)
                    val linked = !ControlLink.needsConnect(RustCore.controlPeer(), address, controlPort)
                    // A computer reached again (a restart, a dropped link)
                    // knows nothing of what was sent: it hears it again.
                    sent = MediaRule.forLink(sent, linked)
                    if (!linked) heldArt = null
                    val presses = RustCore.takeMediaPresses()
                    var controllers = if (on) MediaSessions.controllers(context) else emptyList()
                    // Presses act on the session reported, the one the computer shows.
                    for (command in presses) {
                        if (!MediaRule.accepts(command, on)) continue
                        val target = sent.last?.packageName ?: continue
                        controllers.firstOrNull { it.packageName == target }?.let { MediaSessions.press(it, command) }
                    }
                    if (linked) {
                        if (on && presses.isNotEmpty()) {
                            delay(300) // the player's new state
                            controllers = MediaSessions.controllers(context)
                        }
                        // With the setting off, `{}` once, so the computer
                        // forgets the song it shows.
                        val sessions = if (on) {
                            controllers.mapNotNull { c -> c.metadata.let { m -> MediaSessions.describe(context, c, m)?.let { Triple(c, m, it) } } }
                        } else {
                            emptyList()
                        }
                        val picked = MediaRule.pick(sessions.map { it.third })
                        // N2: its cover, of the session picked, encoded once a track.
                        val cover = sessions.firstOrNull { it.third === picked }?.let { (c, m, _) -> covers.of(c.packageName, m) }
                        val now = picked?.copy(artId = cover?.id)
                        val nowMs = android.os.SystemClock.elapsedRealtime()
                        if (MediaRule.shouldSend(sent, now, nowMs)) {
                            val withArt = cover?.takeIf { now?.artId != null && MediaRule.sendArt(it.id, heldArt) }
                            val body = MediaSessions.body(now, background = false, cover = withArt)
                            val result = RustCore.sendPhonePacket(address, controlPort, PHONE_MEDIA, body.toString())
                            if (result.isSuccess) {
                                sent = MediaRule.Sent(any = true, last = now, atMs = nowMs)
                                heldArt = now?.artId
                            } else {
                                Log.d(TAG, "Not sent: ${result.exceptionOrNull()?.message}")
                            }
                        }
                    }
                }
                delay(CHECK_MS)
            }
        }
    }
}
