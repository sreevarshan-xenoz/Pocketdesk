package dev.linuxlink.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
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
import dev.linuxlink.android.R
import dev.linuxlink.android.pair.PairLink
import dev.linuxlink.android.pair.PairOutcome
import dev.linuxlink.android.pair.RustPairOps
import dev.linuxlink.android.pair.runPairing
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * Pairs with the desktop a scanned (or opened) pairing code describes, then
 * hands over to [onPaired]. Every address is pinned to the code's
 * certificate fingerprint before the phone connects to it.
 */
@Composable
fun PairLinkScreen(
    uri: String,
    onPaired: (link: PairLink, address: String, serverId: String) -> Unit,
    onBack: () -> Unit,
) {
    val parsed = remember(uri) { PairLink.parse(uri) }
    var outcome by remember(uri) { mutableStateOf<PairOutcome?>(null) }
    val link = parsed.getOrNull()

    LaunchedEffect(uri) {
        if (link == null) return@LaunchedEffect
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
            outcome == null -> {
                CircularProgressIndicator()
                Spacer(Modifier.height(16.dp))
                Text(
                    stringResource(R.string.pair_link_title, link.name.ifBlank { link.addresses.first() }),
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
                    (outcome as? PairOutcome.Unreachable)?.lastError.orEmpty(),
                ),
                textAlign = TextAlign.Center,
            )
        }
        if (link == null || outcome != null) {
            Spacer(Modifier.height(16.dp))
            TextButton(onClick = onBack) { Text(stringResource(R.string.exit)) }
        }
    }
}
