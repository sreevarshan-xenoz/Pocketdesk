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
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.pair.DirectLinkDecision
import dev.pocketdesk.android.pair.DirectLinkNetwork
import dev.pocketdesk.android.pair.JoinOutcome
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
 *
 * A direct link (a computer's own Wi-Fi, made for this phone) joins that
 * network first, for Pocketdesk only, once "Pair" is tapped: the system asks
 * as well. The network stays joined through the session that follows and is
 * released by the caller when the session ends or the pairing is left.
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
    val context = androidx.compose.ui.platform.LocalContext.current.applicationContext
    val link = parsed.getOrNull()
    val decision = DirectLinkDecision.decide(
        link?.wifi,
        remember { Prefs.directLinks(context) },
        android.os.Build.VERSION.SDK_INT,
    )
    // Waiting for the system's prompt and the join of a direct link.
    var joining by remember(uri) { mutableStateOf(false) }
    var joinFailed by remember(uri) { mutableStateOf(false) }
    val blocked = decision == DirectLinkDecision.Disabled || decision == DirectLinkDecision.TooOld
    // Back cancels as Cancel does (U2: it left the app), and only when
    // Cancel shows: a pairing under way runs to its end, or the computer
    // could keep this phone paired while the phone saved nothing. Joining a
    // direct link is not yet a pairing: back leaves it, and the join with it.
    androidx.activity.compose.BackHandler {
        if (link == null || outcome != null || !confirmed || joining || joinFailed || blocked) onBack()
    }

    LaunchedEffect(uri, confirmed) {
        if (link == null || !confirmed || blocked) return@LaunchedEffect
        val wifi = link.wifi
        if (decision == DirectLinkDecision.Join && wifi != null) {
            joining = true
            val joined = DirectLinkNetwork.join(context, wifi)
            joining = false
            if (joined is JoinOutcome.Unavailable) {
                joinFailed = true
                return@LaunchedEffect
            }
        }
        val result = withContext(Dispatchers.IO) { runPairing(link, RustPairOps(context)) }
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
            decision == DirectLinkDecision.Disabled -> Text(
                stringResource(R.string.pair_link_direct_disabled),
                textAlign = TextAlign.Center,
            )
            decision == DirectLinkDecision.TooOld -> Text(
                stringResource(R.string.pair_link_direct_too_old),
                textAlign = TextAlign.Center,
            )
            joinFailed -> Text(
                stringResource(R.string.pair_link_join_failed),
                textAlign = TextAlign.Center,
            )
            joining -> {
                CircularProgressIndicator()
                Spacer(Modifier.height(16.dp))
                Text(
                    stringResource(R.string.pair_link_joining),
                    style = MaterialTheme.typography.titleMedium,
                    textAlign = TextAlign.Center,
                )
                Text(
                    stringResource(R.string.pair_link_joining_hint),
                    style = MaterialTheme.typography.bodySmall,
                    textAlign = TextAlign.Center,
                )
            }
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
                if (decision == DirectLinkDecision.Join) {
                    Spacer(Modifier.height(8.dp))
                    Text(stringResource(R.string.pair_link_direct_note), textAlign = TextAlign.Center)
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
        if (link == null || outcome != null || !confirmed || joining || joinFailed || blocked) {
            Spacer(Modifier.height(16.dp))
            TextButton(onClick = onBack) { Text(stringResource(R.string.cancel)) }
        }
    }
}
