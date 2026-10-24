package dev.pocketdesk.android.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
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
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.pocketdesk.android.R

/** Phone-side clipboard history, persisted across sessions (Tier 1 #4). */
object ClipHistory {
    private const val PREFS = "pocketdesk-clipboard"
    private const val KEY = "entries"
    private const val MAX = 20

    // Unit separator: a character no real clipboard text would contain.
    private const val SEP = "\u001F"

    fun all(ctx: Context): List<String> =
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .getString(KEY, null)
            ?.split(SEP)
            ?.filter { it.isNotBlank() }
            .orEmpty()

    fun add(ctx: Context, text: String) {
        val clean = text.trim()
        if (clean.isBlank() || clean.length > 8192) return
        val current = all(ctx).toMutableList()
        if (current.firstOrNull() == clean) return
        current.remove(clean)
        current.add(0, clean)
        while (current.size > MAX) current.removeAt(current.size - 1)
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .putString(KEY, current.joinToString(SEP))
            .apply()
    }

    fun clear(ctx: Context) {
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().remove(KEY).apply()
    }
}

fun localClipboardText(ctx: Context): String? =
    ctx.getSystemService(ClipboardManager::class.java)
        ?.primaryClip
        // An image clip is not text: coerced, it would be its URI (N4).
        ?.takeIf { it.itemCount > 0 && !it.description.hasMimeType("image/*") }
        ?.getItemAt(0)
        ?.coerceToText(ctx)
        ?.toString()
        ?.takeIf { it.isNotBlank() }

fun writeLocalClipboard(ctx: Context, text: String) {
    val cm = ctx.getSystemService(ClipboardManager::class.java) ?: return
    if (localClipboardText(ctx) == text) return
    cm.setPrimaryClip(ClipData.newPlainText("Pocketdesk", text))
    // AA1: how long, never the text, as the received image's line says it
    // (a copy from the computer, or one picked from the history).
    android.util.Log.i("PocketdeskClip", "text put on the clipboard (${text.length} chars)")
}

/**
 * Bottom sheet over [ClipHistory]: tap an entry to put it on the phone
 * clipboard and push it to the remote; entries also land here from sync.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ClipboardHistorySheet(
    onDismiss: () -> Unit,
    onPick: (String) -> Unit,
    // From Home (D3): the computer a tap sends to; in a session, null (a tap
    // copies, and sends with the session's sync).
    sendTo: String? = null,
) {
    val context = LocalContext.current
    val entries = remember { mutableStateOf(ClipHistory.all(context)) }
    // C8: clearing 20 captured clips is destructive enough to confirm.
    var confirmClear by remember { mutableStateOf(false) }

    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(modifier = Modifier.fillMaxWidth().padding(bottom = 16.dp)) {
            Row(
                modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp),
            ) {
                Column(modifier = Modifier.weight(1f)) {
                    Text(
                        stringResource(R.string.clipboard_history),
                        style = MaterialTheme.typography.titleMedium,
                    )
                    // What a tap does, from Home (E6).
                    if (sendTo != null) {
                        Text(
                            stringResource(R.string.clipboard_tap_sends, sendTo),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
                // Only with something to clear (E6).
                if (entries.value.isNotEmpty()) {
                    TextButton(
                        onClick = { confirmClear = true },
                    ) {
                        Text(stringResource(R.string.clear), color = MaterialTheme.colorScheme.error)
                    }
                }
            }
            HorizontalDivider()
            LazyColumn(modifier = Modifier.fillMaxWidth()) {
                if (entries.value.isEmpty()) {
                    item {
                        Column(
                            modifier = Modifier.fillMaxWidth().padding(vertical = 24.dp),
                            horizontalAlignment = Alignment.CenterHorizontally,
                            verticalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            LlIcon(
                                LlIcons.Clipboard,
                                null,
                                size = 40.dp,
                                tint = MaterialTheme.colorScheme.outline,
                            )
                            Text(
                                stringResource(
                                    if (sendTo != null) R.string.clipboard_empty_home else R.string.clipboard_empty,
                                ),
                                style = MaterialTheme.typography.bodyMedium,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                }
                items(entries.value) { entry ->
                    val pickLabel = stringResource(if (sendTo != null) R.string.clipboard_send else R.string.clipboard_copy)
                    Text(
                        entry,
                        style = MaterialTheme.typography.bodyMedium,
                        maxLines = 3,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier
                            .fillMaxWidth()
                            // 48 dp to the touch, with what a tap does (E6).
                            .heightIn(min = 48.dp)
                            .clickable(onClickLabel = pickLabel) {
                                ClipHistory.add(context, entry)
                                entries.value = ClipHistory.all(context)
                                onPick(entry)
                            }
                            .padding(horizontal = 16.dp, vertical = 12.dp),
                    )
                }
            }
        }
    }

    if (confirmClear) {
        AlertDialog(
            onDismissRequest = { confirmClear = false },
            title = { Text(stringResource(R.string.clear)) },
            text = { Text(stringResource(R.string.clipboard_clear_message)) },
            confirmButton = {
                Button(
                    onClick = {
                        ClipHistory.clear(context)
                        entries.value = emptyList()
                        confirmClear = false
                    },
                    colors = ButtonDefaults.buttonColors(
                        containerColor = MaterialTheme.colorScheme.error,
                        contentColor = MaterialTheme.colorScheme.onError,
                    ),
                ) {
                    Text(stringResource(R.string.clear))
                }
            },
            dismissButton = {
                TextButton(onClick = { confirmClear = false }) {
                    Text(stringResource(R.string.cancel))
                }
            },
        )
    }
}