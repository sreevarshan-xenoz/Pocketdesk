package dev.pocketdesk.android.stream

import android.view.KeyEvent
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.minimumInteractiveComponentSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.ui.plainErrorText
import dev.pocketdesk.android.ui.rememberLlHaptics
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Desktop shortcut bar (Tier 1 #3): keys a touchscreen cannot produce.
 * C9 groups it into System (Super, window switching, safety screen,
 * screenshots, Esc) and Workspaces (W1–9 — Super+1..9 drives Hyprland
 * workspace switching on the server). Key-cap labels stay literal by
 * design (see the i18n exemption note in strings.xml); only the group
 * captions are localized.
 *
 * Rendered as a dark, rounded floating pill rather than a theme-coloured
 * sheet: it lives on top of remote video (or the dock) where a surfaceVariant
 * fill is unreadable, and the fixed dark fill keeps the key-caps legible in
 * both light and dark themes without a second palette.
 */
@Composable
fun ShortcutBar(
    address: String,
    port: Int,
    modifier: Modifier = Modifier,
    // U2: what to say when a key did not reach the computer.
    onFailed: (String) -> Unit = {},
) {
    val content = Color.White
    Surface(
        modifier = modifier.shadow(8.dp, PillShape),
        shape = PillShape,
        color = Color.Black.copy(alpha = 0.62f),
        tonalElevation = 0.dp,
    ) {
        Row(
            modifier = Modifier
                .horizontalScroll(rememberScrollState())
                .padding(horizontal = 8.dp, vertical = 5.dp),
            horizontalArrangement = Arrangement.spacedBy(5.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            val press = KeyEvent.KEYCODE_META_LEFT
            GroupLabel(stringResource(R.string.shortcut_system), content)
            Shortcut("Super", content, onFailed) { RustCore.tapKey(address, port, press) }
            Shortcut("Alt+Tab", content, onFailed) {
                RustCore.hotkey(address, port, KeyEvent.KEYCODE_TAB, KeyEvent.KEYCODE_ALT_LEFT)
            }
            Shortcut("Ctrl+Alt+Del", content, onFailed) {
                RustCore.hotkey(
                    address,
                    port,
                    KeyEvent.KEYCODE_FORWARD_DEL,
                    KeyEvent.KEYCODE_CTRL_LEFT,
                    KeyEvent.KEYCODE_ALT_LEFT,
                )
            }
            Shortcut("PrtSc", content, onFailed) { RustCore.tapKey(address, port, KeyEvent.KEYCODE_SYSRQ) }
            Shortcut("Esc", content, onFailed) { RustCore.tapKey(address, port, KeyEvent.KEYCODE_ESCAPE) }
            Divider(content)
            GroupLabel(stringResource(R.string.shortcut_workspaces), content)
            (1..9).forEach { workspace ->
                Shortcut("W$workspace", content, onFailed) {
                    RustCore.hotkey(
                        address,
                        port,
                        KeyEvent.KEYCODE_1 + (workspace - 1),
                        press,
                    )
                }
            }
        }
    }
}

private val PillShape = RoundedCornerShape(18.dp)

@Composable
private fun GroupLabel(text: String, color: Color) {
    Text(
        text,
        style = MaterialTheme.typography.labelSmall,
        color = color.copy(alpha = 0.65f),
        modifier = Modifier.padding(horizontal = 4.dp),
    )
}

@Composable
private fun Divider(color: Color) {
    Box(
        modifier = Modifier
            .width(1.dp)
            .height(20.dp)
            .padding(horizontal = 4.dp)
            .background(color.copy(alpha = 0.3f)),
    )
}

@Composable
private fun Shortcut(
    label: String,
    color: Color,
    onFailed: (String) -> Unit,
    action: () -> Result<Unit>,
) {
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val resources = LocalResources.current
    val haptics = rememberLlHaptics()
    Box(
        modifier = Modifier
            // 48 dp to the touch, however small it looks (U4).
            .minimumInteractiveComponentSize()
            .height(36.dp)
            .widthIn(min = 36.dp)
            .clip(RoundedCornerShape(18.dp))
            .background(color.copy(alpha = 0.14f))
            .clickable(role = Role.Button) {
                haptics.tap()
                scope.launch(Dispatchers.IO) {
                    action().onFailure {
                        val said = resources.getString(
                            R.string.shortcut_failed,
                            label,
                            context.plainErrorText(it.message),
                        )
                        withContext(Dispatchers.Main) { onFailed(said) }
                    }
                }
            }
            .padding(horizontal = 12.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            label,
            fontSize = 12.sp,
            textAlign = TextAlign.Center,
            color = color,
        )
    }
}
