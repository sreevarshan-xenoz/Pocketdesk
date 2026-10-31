package dev.pocketdesk.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.pair.IntroduceRule
import dev.pocketdesk.android.pair.PairOutcome
import dev.pocketdesk.android.pair.RustPairOps
import dev.pocketdesk.android.pair.pairedComputerIds
import dev.pocketdesk.android.pair.runPairing
import dev.pocketdesk.android.pair.savePaired
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject

/**
 * Ecosystem phase 4: "Also pair with <computer>?" for each of the owner's
 * other computers that [computer] (the one at [address]) is paired with
 * both ways and this phone is not. Nothing is added until the owner taps
 * Pair here; then the computer introduces this phone, and the phone pairs
 * with the other as from a scanned code. [auto]: offered by itself after
 * pairing, so it closes quietly when there is nothing to offer.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun IntroduceSheet(
    address: String,
    controlPort: Int,
    computer: String,
    auto: Boolean,
    onDismiss: () -> Unit,
) {
    val context = LocalContext.current.applicationContext
    // One sheet at a time (IntroduceRule.Gate): another already open keeps
    // this one from showing at all.
    val mine = remember(address) { IntroduceRule.gate.open(address) }
    DisposableEffect(address) {
        onDispose { if (mine) IntroduceRule.gate.closed(address) }
    }
    if (!mine) {
        LaunchedEffect(address) { onDismiss() }
        return
    }
    var offers by remember { mutableStateOf<List<IntroduceRule.Offer>?>(null) }
    var at by remember { mutableIntStateOf(0) }
    // A string resource: why there is nothing to offer.
    var failure by remember { mutableStateOf<Int?>(null) }
    var working by remember { mutableStateOf(false) }
    // What the last Pair came to: a resource and the computer's name.
    var said by remember { mutableStateOf<Pair<Int, String>?>(null) }
    val scope = rememberCoroutineScope()

    LaunchedEffect(address, controlPort) {
        val asked = IntroduceRule.newRequest()
        withContext(Dispatchers.IO) { RustCore.introductions(address, controlPort, asked) }
            .mapCatching { JSONObject(it) }
            .onSuccess { answer ->
                if (!IntroduceRule.isAnswerTo(asked, answer.optString("request"))) {
                    failure = R.string.introduce_no_answer
                } else if (answer.has("refused")) {
                    failure = IntroduceRule.refusal(answer.optString("refused"))
                } else {
                    val listed = answer.optJSONArray("computers")
                    val all = (0 until (listed?.length() ?: 0)).mapNotNull { i ->
                        listed?.optJSONObject(i)?.let {
                            IntroduceRule.Offer(it.optString("id"), it.optString("name"))
                        }
                    }
                    val known = withContext(Dispatchers.IO) { pairedComputerIds(context) }
                    offers = IntroduceRule.fresh(all, known)
                }
            }
            .onFailure {
                android.util.Log.i("IntroduceSheet", "No answer: ${it.message}")
                failure = R.string.introduce_no_answer
            }
        // Offered by itself: nothing to say, nothing shown.
        if (auto && (failure != null || offers.isNullOrEmpty())) onDismiss()
    }

    // Not now is heard: this computer offers nothing more by itself for a
    // while, and passing its last offer closes the sheet rather than
    // saying there is nothing to offer.
    fun notNow(list: List<IntroduceRule.Offer>) {
        IntroduceRule.gate.notNow(address, System.currentTimeMillis())
        if (at + 1 >= list.size && said == null) onDismiss() else at++
    }

    fun record(outcome: IntroduceRule.Outcome) {
        Prefs.setIntroduceLast(context, IntroduceRule.encode(outcome))
    }

    fun pair(offer: IntroduceRule.Offer) {
        val name = IntroduceRule.shownName(offer)
        working = true
        scope.launch {
            val asked = IntroduceRule.newRequest()
            val answer = withContext(Dispatchers.IO) {
                RustCore.introduce(address, controlPort, offer.id, asked)
            }.mapCatching { JSONObject(it) }.getOrNull()
                ?.takeIf { IntroduceRule.isAnswerTo(asked, it.optString("request")) }
            val link = answer?.let {
                IntroduceRule.takeLink(asked, it.optString("request"), offer.id, it.optString("link"))
            }
            said = when {
                answer == null -> {
                    record(IntroduceRule.Outcome(IntroduceRule.Kind.UNREACHABLE, name))
                    R.string.introduce_no_answer to name
                }
                answer.has("refused") -> {
                    val code = answer.optString("refused")
                    record(IntroduceRule.Outcome(IntroduceRule.Kind.REFUSED, name, code))
                    IntroduceRule.refusal(code) to name
                }
                link == null -> {
                    record(IntroduceRule.Outcome(IntroduceRule.Kind.REFUSED, name, "failed"))
                    R.string.introduce_refused_failed to name
                }
                else -> when (val result = withContext(Dispatchers.IO) { runPairing(link, RustPairOps(context)) }) {
                    is PairOutcome.Paired -> {
                        withContext(Dispatchers.IO) { savePaired(context, link, result.address, result.serverId) }
                        record(IntroduceRule.Outcome(IntroduceRule.Kind.PAIRED, name))
                        R.string.introduce_paired to name
                    }
                    PairOutcome.Expired -> {
                        record(IntroduceRule.Outcome(IntroduceRule.Kind.EXPIRED, name))
                        R.string.introduce_expired to name
                    }
                    is PairOutcome.Unreachable -> {
                        record(IntroduceRule.Outcome(IntroduceRule.Kind.UNREACHABLE, name))
                        R.string.introduce_unreachable to name
                    }
                }
            }
            working = false
            at++
        }
    }

    ModalBottomSheet(
        onDismissRequest = {
            if (!working) {
                // Closed with an offer on it: as Not now.
                if (offers?.getOrNull(at) != null) IntroduceRule.gate.notNow(address, System.currentTimeMillis())
                onDismiss()
            }
        },
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 16.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            said?.let { (words, name) ->
                Text(stringResource(words, name), style = MaterialTheme.typography.bodyMedium)
            }
            val list = offers
            val offer = list?.getOrNull(at)
            when {
                failure != null -> Text(stringResource(failure!!))
                list == null -> Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    CircularProgressIndicator()
                    Text(stringResource(R.string.introduce_asking))
                }
                working -> Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    CircularProgressIndicator()
                    Text(stringResource(R.string.introduce_pairing))
                }
                offer != null -> {
                    val name = IntroduceRule.shownName(offer)
                    Text(
                        stringResource(R.string.introduce_title, name),
                        style = MaterialTheme.typography.titleMedium,
                    )
                    Text(stringResource(R.string.introduce_body, computer, name))
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Button(onClick = { pair(offer) }) { Text(stringResource(R.string.introduce_pair)) }
                        TextButton(onClick = { notNow(list.orEmpty()) }) { Text(stringResource(R.string.introduce_not_now)) }
                    }
                }
                said == null -> Text(stringResource(R.string.introduce_none, computer))
                else -> {}
            }
            if (!working && (offer == null || failure != null)) {
                TextButton(onClick = onDismiss) { Text(stringResource(R.string.introduce_close)) }
            }
        }
    }
}
