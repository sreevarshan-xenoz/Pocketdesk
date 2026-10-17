package dev.pocketdesk.android.ui

import android.content.Context
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.res.stringResource
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore

/**
 * "Lock the computer?" (U2): Lock locked at one tap, from the session's
 * sheet and from the session notification, and said nothing after. Asked
 * first now, from both; [onLock] runs [lockComputer] in the caller's scope
 * (this dialog's own ends as it closes) and says what happened.
 */
@Composable
fun LockQuestion(onDismiss: () -> Unit, onLock: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.lock_confirm_title)) },
        text = { Text(stringResource(R.string.lock_confirm_message)) },
        confirmButton = {
            TextButton(onClick = {
                onDismiss()
                onLock()
            }) { Text(stringResource(R.string.lock_confirm)) }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) }
        },
    )
}

/** Lock the computer; what to say about it. Blocks: call off the main thread. */
fun lockComputer(context: Context, address: String, controlPort: Int): String {
    val outcome = RustCore.desktopPrivacy(address, controlPort, "status", lock = true).fold(
        { PrivacyReply.read(it, askedLock = true) },
        { PrivacyOutcome.Refused(it.message) },
    )
    return when (outcome) {
        PrivacyOutcome.Done -> context.getString(R.string.lock_done)
        is PrivacyOutcome.Refused ->
            context.getString(R.string.lock_failed, context.plainErrorText(outcome.why))
    }
}
