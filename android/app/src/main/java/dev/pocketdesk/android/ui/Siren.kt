package dev.pocketdesk.android.ui

import android.content.Context
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.ring.PhoneRinger
import dev.pocketdesk.android.ring.RingOwner
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

private const val SIREN_POLL_MS = 2_000L

/**
 * Find-my-device siren, phone side (R3 Tier-2 #11): while a session or the
 * remote is on screen, take the bridge latch the control reader sets when
 * the computer pushes `kdeconnect.findmydevice` `{ring:true}`, and ring.
 *
 * N1: the ring is [PhoneRinger]'s, the same as in the background (its
 * notification with Stop, the alarm volume raised and put back after before
 * Android 17, left as the owner set it on 17 (N4)), and while a
 * watcher is on screen the latch is its own ([RingOwner]): the background
 * tick leaves it alone. C8's dialog still says what is happening, with a
 * Silence button.
 */
@Composable
fun SirenWatcher(context: Context) {
    var stopJob by remember { mutableStateOf<Job?>(null) }
    var ringing by remember { mutableStateOf(false) }

    fun silence() {
        stopJob?.cancel()
        stopJob = null
        PhoneRinger.stop(context)
        ringing = false
    }

    DisposableEffect(context) {
        RingOwner.watching()
        val scope = CoroutineScope(Dispatchers.Main)
        val job =
            scope.launch {
                while (isActive) {
                    if (RustCore.checkSiren() && !ringing) {
                        val computer = RustCore.controlPeer()?.let { HostStore.name(context, it.first) }
                        PhoneRinger.ring(context, computer)
                        ringing = true
                        stopJob =
                            scope.launch {
                                delay(PhoneRinger.RING_MS)
                                ringing = false
                            }
                    }
                    delay(SIREN_POLL_MS)
                }
            }
        onDispose {
            RingOwner.left()
            job.cancel()
            stopJob?.cancel()
        }
    }

    if (ringing) {
        AlertDialog(
            onDismissRequest = ::silence,
            icon = {
                LlIcon(
                    LlIcons.Notifications,
                    null,
                    tint = MaterialTheme.colorScheme.error,
                    size = 32.dp,
                )
            },
            title = { Text(stringResource(R.string.siren_ringing_title)) },
            text = { Text(stringResource(R.string.siren_ringing_text)) },
            confirmButton = {
                Button(onClick = ::silence) {
                    Text(stringResource(R.string.siren_dismiss))
                }
            },
            dismissButton = {
                TextButton(onClick = ::silence) {
                    Text(stringResource(R.string.cancel))
                }
            },
        )
    }
}
