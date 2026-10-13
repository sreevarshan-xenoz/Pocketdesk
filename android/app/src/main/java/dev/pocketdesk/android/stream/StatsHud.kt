package dev.pocketdesk.android.stream

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.pocketdesk.android.bridge.RustCore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import org.json.JSONObject

/**
 * Streaming telemetry HUD overlay (Tier 1 #1): fps / bitrate / RTT /
 * end-to-end latency / frame drops, refreshed on a fixed interval from the
 * Rust core, over a caption naming the sample basis of the rate and link
 * figures. When an [address]/[controlPort] are given, the desktop's battery is
 * polled slowly alongside (Tier-2 #11). Place on top of [RemoteDesktopView]
 * inside a Box.
 *
 * A [FlowRow], not a plain Row: on a narrow phone in landscape the six items
 * are wider than the screen, and a Row would squash the trailing ones into a
 * one-character-per-line column.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun StatsHud(
    modifier: Modifier = Modifier,
    refreshMs: Long = 500,
    address: String? = null,
    controlPort: Int = 1716,
) {
    var stats by remember { mutableStateOf<RustCore.StreamingStats?>(null) }
    var battery by remember { mutableStateOf<String?>(null) }

    LaunchedEffect(refreshMs) {
        while (true) {
            stats = runCatching { RustCore.streamingStats() }.getOrNull()
            delay(refreshMs)
        }
    }

    LaunchedEffect(address, controlPort) {
        val host = address ?: return@LaunchedEffect
        battery = null
        while (true) {
            battery = withContext(Dispatchers.IO) {
                runCatching {
                    val body = JSONObject(RustCore.getBattery(host, controlPort).getOrThrow())
                    when {
                        body.optBoolean("noBattery") -> null
                        else -> {
                            val charge = body.optInt("currentCharge", -1)
                            if (charge < 0) {
                                null
                            } else {
                                "$charge%" +
                                    if (body.optBoolean("isCharging")) "⚡" else ""
                            }
                        }
                    }
                }.getOrNull()
            }
            delay(15_000)
        }
    }

    val s = stats ?: return
    Column(
        modifier = modifier
            .clip(RoundedCornerShape(8.dp))
            .background(Color.Black.copy(alpha = 0.55f))
            .border(1.dp, Color.White.copy(alpha = 0.14f), RoundedCornerShape(8.dp))
            .padding(horizontal = 10.dp, vertical = 6.dp),
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        FlowRow(
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            verticalArrangement = Arrangement.spacedBy(2.dp),
            maxItemsInEachRow = 6,
        ) {
            // A number measured over half a second reads exactly like one
            // measured over three, so the ones still filling their window are
            // dimmed until they are not (roadmap 2056).
            val settled = s.ratesSettled
            HudItem("fps", "%.0f".format(s.fps), settled)
            HudItem("kbps", "%,d".format(s.bitrateKbps), settled)
            HudItem("rtt", "${s.rttMs}ms", s.rttSamples > 0)
            HudItem("e2e", "${s.e2eLatencyMs}ms")
            HudItem("drops", "${s.frameDrops}")
            battery?.let { HudItem("desk", it) }
        }
        Text(
            text = basis(s),
            maxLines = 1,
            style = MaterialTheme.typography.labelSmall.copy(
                fontFamily = FontFamily.Monospace,
                fontSize = 10.sp,
                color = Color.White.copy(alpha = 0.55f),
            ),
        )
    }
}

/**
 * What the numbers above were measured on. A rate is a division by some span of
 * received video and the link figure is a median of some number of polls;
 * quoting either without saying how much of a sample stood behind it is how a
 * HUD ends up presenting a guess as a reading.
 */
private fun basis(s: RustCore.StreamingStats): String {
    val rates = if (s.rateWindowMs == 0L) "no video yet"
    else "rates over %.1fs".format(s.rateWindowMs / 1000.0)
    val rtt = if (s.rttSamples == 0) "rtt unsampled" else "rtt median of %d".format(s.rttSamples)
    return "$rates · $rtt"
}

@Composable
private fun HudItem(label: String, value: String, confident: Boolean = true) {
    Text(
        text = "$label $value",
        maxLines = 1,
        style = MaterialTheme.typography.labelSmall.copy(
            fontFamily = FontFamily.Monospace,
            fontSize = 11.sp,
            color = if (confident) Color.White else Color.White.copy(alpha = 0.45f),
        ),
    )
}
