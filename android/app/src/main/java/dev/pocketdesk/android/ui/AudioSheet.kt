package dev.pocketdesk.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Slider
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.repeatOnLifecycle
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

/** One desktop output sink as reported by the audio-control plugin. */
data class AudioSink(
    val index: Int,
    val name: String,
    val description: String,
    val isDefault: Boolean,
)

private data class AudioSnapshot(
    val volume: Int,
    val muted: Boolean,
    val sinks: List<AudioSink>,
)

/** The `playing` object of a `media` answer (T8c), or null when nothing plays. */
private fun parsePlaying(body: JSONObject): NowPlaying? {
    val o = body.optJSONObject("playing") ?: return null
    return NowPlaying(
        title = o.optString("title"),
        artist = o.optString("artist"),
        app = o.optString("app"),
        isPlaying = o.optBoolean("isPlaying"),
        canPlayPause = o.optBoolean("canPlayPause"),
        canNext = o.optBoolean("canNext"),
        canPrevious = o.optBoolean("canPrevious"),
    )
}

/** How often the card reads what is playing while the sheet is open. */
private const val MEDIA_REFRESH_MS = 2_000L

private fun parseSinks(body: JSONObject): List<AudioSink> {
    val arr = body.optJSONArray("sinks") ?: JSONArray()
    return (0 until arr.length()).map { i ->
        val o = arr.getJSONObject(i)
        AudioSink(
            index = o.optInt("index"),
            name = o.optString("name"),
            description = o.optString("description"),
            isDefault = o.optBoolean("isDefault"),
        )
    }
}

/**
 * Desktop audio control (Tier-3 #16): volume + mute for the default output
 * and routing between sinks, over the KDE Connect control channel. The
 * server drives wpctl/pactl; this sheet is a thin remote panel.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AudioControlSheet(
    address: String,
    controlPort: Int,
    onDismiss: () -> Unit,
) {
    var snapshot by remember { mutableStateOf<AudioSnapshot?>(null) }
    // Errors split in two: server-supplied text (not localizable) vs a local
    // fallback resource, resolved at render time so it follows configuration.
    var errorText by remember { mutableStateOf<String?>(null) }
    var errorRes by remember { mutableStateOf<Int?>(null) }
    var slider by remember { mutableFloatStateOf(0f) }
    val scope = rememberCoroutineScope()
    // T8c: what the computer is playing. Its own errors (no player tool,
    // a refused button) stay in its card, not the sheet's.
    var mediaRead by remember { mutableStateOf(false) }
    var playing by remember { mutableStateOf<NowPlaying?>(null) }
    var mediaError by remember { mutableStateOf<String?>(null) }

    suspend fun media(body: JSONObject): JSONObject? {
        val result = withContext(Dispatchers.IO) {
            RustCore.audioControl(address, controlPort, body.toString())
        }
        val answer = result.getOrNull()?.let { runCatching { JSONObject(it) }.getOrNull() }
        return if (answer != null && answer.optBoolean("ok", true)) {
            mediaError = null
            answer
        } else {
            mediaError = answer?.optString("error")?.ifBlank { null }
                ?: result.exceptionOrNull()?.message
                ?: ""
            null
        }
    }

    suspend fun refreshMedia() {
        val answer = media(JSONObject().put("action", "media"))
        // A read that failed shows only why: the last track's buttons would
        // fail the same way.
        playing = answer?.let { parsePlaying(it) }
        mediaRead = answer != null
    }

    fun mediaCommand(command: String) {
        scope.launch {
            media(JSONObject().put("action", "mediaControl").put("command", command))
            refreshMedia()
        }
    }

    fun clearError() {
        errorText = null
        errorRes = null
    }

    suspend fun call(body: JSONObject): JSONObject? {
        val result =
            withContext(Dispatchers.IO) {
                RustCore.audioControl(address, controlPort, body.toString())
            }
        return result.fold(
            onSuccess = { json ->
                val obj = JSONObject(json)
                if (obj.optBoolean("ok", true)) {
                    obj
                } else {
                    val srv = obj.optString("error", "")
                    errorText = srv.ifBlank { null }
                    errorRes = if (srv.isBlank()) R.string.audio_rejected else null
                    null
                }
            },
            onFailure = { e ->
                val msg = e.message
                errorText = msg?.ifBlank { null }
                errorRes = if (msg.isNullOrBlank()) R.string.audio_failed else null
                null
            },
        )
    }

    suspend fun refreshAll() {
        val status = call(JSONObject().put("action", "status"))
        val sinks = call(JSONObject().put("action", "sinks"))
        if (status != null && sinks != null) {
            val v = status.optInt("volume", 0)
            snapshot = AudioSnapshot(v, status.optBoolean("muted"), parseSinks(sinks))
            slider = v.toFloat()
        }
    }

    LaunchedEffect(address, controlPort) {
        clearError()
        refreshAll()
    }
    // Only while the app is in front: an open sheet left behind Home would
    // otherwise keep asking the computer every 2 s.
    val lifecycleOwner = LocalLifecycleOwner.current
    LaunchedEffect(address, controlPort, lifecycleOwner) {
        lifecycleOwner.repeatOnLifecycle(Lifecycle.State.STARTED) {
            while (isActive) {
                refreshMedia()
                delay(MEDIA_REFRESH_MS)
            }
        }
    }

    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(modifier = Modifier.padding(horizontal = 16.dp)) {
            Text(stringResource(R.string.desktop_audio), style = MaterialTheme.typography.titleMedium)
            NowPlayingCard(
                read = mediaRead,
                playing = playing,
                error = mediaError,
                onCommand = ::mediaCommand,
            )
            val s = snapshot
            val error = errorText ?: errorRes?.let { stringResource(it) }
            when {
                s == null && error == null ->
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(vertical = 24.dp),
                        horizontalArrangement = Arrangement.Center,
                    ) {
                        CircularProgressIndicator()
                    }

                else -> {
                    if (error != null) {
                        Text(
                            error,
                            color = MaterialTheme.colorScheme.error,
                            style = MaterialTheme.typography.bodyMedium,
                            modifier = Modifier.padding(vertical = 8.dp),
                        )
                    }
                    if (s != null) {
                        Text(
                            stringResource(R.string.audio_volume, slider.toInt()) +
                                if (s.muted) "  ${stringResource(R.string.audio_muted)}" else "",
                            style = MaterialTheme.typography.bodyMedium,
                            modifier = Modifier.padding(top = 8.dp),
                        )
                        Slider(
                            value = slider,
                            onValueChange = { slider = it },
                            valueRange = 0f..100f,
                            onValueChangeFinished = {
                                val v = slider.toInt()
                                clearError()
                                scope.launch {
                                    if (
                                        call(
                                            JSONObject()
                                                .put("action", "setVolume")
                                                .put("volume", v),
                                        ) != null
                                    ) {
                                        refreshAll()
                                    }
                                }
                            },
                        )
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Text(stringResource(R.string.mute), style = MaterialTheme.typography.bodyMedium)
                            Switch(
                                checked = s.muted,
                                onCheckedChange = { m ->
                                    clearError()
                                    scope.launch {
                                        if (
                                            call(
                                                JSONObject()
                                                    .put("action", "setMuted")
                                                    .put("muted", m),
                                            ) != null
                                        ) {
                                            refreshAll()
                                        }
                                    }
                                },
                            )
                        }
                        HorizontalDivider(modifier = Modifier.padding(vertical = 12.dp))
                        Text(
                            stringResource(R.string.output_device),
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        LazyColumn(modifier = Modifier.padding(bottom = 24.dp)) {
                            items(s.sinks, key = { it.name }) { sink ->
                                ListItem(
                                    headlineContent = {
                                        Text(
                                            sink.description.ifBlank { sink.name },
                                            style = MaterialTheme.typography.bodyLarge,
                                            maxLines = 1,
                                            overflow = TextOverflow.Ellipsis,
                                            color = if (sink.isDefault) {
                                                MaterialTheme.colorScheme.primary
                                            } else {
                                                MaterialTheme.colorScheme.onSurface
                                            },
                                        )
                                    },
                                    supportingContent = {
                                        Text(
                                            sink.name,
                                            style = MaterialTheme.typography.bodySmall,
                                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                                            maxLines = 1,
                                            overflow = TextOverflow.Ellipsis,
                                        )
                                    },
                                    leadingContent = { LlIcon(LlIcons.Volume, null) },
                                    trailingContent = {
                                        RadioButton(
                                            selected = sink.isDefault,
                                            onClick = {
                                                clearError()
                                                scope.launch {
                                                    if (
                                                        call(
                                                            JSONObject()
                                                                .put("action", "selectSink")
                                                                .put("name", sink.name),
                                                        ) != null
                                                    ) {
                                                        refreshAll()
                                                    }
                                                }
                                            },
                                        )
                                    },
                                )
                                HorizontalDivider()
                            }
                        }
                    }
                }
            }
        }
    }
}

/**
 * "Now playing on the computer" (T8c): the title, who and where, and
 * Previous, Play/Pause and Next, each disabled when the player cannot take it.
 */
@Composable
private fun NowPlayingCard(
    read: Boolean,
    playing: NowPlaying?,
    error: String?,
    onCommand: (String) -> Unit,
) {
    Column(modifier = Modifier.padding(top = 8.dp)) {
        Text(
            stringResource(R.string.media_now_playing),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        when {
            playing != null -> {
                Text(
                    playing.headline(),
                    style = MaterialTheme.typography.bodyLarge,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.padding(top = 4.dp),
                )
                val subline = playing.subline()
                if (subline.isNotEmpty()) {
                    Text(
                        subline,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.Center,
                ) {
                    IconButton(onClick = { onCommand("previous") }, enabled = playing.canPrevious) {
                        LlIcon(LlIcons.SkipPrevious, stringResource(R.string.media_previous))
                    }
                    IconButton(onClick = { onCommand("playPause") }, enabled = playing.canPlayPause) {
                        if (playing.isPlaying) {
                            LlIcon(LlIcons.Pause, stringResource(R.string.media_pause))
                        } else {
                            LlIcon(LlIcons.Play, stringResource(R.string.media_play))
                        }
                    }
                    IconButton(onClick = { onCommand("next") }, enabled = playing.canNext) {
                        LlIcon(LlIcons.SkipNext, stringResource(R.string.media_next))
                    }
                }
            }
            read ->
                Text(
                    stringResource(R.string.media_nothing),
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.padding(vertical = 4.dp),
                )
        }
        if (!error.isNullOrBlank()) {
            Text(
                error,
                color = MaterialTheme.colorScheme.error,
                style = MaterialTheme.typography.bodySmall,
            )
        }
        HorizontalDivider(modifier = Modifier.padding(top = 8.dp))
    }
}
