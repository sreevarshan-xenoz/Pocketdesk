package dev.pocketdesk.android.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.calls.Dials
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withContext

/**
 * Numbers the computer of this session asks to dial (L4): the dialer opens
 * at once while the app is in front, and is offered in a notification while
 * it is not (Android blocks a start from behind; L4 review).
 */
@Composable
fun DialsEffect(address: String) {
    val context = LocalContext.current
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(address) {
        Dials.inSession = true
        onDispose { Dials.inSession = false }
    }
    LaunchedEffect(address) {
        while (isActive) {
            val own = HostStore.hosts(context).firstOrNull { it.address == address }
            val inFront = lifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED)
            withContext(Dispatchers.IO) {
                Dials.take(context, own, inFront)
                // BF3: questions, judged as outside a session against the
                // phone's own computer, not this session's (BF3 review).
                dev.pocketdesk.android.ask.Asks.take(context, dev.pocketdesk.android.notify.PhoneLink.target(context))
            }
            delay(1_000)
        }
    }
}
