package dev.pocketdesk.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.shortcuts.DeckWidget
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * The computer's deck (F1): the buttons its owner set up in Pocketdesk's
 * Settings there, in a grid; a tap does the button's action on the
 * computer and says how it went on the button for a moment.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DeckSheet(address: String, controlPort: Int, computer: String, onDismiss: () -> Unit) {
    val context = LocalContext.current
    val resources = LocalResources.current
    val haptics = rememberLlHaptics()
    val scope = rememberCoroutineScope()
    var buttons by remember { mutableStateOf<List<Deck.Button>?>(null) }
    var failure by remember { mutableStateOf<String?>(null) }
    // What a press said, shown on its button for a moment, by index.
    val said = remember { mutableStateMapOf<Int, String>() }
    val busy = remember { mutableStateMapOf<Int, Boolean>() }

    LaunchedEffect(address, controlPort) {
        withContext(Dispatchers.IO) { RustCore.listDeck(address, controlPort) }
            // G3: the home screen's deck shows what the sheet heard.
            .onSuccess { json -> runCatching { Deck.parse(json) }.onSuccess { DeckWidget.keep(context, address, json) } }
            .mapCatching { Deck.parse(it) }
            .onSuccess { buttons = it }
            .onFailure { failure = context.plainErrorText(it.message) }
    }

    Prefs.observedVersion()
    val columns = Deck.columns(Prefs.deckColumns(context))

    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 16.dp)
                .padding(bottom = 24.dp),
        ) {
            Text(stringResource(R.string.deck_title, computer), style = MaterialTheme.typography.titleMedium)
            Text(
                stringResource(R.string.deck_hint),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(bottom = 12.dp),
            )
            val shown = buttons
            when {
                failure != null -> Text(stringResource(R.string.deck_failed, failure.orEmpty()))
                shown == null -> Text(stringResource(R.string.deck_loading))
                shown.isEmpty() -> Text(stringResource(R.string.deck_none))
                else -> Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Deck.rows(shown, columns).forEach { row ->
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            row.forEach { button ->
                                val note = said[button.index]
                                FilledTonalButton(
                                    onClick = {
                                        if (busy[button.index] == true) return@FilledTonalButton
                                        haptics.tap()
                                        busy[button.index] = true
                                        scope.launch {
                                            val answer = withContext(Dispatchers.IO) {
                                                RustCore.pressDeck(address, controlPort, button.index, button.label)
                                            }
                                            val outcome = answer.mapCatching { Deck.outcome(it) }
                                            val ok = outcome.getOrNull()?.ok == true
                                            val text = outcome.fold(
                                                onSuccess = {
                                                    if (it.waiting) {
                                                        resources.getString(R.string.deck_waiting)
                                                    } else if (it.ok) {
                                                        resources.getString(R.string.deck_done)
                                                    } else {
                                                        context.plainErrorText(it.reason)
                                                    }
                                                },
                                                onFailure = { context.plainErrorText(it.message) },
                                            )
                                            said[button.index] = text
                                            busy[button.index] = false
                                            // A refusal stays long enough to read.
                                            delay(if (ok) 1_500 else 4_000)
                                            if (said[button.index] == text) said.remove(button.index)
                                        }
                                    },
                                    modifier = Modifier
                                        .weight(1f)
                                        .heightIn(min = 72.dp)
                                        .semantics { contentDescription = button.label },
                                ) {
                                    Column(horizontalAlignment = Alignment.CenterHorizontally) {
                                        LlIcon(kindIcon(button.kind), null, size = 20.dp)
                                        Text(
                                            note ?: button.label,
                                            textAlign = TextAlign.Center,
                                            maxLines = 3,
                                            overflow = TextOverflow.Ellipsis,
                                            style = MaterialTheme.typography.labelLarge,
                                        )
                                    }
                                }
                            }
                            // The last row's gaps keep its buttons the size of the others.
                            repeat(columns - row.size) { Spacer(Modifier.weight(1f).size(0.dp)) }
                        }
                    }
                }
            }
        }
    }
}

/** Each kind's icon, as the widget draws it ([Deck.iconRes]). */
private fun kindIcon(kind: String): LlGlyph = LlGlyph(Deck.iconRes(kind))
