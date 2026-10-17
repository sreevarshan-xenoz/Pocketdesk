package dev.pocketdesk.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import dev.pocketdesk.android.R
import dev.pocketdesk.android.pair.PairLink
import dev.pocketdesk.android.pair.PairOutcome
import dev.pocketdesk.android.pair.RustPairOps
import dev.pocketdesk.android.pair.runPairing
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * Pairs with the desktop a scanned (or opened) pairing code describes, then
 * hands over to [onPaired].
 *
 * Nothing happens until the user confirms: a pocketdesk:// link can be
 * opened by any web page or app, so the screen first shows the addresses
 * that would be contacted (the name is the link's own claim, shown as such
 * and sanitized), warns when the code was not scanned here, and waits for
 * "Pair". Every address is then pinned to the code's certificate fingerprint
 * before it is contacted.
 */
@Composable
fun PairLinkScreen(
    uri: String,
    /** False when another app or a web page opened the link (not scanned here). */
    scanned: Boolean,
    onPaired: (link: PairLink, address: String, serverId: String) -> Unit,
    onBack: () -> Unit,
) {
    val parsed = remember(uri) { PairLink.parse(uri) }
    var outcome by remember(uri) { mutableStateOf<PairOutcome?>(null) }
    var confirmed by remember(uri) { mutableStateOf(false) }
    val link = parsed.getOrNull()

    LaunchedEffect(uri, confirmed) {
        if (link == null || !confirmed) return@LaunchedEffect
        val result = withContext(Dispatchers.IO) { runPairing(link, RustPairOps) }
        if (result is PairOutcome.Paired) {
            onPaired(link, result.address, result.serverId)
        } else {
            outcome = result
        }
    }

    Column(
        modifier = Modifier.fillMaxSize().padding(32.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        when {
            link == null -> Text(
                stringResource(R.string.pair_link_invalid),
                textAlign = TextAlign.Center,
            )
            !confirmed -> {
                Text(
                    stringResource(R.string.pair_link_confirm_title, link.addresses.joinToString(", ")),
                    style = MaterialTheme.typography.titleMedium,
                    textAlign = TextAlign.Center,
                )
                val name = PairLink.displayName(link.name)
                if (name.isNotEmpty()) {
                    Text(stringResource(R.string.pair_link_calls_itself, name), textAlign = TextAlign.Center)
                }
                if (!scanned) {
                    Spacer(Modifier.height(8.dp))
                    Text(
                        stringResource(R.string.pair_link_not_scanned),
                        color = MaterialTheme.colorScheme.error,
                        textAlign = TextAlign.Center,
                    )
                }
                Spacer(Modifier.height(8.dp))
                Text(stringResource(R.string.pair_link_confirm_body), textAlign = TextAlign.Center)
                Spacer(Modifier.height(16.dp))
                Button(onClick = { confirmed = true }) { Text(stringResource(R.string.pair_link_confirm)) }
            }
            outcome == null -> {
                CircularProgressIndicator()
                Spacer(Modifier.height(16.dp))
                Text(
                    stringResource(
                        R.string.pair_link_title,
                        PairLink.displayName(link.name).ifBlank { link.addresses.first() },
                    ),
                    style = MaterialTheme.typography.titleMedium,
                )
                Text(
                    stringResource(R.string.pair_link_trying, link.addresses.joinToString(", ")),
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            outcome == PairOutcome.Expired -> Text(
                stringResource(R.string.pair_link_expired),
                textAlign = TextAlign.Center,
            )
            else -> Text(
                stringResource(
                    R.string.pair_link_unreachable,
                    androidx.compose.ui.platform.LocalContext.current.plainErrorText(
                        (outcome as? PairOutcome.Unreachable)?.lastError,
                    ),
                ),
                textAlign = TextAlign.Center,
            )
        }
        if (link == null || outcome != null || !confirmed) {
            Spacer(Modifier.height(16.dp))
            TextButton(onClick = onBack) { Text(stringResource(R.string.exit)) }
        }
    }
}
