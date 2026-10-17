package dev.pocketdesk.android.ui

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import dev.pocketdesk.android.R
import dev.pocketdesk.android.service.SessionForegroundService
import dev.pocketdesk.android.stream.MicCapture
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * The phone's mic shared into the session (R4 E2): MediaCodec-encodes it to
 * Opus and streams it to the computer's mic relay. One for a screen session
 * and a sound session (N5). [on] while it runs; a failure is reported as a
 * string resource through the `onError` of [rememberMicShare].
 */
@Stable
class MicShare internal constructor(
    private val context: Context,
    private val scope: CoroutineScope,
    private val askPermission: () -> Unit,
    private val onError: (Int) -> Unit,
) {
    private var capture by mutableStateOf<MicCapture?>(null)

    val on: Boolean get() = capture != null

    fun start() {
        if (capture != null) return
        if (!MicCapture.isAvailable()) {
            onError(R.string.mic_unavailable)
            return
        }
        if (context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            askPermission()
            return
        }
        val starting = MicCapture()
        scope.launch(Dispatchers.IO) {
            // onDropped: the worker saw the session die; clear the switch.
            val error = starting.start { scope.launch(Dispatchers.Main) { capture = null } }
            withContext(Dispatchers.Main) {
                if (error != null) onError(error) else capture = starting
            }
        }
    }

    fun stop() {
        val running = capture ?: return
        capture = null
        scope.launch(Dispatchers.IO) { running.stop() }
    }

    internal fun release() {
        capture?.let { running -> Thread { running.stop() }.start() }
        capture = null
    }
}

/**
 * A [MicShare] for the session with `address`, asking for the mic when it
 * must; after a grant it starts the session's foreground service again, as
 * Android 14+ pins mic access to the service types declared at its start.
 */
@Composable
fun rememberMicShare(address: String, port: Int, controlPort: Int, onError: (Int) -> Unit): MicShare {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val reportError by rememberUpdatedState(onError)
    var reply by remember { mutableStateOf<Boolean?>(null) }
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { reply = it }
    val share =
        remember(address, port) {
            MicShare(context, scope, { launcher.launch(Manifest.permission.RECORD_AUDIO) }, { reportError(it) })
        }
    LaunchedEffect(reply) {
        val granted = reply ?: return@LaunchedEffect
        reply = null
        if (!granted) {
            reportError(R.string.mic_permission_denied)
            return@LaunchedEffect
        }
        context.startForegroundService(
            Intent(context, SessionForegroundService::class.java).apply {
                action = SessionForegroundService.ACTION_START
                putExtra(SessionForegroundService.EXTRA_ADDRESS, address)
                putExtra(SessionForegroundService.EXTRA_PORT, port)
                putExtra(SessionForegroundService.EXTRA_CONTROL_PORT, controlPort)
            },
        )
        share.start()
    }
    DisposableEffect(share) { onDispose { share.release() } }
    return share
}
