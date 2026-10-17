package dev.pocketdesk.android.stream

import android.os.SystemClock
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import org.json.JSONObject

/**
 * Streaming telemetry HUD overlay (Tier 1 #1). With [details] off (the
 * default, U4) it is one coloured dot and a word for the connection
 * ([LinkQuality]), shown only while the stream is [live]; on, the numbers:
 * fps / bitrate / RTT /
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
    details: Boolean = true,
    live: Boolean = true,
) {
    var stats by remember { mutableStateOf<RustCore.StreamingStats?>(null) }
    var battery by remember { mutableStateOf<String?>(null) }
    var recentDrops by remember { mutableLongStateOf(0L) }
    val drops = remember { LinkQuality.DropWindow(spanMs = 3_000) }

    LaunchedEffect(refreshMs) {
        while (true) {
            stats = runCatching { RustCore.streamingStats() }.getOrNull()
            stats?.let {
                drops.add(SystemClock.elapsedRealtime(), it.frameDrops)
                recentDrops = drops.recent()
            }
            delay(refreshMs)
        }
    }

    LaunchedEffect(address, controlPort, details) {
        // The computer's battery is one of the details.
        if (!details) return@LaunchedEffect
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
    if (!details) {
        // Only a stream that is up has a connection to judge: a refused or
        // dropped one keeps its last numbers, which would read as "poor".
        if (!live) return
        QualityDot(LinkQuality.of(s.rttMs, s.rttSamples, s.e2eLatencyMs, recentDrops), modifier)
        return
    }
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
            HudItem(stringResource(R.string.hud_fps), "%.0f".format(s.fps), settled)
            HudItem(stringResource(R.string.hud_kbps), "%,d".format(s.bitrateKbps), settled)
            HudItem(stringResource(R.string.hud_rtt), "${s.rttMs} ms", s.rttSamples > 0)
            HudItem(stringResource(R.string.hud_delay), "${s.e2eLatencyMs} ms")
            HudItem(stringResource(R.string.hud_lost), "${s.frameDrops}")
            battery?.let { HudItem(stringResource(R.string.hud_computer), it) }
        }
        Text(
            text = basis(s, LocalResources.current),
            maxLines = 1,
            style = MaterialTheme.typography.labelSmall.copy(
                fontFamily = FontFamily.Monospace,
                fontSize = 10.sp,
                color = Color.White.copy(alpha = 0.55f),
            ),
        )
    }
}

/** The connection in a dot and a word, the word for those who cannot tell the colours apart. */
@Composable
private fun QualityDot(level: LinkQuality.Level, modifier: Modifier) {
    val (colour, word) = when (level) {
        LinkQuality.Level.MEASURING -> Color(0xFF9E9E9E) to R.string.link_measuring
        LinkQuality.Level.GOOD -> Color(0xFF4CAF50) to R.string.link_good
        LinkQuality.Level.FAIR -> Color(0xFFFFC107) to R.string.link_fair
        LinkQuality.Level.POOR -> Color(0xFFF44336) to R.string.link_poor
    }
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = modifier
            .clip(RoundedCornerShape(12.dp))
            .background(Color.Black.copy(alpha = 0.55f))
            .padding(horizontal = 10.dp, vertical = 5.dp),
    ) {
        Box(
            Modifier
                .size(10.dp)
                .background(colour, CircleShape),
        )
        Spacer(Modifier.width(6.dp))
        Text(stringResource(word), color = Color.White, style = MaterialTheme.typography.labelSmall)
    }
}

/**
 * What the numbers above were measured on. A rate is a division by some span of
 * received video and the link figure is a median of some number of polls;
 * quoting either without saying how much of a sample stood behind it is how a
 * HUD ends up presenting a guess as a reading.
 */
private fun basis(s: RustCore.StreamingStats, res: android.content.res.Resources): String {
    val rates = if (s.rateWindowMs == 0L) {
        res.getString(R.string.hud_no_video)
    } else {
        res.getString(R.string.hud_rates_over, "%.1f".format(s.rateWindowMs / 1000.0))
    }
    val rtt = if (s.rttSamples == 0) {
        res.getString(R.string.hud_rtt_unmeasured)
    } else {
        res.getString(R.string.hud_rtt_median, s.rttSamples)
    }
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
