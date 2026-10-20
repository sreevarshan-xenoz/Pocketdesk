package dev.pocketdesk.android.ui

import android.content.Context
import android.os.Build
import android.os.SystemClock
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import androidx.compose.foundation.layout.Column
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.stream.TalkTimer
import kotlinx.coroutines.delay

/**
 * The talk timer in Remote's header (H2): "Timer" picks a length, then the
 * button counts down (and past the end); the phone buzzes at five minutes
 * left, one minute left and the end, as Settings "Talk timer buzzes" lets
 * it. A tap on the running timer stops it.
 */
@Composable
fun TalkTimerButton() {
    val context = LocalContext.current
    // On the uptime clock, which sleep does not stop; kept across rotation.
    var endsAt by rememberSaveable { mutableLongStateOf(0L) }
    var totalSec by rememberSaveable { mutableLongStateOf(0L) }
    var choosing by remember { mutableStateOf(false) }
    var now by remember { mutableLongStateOf(SystemClock.elapsedRealtime()) }

    LaunchedEffect(endsAt) {
        if (endsAt == 0L) return@LaunchedEffect
        var before = (endsAt - SystemClock.elapsedRealtime()) / 1000 + 1
        while (true) {
            now = SystemClock.elapsedRealtime()
            val left = Math.floorDiv(endsAt - now, 1000L)
            val pulses = TalkTimer.pulses(totalSec, before, left)
            if (pulses > 0 && Prefs.talkTimerBuzz(context)) buzz(context, pulses)
            before = left
            delay(TICK_MS)
        }
    }

    if (endsAt == 0L) {
        TextButton(onClick = { choosing = true }) { Text(stringResource(R.string.talk_timer)) }
    } else {
        val left = Math.floorDiv(endsAt - now, 1000L)
        val shown = TalkTimer.text(left)
        val description = stringResource(R.string.talk_timer_running, shown)
        TextButton(
            onClick = { endsAt = 0L },
            modifier = androidx.compose.ui.Modifier.semantics {
                contentDescription = description
            },
        ) {
            Text(
                shown,
                color = if (left < 0) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.primary,
            )
        }
    }

    if (choosing) {
        AlertDialog(
            onDismissRequest = { choosing = false },
            title = { Text(stringResource(R.string.talk_timer_title)) },
            text = {
                Column {
                    TalkTimer.MINUTES.forEach { minutes ->
                        TextButton(onClick = {
                            Prefs.setTalkTimerMinutes(context, minutes)
                            totalSec = minutes * 60L
                            endsAt = SystemClock.elapsedRealtime() + totalSec * 1000
                            choosing = false
                        }) {
                            val mark = if (minutes == Prefs.talkTimerMinutes(context)) " ✓" else ""
                            Text(pluralStringResource(R.plurals.talk_timer_minutes, minutes, minutes) + mark)
                        }
                    }
                }
            },
            confirmButton = {},
            dismissButton = { TextButton(onClick = { choosing = false }) { Text(stringResource(R.string.cancel)) } },
        )
    }
}

private const val TICK_MS = 250L

/** [count] short buzzes: one at five minutes, two at one, three at the end. */
private fun buzz(context: Context, count: Int) {
    val vibrator = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
        context.getSystemService(VibratorManager::class.java)?.defaultVibrator
    } else {
        @Suppress("DEPRECATION")
        context.getSystemService(Vibrator::class.java)
    } ?: return
    // Off, on, in pairs: 220 ms buzzes 180 ms apart.
    val timings = LongArray(count * 2) { i -> if (i % 2 == 1) 220L else if (i == 0) 0L else 180L }
    runCatching { vibrator.vibrate(VibrationEffect.createWaveform(timings, -1)) }
}
