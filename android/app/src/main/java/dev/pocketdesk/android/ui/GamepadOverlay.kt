package dev.pocketdesk.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.waitForUpOrCancellation
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.pocketdesk.android.R
import dev.pocketdesk.android.stream.GamepadState
import dev.pocketdesk.android.stream.PadButton
import kotlin.math.hypot
import kotlin.math.roundToInt

/**
 * The phone as a gamepad (E4), over the stream: a stick and the D-pad on the
 * left, A, B, X, Y and the right stick on the right, the bumpers above, Select
 * and Start between. Each button says what it does on the computer.
 * [onChange] is called after each change to [pad]; the screen sends it.
 */
@Composable
fun GamepadOverlay(
    pad: GamepadState,
    onChange: () -> Unit,
    onClose: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Box(
        modifier
            .fillMaxSize()
            // What the controls leave is the overlay's: a palm between them
            // must not reach the computer through the stream below (E4
            // review). Children see each event first, in the Main pass.
            .pointerInput(Unit) {
                awaitPointerEventScope {
                    while (true) awaitPointerEvent().changes.forEach { it.consume() }
                }
            },
    ) {
        IconButton(
            onClick = onClose,
            modifier = Modifier
                .align(Alignment.TopEnd)
                .padding(8.dp)
                .clip(CircleShape)
                .background(Scrim),
        ) {
            LlIcon(LlIcons.Close, stringResource(R.string.gamepad_close), tint = Color.White)
        }
        Row(
            modifier = Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .padding(horizontal = 8.dp, vertical = 12.dp),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.Bottom,
        ) {
            Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(10.dp)) {
                PadKey("LB", stringResource(R.string.key_page_up), PadButton.LB, pad, onChange, Bumper, 92.dp, 40.dp)
                Stick(left = true, caption = stringResource(R.string.gamepad_mouse), pad = pad, onChange = onChange)
                Dpad(pad, onChange)
            }
            Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(10.dp)) {
                PadKey("Select", stringResource(R.string.key_backspace), PadButton.SELECT, pad, onChange, Bumper, 84.dp, 40.dp)
                PadKey("Start", stringResource(R.string.gamepad_click), PadButton.START, pad, onChange, Bumper, 84.dp, 40.dp)
            }
            Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(10.dp)) {
                PadKey("RB", stringResource(R.string.key_page_down), PadButton.RB, pad, onChange, Bumper, 92.dp, 40.dp)
                FaceButtons(pad, onChange)
                Stick(left = false, caption = stringResource(R.string.gamepad_scroll), pad = pad, onChange = onChange)
            }
        }
    }
}

private val Scrim = Color.Black.copy(alpha = 0.45f)
private val Held = Color.White.copy(alpha = 0.45f)
private val Bumper = RoundedCornerShape(12.dp)

@Composable
private fun Dpad(pad: GamepadState, onChange: () -> Unit) {
    val key = 44.dp
    Column(horizontalAlignment = Alignment.CenterHorizontally) {
        PadKey("↑", null, PadButton.UP, pad, onChange, CircleShape, key, key)
        Row {
            PadKey("←", null, PadButton.LEFT, pad, onChange, CircleShape, key, key)
            Spacer(Modifier.width(key))
            PadKey("→", null, PadButton.RIGHT, pad, onChange, CircleShape, key, key)
        }
        PadKey("↓", null, PadButton.DOWN, pad, onChange, CircleShape, key, key)
    }
}

@Composable
private fun FaceButtons(pad: GamepadState, onChange: () -> Unit) {
    val key = 50.dp
    Column(horizontalAlignment = Alignment.CenterHorizontally) {
        PadKey("Y", stringResource(R.string.key_tab), PadButton.Y, pad, onChange, CircleShape, key, key)
        Row {
            PadKey("X", stringResource(R.string.key_space), PadButton.X, pad, onChange, CircleShape, key, key)
            Spacer(Modifier.width(key))
            PadKey("B", stringResource(R.string.key_escape), PadButton.B, pad, onChange, CircleShape, key, key)
        }
        PadKey("A", stringResource(R.string.key_enter), PadButton.A, pad, onChange, CircleShape, key, key)
    }
}

/**
 * One button: held while touched, several at once with other fingers; a
 * screen reader's click presses and lets go.
 */
@Composable
private fun PadKey(
    label: String,
    caption: String?,
    bit: Int,
    pad: GamepadState,
    onChange: () -> Unit,
    shape: Shape,
    width: Dp,
    height: Dp,
) {
    var held by remember { mutableStateOf(false) }
    fun set(down: Boolean) {
        held = down
        if (pad.press(bit, down)) onChange()
    }
    val description = if (caption != null) "$label, $caption" else label
    Box(
        contentAlignment = Alignment.Center,
        modifier = Modifier
            .size(width, height)
            .padding(2.dp)
            .clip(shape)
            .background(if (held) Held else Scrim)
            .semantics {
                role = Role.Button
                contentDescription = description
                onClick {
                    set(true)
                    set(false)
                    true
                }
            }
            .pointerInput(bit) {
                awaitEachGesture {
                    awaitFirstDown(requireUnconsumed = false).consume()
                    set(true)
                    try {
                        waitForUpOrCancellation()?.consume()
                    } finally {
                        set(false)
                    }
                }
            },
    ) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Text(label, color = Color.White, fontSize = 14.sp)
            if (caption != null) Text(caption, color = Color.White.copy(alpha = 0.8f), fontSize = 9.sp, maxLines = 1)
        }
    }
}

/** A stick: dragged from its centre, back to it when let go. */
@Composable
private fun Stick(left: Boolean, caption: String, pad: GamepadState, onChange: () -> Unit) {
    val diameter = 116.dp
    val knob = 44.dp
    var at by remember { mutableStateOf(Offset.Zero) }
    Box(
        contentAlignment = Alignment.Center,
        modifier = Modifier
            .size(diameter)
            .clip(CircleShape)
            .background(Scrim)
            .semantics { contentDescription = caption }
            .pointerInput(left) {
                val radius = size.width / 2f
                fun moveTo(position: Offset) {
                    val v = (position - Offset(radius, radius)) / radius
                    val length = hypot(v.x, v.y)
                    val held = if (length > 1f) v / length else v
                    at = held * radius
                    if (pad.stick(left, held.x, held.y)) onChange()
                }
                awaitEachGesture {
                    val down = awaitFirstDown(requireUnconsumed = false)
                    down.consume()
                    moveTo(down.position)
                    try {
                        while (true) {
                            val change = awaitPointerEvent().changes.firstOrNull { it.id == down.id } ?: break
                            if (!change.pressed) break
                            change.consume()
                            moveTo(change.position)
                        }
                    } finally {
                        at = Offset.Zero
                        if (pad.stick(left, 0f, 0f)) onChange()
                    }
                }
            },
    ) {
        Text(caption, color = Color.White.copy(alpha = 0.7f), fontSize = 10.sp, modifier = Modifier.align(Alignment.TopCenter).padding(top = 6.dp))
        Box(
            Modifier
                .offset { IntOffset(at.x.roundToInt(), at.y.roundToInt()) }
                .size(knob)
                .clip(CircleShape)
                .background(Held),
        )
    }
}
