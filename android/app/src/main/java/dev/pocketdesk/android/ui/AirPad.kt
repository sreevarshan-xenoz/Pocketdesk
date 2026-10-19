package dev.pocketdesk.android.ui

import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.stream.AirMouse
import dev.pocketdesk.android.stream.PadOutput
import dev.pocketdesk.android.stream.launchPadSend
import dev.pocketdesk.android.stream.sendPadOutputs
import kotlin.math.abs

/** Whether this phone has a gyroscope for the air mouse (F4). */
fun hasGyroscope(context: android.content.Context): Boolean =
    context.getSystemService(SensorManager::class.java)?.getDefaultSensor(Sensor.TYPE_GYROSCOPE) != null

/**
 * The air mouse (F4): held, the phone's turn moves the computer's pointer
 * through the trackpad's own moves; a short tap without moving is a click.
 * The gyroscope is listened to only while the pad is held.
 */
@Composable
fun AirPad(address: String, port: Int, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    Prefs.observedVersion()
    val speed = Prefs.airMouseSpeed(context)
    val sensors = remember { context.getSystemService(SensorManager::class.java) }
    val gyroscope = remember { sensors?.getDefaultSensor(Sensor.TYPE_GYROSCOPE) }
    var held by remember { mutableStateOf(false) }

    fun send(outputs: List<PadOutput>) = sendPadOutputs(address, port, outputs) { launchPadSend(it) }

    // The pad's own counts: how far this hold moved, for telling a tap.
    val moved = remember { intArrayOf(0) }
    val air = remember(speed) { AirMouse(speed) }
    val listener = remember(air) {
        object : SensorEventListener {
            private var lastNs = 0L

            override fun onSensorChanged(event: SensorEvent) {
                val dt = if (lastNs == 0L) 0f else (event.timestamp - lastNs) / 1e9f
                lastNs = event.timestamp
                val (dx, dy) = air.step(rateX = event.values[0], rateZ = event.values[2], dtSec = dt)
                if (dx != 0 || dy != 0) {
                    moved[0] += abs(dx) + abs(dy)
                    send(listOf(PadOutput.Move(dx, dy)))
                }
            }

            override fun onAccuracyChanged(sensor: Sensor?, accuracy: Int) = Unit

            fun restart() {
                lastNs = 0L
            }
        }
    }
    DisposableEffect(held) {
        if (held && gyroscope != null) {
            air.reset()
            listener.restart()
            sensors?.registerListener(listener, gyroscope, SensorManager.SENSOR_DELAY_GAME)
        }
        onDispose { sensors?.unregisterListener(listener) }
    }

    Surface(
        modifier = modifier
            .padding(12.dp)
            .pointerInput(address to port) {
                awaitEachGesture {
                    val down = awaitFirstDown(requireUnconsumed = false)
                    down.consume()
                    moved[0] = 0
                    held = true
                    var up = down
                    while (true) {
                        val event = awaitPointerEvent()
                        event.changes.forEach { it.consume() }
                        val change = event.changes.firstOrNull { it.id == down.id } ?: break
                        up = change
                        if (!change.pressed) break
                    }
                    held = false
                    val tap = up.uptimeMillis - down.uptimeMillis < AirMouse.TAP_MS && moved[0] < AirMouse.TAP_PX
                    if (tap) send(listOf(PadOutput.Click(0)))
                }
            },
        shape = RoundedCornerShape(24.dp),
        color = if (held) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.surfaceVariant,
        tonalElevation = 3.dp,
    ) {
        Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            Text(
                stringResource(if (gyroscope == null) R.string.air_none else R.string.air_hint),
                textAlign = TextAlign.Center,
                style = MaterialTheme.typography.titleMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(24.dp),
            )
        }
    }
}
