package dev.linuxlink.android.stream.keyboard

import android.os.SystemClock
import android.view.KeyEvent
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.linuxlink.android.R
import dev.linuxlink.android.ui.LlHaptics

/**
 * The bar's height: two rows, or one when [compact] (a short landscape
 * window, where two rows plus the keyboard would leave the stream no room).
 * The session leaves this much room above the keyboard.
 */
fun keyboardBarHeight(compact: Boolean): Dp = if (compact) 56.dp else 104.dp

private val KeyShape = RoundedCornerShape(8.dp)

/**
 * The accessory rows docked above the phone keyboard (spec K7). The latching
 * modifiers, Esc, Tab, the arrows and a hide button come first. Then come the
 * navigation keys, F1-F12 and Ctrl+Alt+Del, on a second scrolling row, or on
 * the same row when [compact]. Key-cap labels are literal (Esc, F5), like the
 * shortcut bar's.
 *
 * A modifier latches the moment it is tapped, and a second tap within the
 * double-tap window locks it ([ModifierLatch.tap]). [latchVersion] changes
 * whenever the latch does, so the caps redraw.
 */
@Composable
fun KeyboardBar(
    keyboard: KeyboardSession,
    latchVersion: Int,
    haptics: LlHaptics,
    compact: Boolean,
    onHide: () -> Unit,
    onCompose: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val key: (Int) -> Unit = { code ->
        haptics.tap()
        keyboard.key(code)
    }
    val first: @Composable RowScope.() -> Unit = {
        for (mod in KeyModifier.entries) {
            ModifierCap(mod, keyboard.latch.state(mod), latchVersion) {
                haptics.toggle()
                keyboard.latch.tap(mod, SystemClock.uptimeMillis())
            }
        }
        Cap("Esc") { key(KeyEvent.KEYCODE_ESCAPE) }
        Cap("Tab") { key(KeyEvent.KEYCODE_TAB) }
        Cap("←") { key(KeyEvent.KEYCODE_DPAD_LEFT) }
        Cap("↑") { key(KeyEvent.KEYCODE_DPAD_UP) }
        Cap("↓") { key(KeyEvent.KEYCODE_DPAD_DOWN) }
        Cap("→") { key(KeyEvent.KEYCODE_DPAD_RIGHT) }
        Cap("✎", stringResource(R.string.send_text_title)) { haptics.tap(); onCompose() }
        Cap("⌄", stringResource(R.string.keyboard_hide)) { haptics.tap(); onHide() }
    }
    val second: @Composable RowScope.() -> Unit = {
        Cap("Home") { key(KeyEvent.KEYCODE_MOVE_HOME) }
        Cap("End") { key(KeyEvent.KEYCODE_MOVE_END) }
        Cap("PgUp") { key(KeyEvent.KEYCODE_PAGE_UP) }
        Cap("PgDn") { key(KeyEvent.KEYCODE_PAGE_DOWN) }
        Cap("Del") { key(KeyEvent.KEYCODE_FORWARD_DEL) }
        Cap("Ins") { key(KeyEvent.KEYCODE_INSERT) }
        for (n in 1..12) {
            Cap("F$n") { key(KeyEvent.KEYCODE_F1 + n - 1) }
        }
        Cap("Ctrl+Alt+Del") {
            haptics.tap()
            keyboard.combo(KeyEvent.KEYCODE_FORWARD_DEL, KeyModifier.Ctrl, KeyModifier.Alt)
        }
    }
    Column(
        modifier = modifier
            .fillMaxWidth()
            .height(keyboardBarHeight(compact))
            .background(Color(0xFF15191C))
            .padding(horizontal = 4.dp, vertical = 4.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        if (compact) {
            KeyRow {
                first()
                second()
            }
        } else {
            KeyRow(first)
            KeyRow(second)
        }
    }
}

@Composable
private fun ColumnScope.KeyRow(content: @Composable RowScope.() -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .weight(1f)
            .horizontalScroll(rememberScrollState()),
        horizontalArrangement = Arrangement.spacedBy(4.dp),
        verticalAlignment = Alignment.CenterVertically,
        content = content,
    )
}

@Composable
private fun Cap(label: String, description: String? = null, onClick: () -> Unit) {
    Box(
        modifier = Modifier
            .widthIn(min = 44.dp)
            .clip(KeyShape)
            .background(Color.White.copy(alpha = 0.12f))
            .clickable(role = Role.Button, onClick = onClick)
            .then(if (description != null) Modifier.semantics { contentDescription = description } else Modifier)
            .padding(horizontal = 12.dp, vertical = 9.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(label, color = Color.White, fontSize = 14.sp)
    }
}

/**
 * A modifier key: dim when off, filled when latched for one key, filled
 * with a ring when locked. TalkBack reads the state.
 */
@Composable
private fun ModifierCap(key: KeyModifier, state: LatchState, latchVersion: Int, onTap: () -> Unit) {
    // Read so the cap recomposes when the latch changes.
    latchVersion.hashCode()
    val accent = MaterialTheme.colorScheme.primary
    val fill = if (state == LatchState.Off) Color.White.copy(alpha = 0.12f) else accent
    val text = if (state == LatchState.Off) Color.White else MaterialTheme.colorScheme.onPrimary
    val stateText = stringResource(
        when (state) {
            LatchState.Off -> R.string.keyboard_mod_off
            LatchState.Once -> R.string.keyboard_mod_once
            LatchState.Locked -> R.string.keyboard_mod_locked
        },
    )
    Box(
        modifier = Modifier
            .widthIn(min = 52.dp)
            .clip(KeyShape)
            .background(fill)
            .then(if (state == LatchState.Locked) Modifier.border(2.dp, Color.White, KeyShape) else Modifier)
            .clickable(role = Role.Switch, onClick = onTap)
            .semantics { stateDescription = stateText }
            .padding(horizontal = 12.dp, vertical = 9.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(key.name, color = text, fontSize = 14.sp)
    }
}
