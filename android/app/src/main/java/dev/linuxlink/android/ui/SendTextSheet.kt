package dev.linuxlink.android.ui

import android.content.ClipboardManager
import android.content.Context
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SuggestionChip
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.linuxlink.android.R
import dev.linuxlink.android.stream.keyboard.TextSend

/** The phone clipboard's text, or null when it holds none. */
fun phoneClipboardText(context: Context): String? {
    val clip = context.getSystemService(ClipboardManager::class.java)?.primaryClip ?: return null
    if (clip.itemCount == 0) return null
    return clip.getItemAt(0).coerceToText(context)?.toString()?.takeIf { it.isNotEmpty() }
}

/**
 * Roadmap F2: text sent as a whole: dictated or long text reviewed before it
 * lands, the phone clipboard typed for fields that block paste, and the
 * session's last few texts to resend (spec S1, S4). [onSend] types the text
 * (and Enter if asked) and returns the error to show, or null once sent.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SendTextSheet(
    history: List<String>,
    onSend: (text: String, pressEnter: Boolean) -> String?,
    onDismiss: () -> Unit,
) {
    val context = LocalContext.current
    var text by remember { mutableStateOf("") }
    var pressEnter by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val noClipboardText = stringResource(R.string.phone_clipboard_no_text)

    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp).padding(bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(stringResource(R.string.send_text_title), style = MaterialTheme.typography.titleMedium)
            OutlinedTextField(
                value = text,
                onValueChange = {
                    text = it
                    error = null
                },
                placeholder = { Text(stringResource(R.string.send_text_hint)) },
                isError = error != null,
                supportingText = error?.let { message -> { Text(message) } },
                modifier = Modifier.fillMaxWidth().heightIn(min = 96.dp),
            )
            if (history.isNotEmpty()) {
                Row(
                    modifier = Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    for (past in history) {
                        SuggestionChip(
                            onClick = { text = past },
                            label = {
                                Text(
                                    past,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                    modifier = Modifier.widthIn(max = 160.dp),
                                )
                            },
                        )
                    }
                }
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    stringResource(R.string.send_text_enter),
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.weight(1f),
                )
                Switch(checked = pressEnter, onCheckedChange = { pressEnter = it })
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                TextButton(onClick = {
                    val clip = phoneClipboardText(context)
                    if (clip == null) error = noClipboardText else text = clip
                }) {
                    Text(stringResource(R.string.send_text_paste))
                }
                Spacer(Modifier.weight(1f))
                Button(
                    enabled = text.isNotEmpty(),
                    onClick = {
                        error = onSend(text, pressEnter)
                        if (error == null) onDismiss()
                    },
                ) {
                    Text(stringResource(R.string.send_text_send))
                }
            }
            Text(
                pluralStringResource(R.plurals.send_text_limit, TextSend.MAX_CHARS, TextSend.MAX_CHARS),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}
