package dev.pocketdesk.android.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.platform.LocalContext
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.calls.Dials
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withContext

/** Numbers the computer of this session asks to dial (L4): the dialer opens at once. */
@Composable
fun DialsEffect(address: String) {
    val context = LocalContext.current
    LaunchedEffect(address) {
        while (isActive) {
            val own = HostStore.hosts(context).firstOrNull { it.address == address }
            withContext(Dispatchers.IO) { Dials.take(context, own, inFront = true) }
            delay(1_000)
        }
    }
}
