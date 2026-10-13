package dev.pocketdesk.android.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.stream.DesktopMapping
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

/** One Hyprland window as reported by the server's windows plugin. */
data class DesktopWindow(
    val address: String,
    val title: String,
    val app: String,
    /** Global desktop position — where the cropped video sits. */
    val at: IntArray,
    /** Monitor-local position — the space WindowCrop rects use. */
    val localAt: IntArray,
    val size: IntArray,
    val monitorSize: IntArray,
    val workspaceName: String,
    val active: Boolean,
) {
    override fun equals(other: Any?): Boolean = other is DesktopWindow && other.address == address
    override fun hashCode(): Int = address.hashCode()
}

/** Parsed `getWindows` payload: windows + the monitor layout box. */
data class WindowsListing(
    val windows: List<DesktopWindow>,
    val activeAddress: String,
    /** `[x, y, w, h]` of the whole monitor layout, or null (old server). */
    val screen: IntArray?,
)

private fun jsonIntArray(arr: JSONArray?): IntArray =
    if (arr == null || arr.length() < 2) IntArray(2) else intArrayOf(arr.getInt(0), arr.getInt(1))

/** Parse one server window object (getWindows payload or hyprland.state snapshot). */
fun desktopWindowFromJson(w: JSONObject): DesktopWindow =
    DesktopWindow(
        address = w.optString("address"),
        title = w.optString("title"),
        app = w.optString("class"),
        at = jsonIntArray(w.optJSONArray("at")),
        localAt = jsonIntArray(w.optJSONArray("local_at")),
        size = jsonIntArray(w.optJSONArray("size")),
        monitorSize = jsonIntArray(w.optJSONArray("monitor_size")),
        workspaceName = w.optJSONObject("workspace")?.optString("name") ?: "",
        active = w.optBoolean("active"),
    )

fun parseWindowsPayload(json: String): WindowsListing {
    val root = JSONArray(json)
    val windowsArr = root.optJSONArray(0) ?: JSONArray()
    val active = root.optString(1, "")
    val screen = root.optJSONArray(2)?.let {
        intArrayOf(it.getInt(0), it.getInt(1), it.getInt(2), it.getInt(3))
    }
    val windows =
        (0 until windowsArr.length()).map { i ->
            desktopWindowFromJson(windowsArr.getJSONObject(i))
        }
    return WindowsListing(windows, active, screen)
}

/**
 * Desktop coordinate for normalized absolute input: the video shows the
 * window's global rect inside the `screen` layout box. Null when the server
 * gave no screen box (client falls back to video-space mapping).
 */
fun DesktopWindow.desktopMapping(screen: IntArray?): DesktopMapping? {
    val s = screen ?: return null
    if (s[2] <= 0 || s[3] <= 0 || size[0] <= 0 || size[1] <= 0) return null
    return DesktopMapping(
        desktopX = at[0],
        desktopY = at[1],
        desktopW = size[0],
        desktopH = size[1],
        screenX = s[0],
        screenY = s[1],
        screenW = s[2],
        screenH = s[3],
    )
}

/**
 * R3#7 window picker: lists the Hyprland session's visible windows; picking
 * one crops the server's capture to its geometry (single-window streaming).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun WindowPickerSheet(
    address: String,
    controlPort: Int,
    selected: DesktopWindow?,
    onDismiss: () -> Unit,
    onPick: (DesktopWindow, IntArray?) -> Unit,
    onFullDesktop: () -> Unit,
) {
    var listing by remember { mutableStateOf<WindowsListing?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    val malformedMsg = stringResource(R.string.malformed_windows)
    val queryFailMsg = stringResource(R.string.window_query_failed)

    LaunchedEffect(address, controlPort) {
        val result =
            withContext(Dispatchers.IO) {
                RustCore.getWindows(address, controlPort)
            }
        result.fold(
            onSuccess = { json ->
                runCatching { parseWindowsPayload(json) }
                    .onSuccess { listing = it }
                    .onFailure { error = malformedMsg }
            },
            onFailure = { error = it.message ?: queryFailMsg },
        )
    }

    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(modifier = Modifier.padding(horizontal = 8.dp)) {
            Text(
                stringResource(R.string.stream_window),
                style = MaterialTheme.typography.titleMedium,
                modifier = Modifier.padding(start = 8.dp, bottom = 4.dp),
            )
            ListItem(
                headlineContent = {
                    Text(stringResource(R.string.whole_desktop), style = MaterialTheme.typography.bodyLarge)
                },
                leadingContent = { LlIcon(LlIcons.Desktop, null) },
                trailingContent = { RadioButton(selected = selected == null, onClick = onFullDesktop) },
                modifier = Modifier.clickable(onClick = onFullDesktop),
            )
            HorizontalDivider()
            when {
                error != null ->
                    Text(
                        error!!,
                        color = MaterialTheme.colorScheme.error,
                        style = MaterialTheme.typography.bodyMedium,
                        modifier = Modifier.padding(vertical = 12.dp),
                    )

                listing == null ->
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(vertical = 24.dp),
                        horizontalArrangement = androidx.compose.foundation.layout.Arrangement.Center,
                    ) {
                        CircularProgressIndicator()
                    }

                else -> LazyColumn(modifier = Modifier.padding(bottom = 24.dp)) {
                    items(listing!!.windows, key = { it.address }) { w ->
                        val label = w.title.ifBlank { w.app.ifBlank { w.address } }
                        val picked = w == selected
                        ListItem(
                            headlineContent = {
                                Text(
                                    label,
                                    style = MaterialTheme.typography.bodyLarge,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                    color = if (picked) {
                                        MaterialTheme.colorScheme.primary
                                    } else {
                                        MaterialTheme.colorScheme.onSurface
                                    },
                                )
                            },
                            supportingContent = {
                                Text(
                                    buildString {
                                        val focusedTag = stringResource(R.string.window_focused)
                                        append(w.app.ifBlank { "?" })
                                        if (w.workspaceName.isNotBlank()) {
                                            append("  ·  ws ")
                                            append(w.workspaceName)
                                        }
                                        append("  ·  ${w.size[0]}x${w.size[1]}")
                                        if (w.active) append("  ·  $focusedTag")
                                    },
                                    style = MaterialTheme.typography.bodySmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                            },
                            leadingContent = { LlIcon(LlIcons.Monitor, null) },
                            trailingContent = {
                                RadioButton(
                                    selected = picked,
                                    onClick = { onPick(w, listing!!.screen) },
                                )
                            },
                            modifier = Modifier.clickable { onPick(w, listing!!.screen) },
                        )
                        HorizontalDivider()
                    }
                }
            }
        }
    }
}
