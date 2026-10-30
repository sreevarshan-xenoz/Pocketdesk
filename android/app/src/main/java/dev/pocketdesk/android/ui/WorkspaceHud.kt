package dev.pocketdesk.android.ui

import android.view.KeyEvent
import android.widget.Toast
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject

/**
 * Hyprland workspace HUD (R3#8): live workspace chips + focused-window
 * title, fed by the server's `hyprland.state` snapshots and `hyprland.event`
 * pushes on the control channel. Numeric workspaces up to 9 switch on tap
 * through the same Super+N keybind injection the shortcut bar uses
 * (Hyprland's IPC dispatch write path is broken upstream).
 *
 * E1 adds a window-chip row: tapping a visible window pulls it into the
 * stream through `onPullWindow`, which drives the same window-crop path the
 * picker sheet uses — one gesture, no sheet.
 */
private const val STATE_TYPE = "kdeconnect.pocketdesk.hyprland.state"
private const val EVENT_TYPE = "kdeconnect.pocketdesk.hyprland.event"

private data class WorkspaceHudState(
    val workspaces: List<WsEntry>,
    val activeId: Int,
    val activeTitle: String?,
    val windows: List<DesktopWindow> = emptyList(),
    val screen: IntArray? = null,
)

private data class WsEntry(val id: Int, val name: String)

/** `"1:2"` / `"2:web"` → id 1 / 2; null when unparseable. */
private fun parseWorkspaceRef(token: String): Int? =
    token.trim().substringBefore(':').toIntOrNull()

private fun parseWindowArray(body: JSONObject): List<DesktopWindow> {
    val arr = body.optJSONArray("windows") ?: return emptyList()
    return (0 until arr.length()).mapNotNull { i ->
        arr.optJSONObject(i)?.let { runCatching { desktopWindowFromJson(it) }.getOrNull() }
    }
}

private fun parseScreenBox(body: JSONObject): IntArray? =
    body.optJSONArray("screen")?.let {
        if (it.length() < 4) null else intArrayOf(it.getInt(0), it.getInt(1), it.getInt(2), it.getInt(3))
    }

private fun parseState(body: JSONObject): WorkspaceHudState {
    val arr = body.optJSONArray("workspaces") ?: return WorkspaceHudState(emptyList(), -1, null)
    val entries =
        (0 until arr.length()).mapNotNull { i ->
            val o = arr.optJSONObject(i) ?: return@mapNotNull null
            val id = o.optInt("id", Int.MIN_VALUE)
            if (id == Int.MIN_VALUE) null else WsEntry(id, o.optString("name"))
        }
    val activeId =
        if (body.isNull("activeWorkspace")) -1 else body.optInt("activeWorkspace", -1)
    // optString turns a JSON null into the text "null": the server sends
    // null for both while no window is focused.
    val title = if (body.isNull("activeTitle")) null else body.optString("activeTitle").ifBlank { null }
    val activeAddress = if (body.isNull("activeAddress")) null else body.optString("activeAddress")
    val windows =
        parseWindowArray(body).map { w ->
            if (w.address == activeAddress) w.copy(active = true) else w
        }
    return WorkspaceHudState(entries, activeId, title, windows, parseScreenBox(body))
}

/** Apply one socket2 event on top of the snapshot-derived state. */
private fun WorkspaceHudState.applyEvent(
    name: String,
    data: String,
): WorkspaceHudState =
    when (name) {
        "workspace" -> {
            val new = data.split(',').getOrNull(1)?.let { parseWorkspaceRef(it) }
            if (new == null) this else copy(activeId = new)
        }

        "activewindow" -> {
            // data = "class,title"; the title itself may contain commas.
            val parts = data.split(',', limit = 2)
            val title = parts.getOrNull(1)?.ifBlank { null } ?: parts[0].ifBlank { null }
            copy(activeTitle = title)
        }

        "createworkspace" -> {
            val id = parseWorkspaceRef(data) ?: return this
            if (workspaces.any { it.id == id }) {
                this
            } else {
                copy(workspaces = workspaces + WsEntry(id, data.substringAfter(':', id.toString())))
            }
        }

        "destroyworkspace" -> {
            val id = data.trim().toIntOrNull() ?: return this
            copy(workspaces = workspaces.filterNot { it.id == id })
        }

        "renameworkspace" -> {
            val parts = data.split(',', limit = 3)
            val id = parts.getOrNull(0)?.toIntOrNull() ?: return this
            val newName = parts.getOrNull(2) ?: return this
            copy(
                workspaces = workspaces.map { if (it.id == id) it.copy(name = newName) else it },
            )
        }

        else -> this
    }

private fun parseEvent(body: JSONObject): Pair<String, String>? {
    val name = body.optString("event")
    if (name.isEmpty()) return null
    return name to body.optString("data")
}

@Composable
fun WorkspaceHud(
    address: String,
    port: Int,
    modifier: Modifier = Modifier,
    croppedAddress: String? = null,
    onPullWindow: (DesktopWindow, IntArray?) -> Unit = { _, _ -> },
) {
    var hud by remember { mutableStateOf<WorkspaceHudState?>(null) }
    val scope = rememberCoroutineScope()

    LaunchedEffect(Unit) {
        withContext(Dispatchers.IO) {
            // The poll call itself waits ~100 ms per round; loop it with no
            // extra delay so subscription gaps stay sub-millisecond.
            while (isActive) {
                val packets = RustCore.pollIncomingPackets()
                for (raw in packets) {
                    val reduce: ((WorkspaceHudState?) -> WorkspaceHudState?)? =
                        runCatching {
                            val o = JSONObject(raw)
                            when (o.optString("type")) {
                                STATE_TYPE -> { _: WorkspaceHudState? ->
                                    parseState(o.optJSONObject("body") ?: JSONObject())
                                }

                                EVENT_TYPE -> { prev: WorkspaceHudState? ->
                                    val ev = parseEvent(o.optJSONObject("body") ?: JSONObject())
                                    if (ev != null && prev != null) {
                                        prev.applyEvent(ev.first, ev.second)
                                    } else {
                                        prev
                                    }
                                }

                                else -> null
                            }
                        }.getOrNull()
                    reduce?.let { hud = it(hud) }
                }
            }
        }
    }

    val state = hud ?: return
    if (state.workspaces.isEmpty() && state.windows.isEmpty()) return

    Column(
        modifier = modifier
            .clip(RoundedCornerShape(8.dp))
            .background(Color.Black.copy(alpha = 0.55f))
            .border(1.dp, Color.White.copy(alpha = 0.14f), RoundedCornerShape(8.dp))
            .padding(horizontal = 6.dp, vertical = 4.dp),
        verticalArrangement = Arrangement.spacedBy(3.dp),
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Row(
                modifier = Modifier.horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                state.workspaces.sortedBy { it.id }.forEach { ws ->
                    WorkspaceChip(
                        ws = ws,
                        active = ws.id == state.activeId,
                        onClick =
                            ws.switchAction(address, port)?.let { action ->
                                { scope.launch(Dispatchers.IO) { action() } }
                            },
                    )
                }
            }
            val title = state.activeTitle
            if (!title.isNullOrBlank()) {
                Text(
                    title,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    fontSize = 11.sp,
                    color = Color.White.copy(alpha = 0.85f),
                    modifier = Modifier.padding(start = 4.dp),
                )
            }
        }
        // E1: a window chip pulls that window into the stream (same crop path
        // the window picker uses), so "what's on the desktop" and "stream this"
        // collapse into one tap.
        if (state.windows.isNotEmpty()) {
            Row(
                modifier = Modifier.horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                state.windows.forEach { w ->
                    WindowChip(
                        window = w,
                        streamed = w.address == croppedAddress,
                        onPull = { onPullWindow(w, state.screen) },
                    )
                }
            }
        }
    }
}

/**
 * E1 window chip: tap to crop-stream this window; re-tap to return to full
 * desktop. UX2: a long title is cut in its middle, so both its start (the
 * document) and its end (often the app) show, the chip is at most 160 dp
 * wide, and a long-press shows the whole title; TalkBack reads it whole.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun WindowChip(
    window: DesktopWindow,
    streamed: Boolean,
    onPull: () -> Unit,
) {
    val context = LocalContext.current
    val shape = RoundedCornerShape(6.dp)
    val label = window.title.ifBlank { window.app.ifBlank { window.address } }
    val fullTitleLabel = stringResource(R.string.window_chip_full_title)
    Text(
        label,
        maxLines = 1,
        overflow = TextOverflow.MiddleEllipsis,
        fontSize = 11.sp,
        color = if (streamed) Color.Black else Color.White,
        modifier = Modifier
            .widthIn(max = 160.dp)
            .clip(shape)
            .then(
                if (streamed) {
                    Modifier.background(Color.White.copy(alpha = 0.9f))
                } else {
                    Modifier.border(1.dp, Color.White.copy(alpha = 0.35f), shape)
                },
            )
            .combinedClickable(
                onLongClickLabel = fullTitleLabel,
                onLongClick = { Toast.makeText(context, label, Toast.LENGTH_LONG).show() },
                onClick = onPull,
            )
            .semantics { contentDescription = label }
            .padding(horizontal = 8.dp, vertical = 2.dp),
    )
}

/** Super+N keybinds exist for workspaces 1..9; named ones are display-only. */
private fun WsEntry.switchAction(
    address: String,
    port: Int,
): (() -> Unit)? {
    val n = name.toIntOrNull() ?: id
    if (n !in 1..9) return null
    return {
        RustCore.hotkey(
            address,
            port,
            KeyEvent.KEYCODE_1 + (n - 1),
            KeyEvent.KEYCODE_META_LEFT,
        )
    }
}

@Composable
private fun WorkspaceChip(
    ws: WsEntry,
    active: Boolean,
    onClick: (() -> Unit)?,
) {
    val shape = RoundedCornerShape(6.dp)
    val label = ws.name.ifBlank { ws.id.toString() }
    Text(
        label,
        fontSize = 12.sp,
        color = if (active) Color.Black else Color.White,
        modifier = Modifier
            .clip(shape)
            .then(
                if (active) {
                    Modifier.background(Color.White.copy(alpha = 0.9f))
                } else {
                    Modifier.border(1.dp, Color.White.copy(alpha = 0.35f), shape)
                },
            )
            .padding(horizontal = 8.dp, vertical = 2.dp)
            .then(if (onClick != null) Modifier.clickable { onClick() } else Modifier),
    )
}
