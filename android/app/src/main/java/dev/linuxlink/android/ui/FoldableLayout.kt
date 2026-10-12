package dev.linuxlink.android.ui

import android.app.Activity
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
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
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.window.layout.FoldingFeature
import androidx.window.layout.WindowInfoTracker
import androidx.window.layout.WindowLayoutInfo
import dev.linuxlink.android.R
import dev.linuxlink.android.Prefs
import dev.linuxlink.android.stream.ShortcutBar
import dev.linuxlink.android.stream.TrackpadGesture
import dev.linuxlink.android.stream.sendPadOutputs
import androidx.compose.ui.platform.LocalContext
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch

/**
 * How a session splits the window when the dual-pane layout is active:
 * [Vertical] = a vertical hinge → left|right columns (stream beside dock);
 * [Horizontal] = a horizontal hinge → top/bottom rows; [None] = no split.
 */
enum class PaneSplit { None, Vertical, Horizontal }

/** User override for the dual-pane layout; [Auto] follows the detected posture. */
enum class PaneMode { Auto, Dual, Single }

/**
 * Observe the device's folding posture and report how the session should split
 * around the hinge. Empty ([PaneSplit.None]) on a phone, on a fold-flat device
 * with no crease, and whenever [WindowInfoTracker] is unavailable — the caller
 * then falls back to a size-based tablet breakpoint.
 */
@Composable
fun rememberFoldingSplit(activity: Activity?): PaneSplit {
    var split by remember { mutableStateOf(PaneSplit.None) }
    LaunchedEffect(activity) {
        val act = activity
        if (act == null) {
            split = PaneSplit.None
            return@LaunchedEffect
        }
        runCatching {
            WindowInfoTracker.getOrCreate(act)
                .windowLayoutInfo(act)
                .collect { info ->
                    val fold = (info as? WindowLayoutInfo)
                        ?.displayFeatures
                        ?.firstOrNull { it is FoldingFeature } as? FoldingFeature
                    split = when (fold?.orientation) {
                        FoldingFeature.Orientation.VERTICAL -> PaneSplit.Vertical
                        FoldingFeature.Orientation.HORIZONTAL -> PaneSplit.Horizontal
                        else -> PaneSplit.None
                    }
                }
        }
            .onFailure { split = PaneSplit.None }
    }
    return split
}

/**
 * Standalone indirect-control surface for the dock pane (R4 E6): a trackpad the
 * user drags on while the desktop is fully visible on the other pane, so the
 * finger never covers the remote view — the problem RustDesk's stretched-phone
 * tablet layout has. Drag moves the cursor (relative motion), a tap clicks,
 * two fingers scroll. Sends go through one ordered dispatcher like the in-view
 * trackpad so packets leave in gesture order.
 */
@Composable
fun TrackpadDock(
    address: String,
    port: Int,
    modifier: Modifier = Modifier,
) {
    val scope = rememberCoroutineScope()
    val ordered = remember { Dispatchers.IO.limitedParallelism(1) }
    fun send(block: () -> Unit) = scope.launch(ordered) { block() }
    Prefs.observedVersion()
    val sensitivity = Prefs.trackpadSensitivity(LocalContext.current)

    Surface(
        modifier = modifier,
        color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.9f),
        tonalElevation = 3.dp,
    ) {
        Box(contentAlignment = Alignment.Center) {
            Text(
                stringResource(R.string.trackpad_hint),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.labelMedium,
                textAlign = TextAlign.Center,
                modifier = Modifier.fillMaxWidth().padding(16.dp),
            )
            Box(
                modifier =
                    Modifier
                        .fillMaxSize()
                        .pointerInput(address to port to sensitivity) {
                            // F3: the in-stream trackpad's gesture engine, so
                            // taps by finger count, tap-and-drag and speed
                            // behave the same here.
                            val pad = TrackpadGesture(slop = viewConfiguration.touchSlop, sensitivity = sensitivity)
                            awaitEachGesture {
                                val down = awaitFirstDown(requireUnconsumed = false)
                                pad.down(down.uptimeMillis)
                                var tracked = down.id
                                var last = down.position
                                while (true) {
                                    val event = awaitPointerEvent()
                                    event.changes.forEach { it.consume() }
                                    val fingers = event.changes.count { it.pressed }
                                    val change = event.changes.firstOrNull { it.pressed }
                                    if (change == null) {
                                        sendPadOutputs(address, port, pad.up(event.changes.first().uptimeMillis)) {
                                            send(it)
                                        }
                                        break
                                    }
                                    // A different finger leading is not motion.
                                    if (change.id != tracked) {
                                        tracked = change.id
                                        last = change.position
                                    }
                                    val delta = change.position - last
                                    last = change.position
                                    sendPadOutputs(address, port, pad.move(delta.x, delta.y, fingers)) {
                                        send(it)
                                    }
                                }
                            }
                        },
            )
        }
    }
}

/** The dock pane: a trackpad filling the pane, with the desktop shortcut bar pinned below it. */
@Composable
fun ControlDock(
    address: String,
    port: Int,
    modifier: Modifier = Modifier,
) {
    Column(modifier) {
        TrackpadDock(address, port, modifier = Modifier.weight(1f).fillMaxWidth())
        ShortcutBar(address, port, modifier = Modifier.fillMaxWidth())
    }
}
