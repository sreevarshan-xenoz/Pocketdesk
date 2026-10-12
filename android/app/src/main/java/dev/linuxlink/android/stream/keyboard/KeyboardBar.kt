package dev.linuxlink.android.stream.keyboard

import android.view.KeyEvent
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
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
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.linuxlink.android.R
import dev.linuxlink.android.ui.LlHaptics

/** The bar's fixed height; the session leaves this much room above the keyboard. */
val KeyboardBarHeight = 104.dp

private val KeyShape = RoundedCornerShape(8.dp)

/**
 * The accessory rows docked above the phone keyboard (spec K7). Row one has
 * the latching modifiers, Esc, Tab, the arrows and a hide button. Row two
 * scrolls through the navigation keys, F1-F12 and Ctrl+Alt+Del. Key-cap
 * labels are literal (Esc, F5), like the shortcut bar's. A modifier is tapped
 * to latch it for one key and double-tapped to lock it; [latchVersion] changes
 * whenever the latch does, so the caps redraw.
 */
@Composable
fun KeyboardBar(
    keyboard: KeyboardSession,
    latchVersion: Int,
    haptics: LlHaptics,
    onHide: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val key: (Int) -> Unit = { code ->
        haptics.tap()
        keyboard.key(code)
    }
    Column(
        modifier = modifier
            .fillMaxWidth()
            .height(KeyboardBarHeight)
            .background(Color(0xFF15191C))
            .padding(horizontal = 4.dp, vertical = 4.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .weight(1f)
                .horizontalScroll(rememberScrollState()),
            horizontalArrangement = Arrangement.spacedBy(4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            for (mod in KeyModifier.entries) {
                ModifierCap(
                    key = mod,
                    state = keyboard.latch.state(mod),
                    latchVersion = latchVersion,
                    onTap = { haptics.toggle(); keyboard.latch.tap(mod) },
                    onLock = { haptics.longPress(); keyboard.latch.lock(mod) },
                )
            }
            Cap("Esc") { key(KeyEvent.KEYCODE_ESCAPE) }
            Cap("Tab") { key(KeyEvent.KEYCODE_TAB) }
            Cap("←") { key(KeyEvent.KEYCODE_DPAD_LEFT) }
            Cap("↑") { key(KeyEvent.KEYCODE_DPAD_UP) }
            Cap("↓") { key(KeyEvent.KEYCODE_DPAD_DOWN) }
            Cap("→") { key(KeyEvent.KEYCODE_DPAD_RIGHT) }
            Cap("⌄", stringResource(R.string.keyboard_hide)) { haptics.tap(); onHide() }
        }
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .weight(1f)
                .horizontalScroll(rememberScrollState()),
            horizontalArrangement = Arrangement.spacedBy(4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
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
    }
}

@Composable
private fun Cap(label: String, description: String? = null, onClick: () -> Unit) {
    Box(
        modifier = Modifier
            .widthIn(min = 44.dp)
            .clip(KeyShape)
            .background(Color.White.copy(alpha = 0.12f))
            .pointerInput(Unit) { detectTapGestures(onTap = { onClick() }) }
            .then(if (description != null) Modifier.semantics { contentDescription = description } else Modifier)
            .padding(horizontal = 12.dp, vertical = 9.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(label, color = Color.White, fontSize = 14.sp)
    }
}

/**
 * A modifier key: dim when off, filled when latched for one key, filled
 * with a ring when locked.
 */
@Composable
private fun ModifierCap(
    key: KeyModifier,
    state: LatchState,
    latchVersion: Int,
    onTap: () -> Unit,
    onLock: () -> Unit,
) {
    // Read so the cap recomposes when the latch changes.
    latchVersion.hashCode()
    val accent = MaterialTheme.colorScheme.primary
    val fill = when (state) {
        LatchState.Off -> Color.White.copy(alpha = 0.12f)
        LatchState.Once, LatchState.Locked -> accent
    }
    val text = if (state == LatchState.Off) Color.White else MaterialTheme.colorScheme.onPrimary
    Box(
        modifier = Modifier
            .widthIn(min = 52.dp)
            .clip(KeyShape)
            .background(fill)
            .then(
                if (state == LatchState.Locked) {
                    Modifier.border(2.dp, Color.White, KeyShape)
                } else {
                    Modifier
                },
            )
            .pointerInput(key) { detectTapGestures(onTap = { onTap() }, onDoubleTap = { onLock() }) }
            .padding(horizontal = 12.dp, vertical = 9.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(key.name, color = text, fontSize = 14.sp)
    }
}
