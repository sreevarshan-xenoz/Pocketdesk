package dev.pocketdesk.android.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.platform.LocalContext
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.automation.Commands
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withContext

/** Commands the computer of this session sends the phone's automation apps (L6). */
@Composable
fun CommandsEffect(address: String) {
    val context = LocalContext.current
    DisposableEffect(address) {
        Commands.inSession = true
        onDispose { Commands.inSession = false }
    }
    LaunchedEffect(address) {
        while (isActive) {
            val own = HostStore.hosts(context).firstOrNull { it.address == address }
            withContext(Dispatchers.IO) { Commands.take(context, own) }
            delay(1_000)
        }
    }
}
