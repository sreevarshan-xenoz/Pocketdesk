package dev.pocketdesk.android.ui

import android.graphics.BitmapFactory
import android.text.format.DateFormat
import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import java.io.File
import java.util.Date
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * One still of the computer's screen (BF2), with no session: taken when the
 * sheet opens and again at each tap, never on its own. The still is read
 * from a file of its own in the app's cache and the file deleted at once,
 * so no picture of the computer's screen stays on the phone; one left by an
 * app stopped mid-peek goes the next time the sheet opens.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PeekSheet(address: String, controlPort: Int, computer: String, onDismiss: () -> Unit) {
    val context = LocalContext.current
    var still by remember { mutableStateOf<ImageBitmap?>(null) }
    var seenAt by remember { mutableStateOf<Date?>(null) }
    var failure by remember { mutableStateOf<String?>(null) }
    var looking by remember { mutableStateOf(true) }
    var asks by remember { mutableIntStateOf(0) }

    LaunchedEffect(address, controlPort, asks) {
        looking = true
        val got = withContext(Dispatchers.IO) {
            if (asks == 0) Peek.leftovers(context.cacheDir.listFiles().orEmpty().toList()).forEach { it.delete() }
            // Its own file (BF2 review): a peek still running from a sheet
            // closed and opened again must not delete this one's.
            val file = File.createTempFile(Peek.FILE_PREFIX, ".jpg", context.cacheDir)
            try {
                RustCore.fetchPeek(address, controlPort, file.path).mapCatching { decode(file) }
            } finally {
                file.delete()
            }
        }
        got.onSuccess {
            still = it.asImageBitmap()
            seenAt = Date()
            failure = null
        }.onFailure {
            failure = context.peekFailure(it.message)
        }
        looking = false
    }

    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 16.dp)
                .padding(bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(stringResource(R.string.peek_title, computer), style = MaterialTheme.typography.titleMedium)
            if (looking) {
                Text(stringResource(R.string.peek_looking), style = MaterialTheme.typography.bodySmall)
                LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
            }
            failure?.let {
                Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium)
            }
            val again = stringResource(R.string.peek_again)
            still?.let { shown ->
                Image(
                    bitmap = shown,
                    contentDescription = stringResource(R.string.peek_image, computer),
                    contentScale = ContentScale.FillWidth,
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable(enabled = !looking, onClickLabel = again) { asks++ },
                )
                seenAt?.let { at ->
                    Text(
                        stringResource(R.string.peek_seen_at, DateFormat.getTimeFormat(context).format(at)),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            OutlinedButton(onClick = { asks++ }, enabled = !looking) { Text(again) }
        }
    }
}

/** The still in [file], refused when it is larger than a computer sends. */
private fun decode(file: File): android.graphics.Bitmap {
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeFile(file.path, bounds)
    check(Peek.fits(bounds.outWidth, bounds.outHeight)) { "not a picture of a screen" }
    return BitmapFactory.decodeFile(file.path) ?: error("not a picture")
}

/** Why a peek brought no still, in words. */
private fun android.content.Context.peekFailure(message: String?): String = when (Peek.refusal(message)) {
    Peek.Refusal.Off -> getString(R.string.peek_off)
    Peek.Refusal.Paused -> getString(R.string.peek_paused)
    Peek.Refusal.Busy -> getString(R.string.peek_busy)
    Peek.Refusal.TooSoon -> getString(R.string.peek_too_soon)
    Peek.Refusal.NoScreen -> getString(R.string.peek_no_screen)
    Peek.Refusal.NotAllowed -> getString(R.string.peek_not_allowed)
    Peek.Refusal.Other -> getString(R.string.peek_failed, plainErrorText(message))
}
