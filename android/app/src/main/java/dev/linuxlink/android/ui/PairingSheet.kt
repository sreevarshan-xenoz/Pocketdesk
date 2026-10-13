package dev.linuxlink.android.ui

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.scaleIn
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import dev.linuxlink.android.HostStore
import dev.linuxlink.android.Prefs
import dev.linuxlink.android.R
import dev.linuxlink.android.bridge.RustCore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

private const val PIN_WAIT_SECS = 2L

/**
 * PIN pairing sheet (R3 Tier-2 #11b). Two flows: ask the desktop to show a
 * PIN and read it back here, or type a PIN produced by `linux-link pair`.
 * Pairing persists the desktop's deviceId on the phone and marks this
 * connection trusted on the desktop.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PairingSheet(
    address: String,
    controlPort: Int,
    message: String?,
    onDismiss: () -> Unit,
    onPaired: (String) -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val keyboard = LocalSoftwareKeyboardController.current
    // Status kept as resource + arg (or raw server text) so the visible
    // string is resolved at render time and follows configuration changes.
    var statusRes by remember { mutableStateOf<Int?>(null) }
    var statusArg by remember { mutableStateOf<String?>(null) }
    var statusText by remember { mutableStateOf(message) }
    var waiting by remember { mutableStateOf(false) }
    var success by remember { mutableStateOf(false) }
    var pinEntry by remember { mutableStateOf("") }
    var revealed by remember { mutableStateOf(Prefs.revealPin(context)) }
    val pinFocus = remember { FocusRequester() }

    fun setStatus(res: Int, arg: String? = null) {
        statusRes = res
        statusArg = arg
        statusText = null
    }

    fun onPairedNow(serverId: String) {
        setStatus(R.string.paired)
        success = true
        HostStore.savePairedDesktop(context, address, serverId)
        onPaired(serverId)
        scope.launch {
            delay(900)
            onDismiss()
        }
    }

    LaunchedEffect(waiting) {
        while (waiting) {
            val serverId = withContext(Dispatchers.IO) { RustCore.checkPairResult(PIN_WAIT_SECS) }
            if (serverId != null) {
                waiting = false
                onPairedNow(serverId)
                break
            }
        }
    }

    fun submitPin() {
        if (!PinRules.canSubmit(pinEntry, success)) return
        // The PIN answers the desktop directly; stop waiting for a push.
        waiting = false
        keyboard?.hide()
        val pin = pinEntry
        scope.launch {
            val result =
                withContext(Dispatchers.IO) {
                    RustCore.pairWithPin(address, controlPort, pin)
                }
            val serverId = result.getOrNull()
            if (serverId != null) {
                onPairedNow(serverId)
            } else {
                val error = result.exceptionOrNull()?.message
                if (error != null) setStatus(R.string.pair_error, error) else setStatus(R.string.pair_wrong_pin)
            }
        }
    }

    // Desktop said "type the PIN it is showing" → put the cursor where the
    // thumb already is.
    LaunchedEffect(statusRes) {
        if (statusRes == R.string.pair_pin_ready) {
            delay(150)
            runCatching { pinFocus.requestFocus() }
        }
    }

    // Fully expanded and scrollable: on a phone held in landscape the
    // half-height sheet cut the PIN buttons off below the screen edge.
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 20.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    stringResource(R.string.pair_title, address),
                    style = MaterialTheme.typography.titleMedium,
                    modifier = Modifier.weight(1f),
                )
                AnimatedVisibility(
                    visible = success,
                    enter = fadeIn() + scaleIn(initialScale = 0.4f),
                ) {
                    LlIcon(LlIcons.Check, null, tint = MaterialTheme.colorScheme.primary, size = 32.dp)
                }
            }
            val status = statusRes?.let { stringResource(it, statusArg.orEmpty()) } ?: statusText.orEmpty()
            if (status.isNotEmpty()) {
                Text(status, style = MaterialTheme.typography.bodyMedium)
            }
            if (waiting) {
                LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
            }
            Button(
                enabled = !waiting && !success,
                onClick = {
                    scope.launch {
                        val result =
                            withContext(Dispatchers.IO) {
                                RustCore.requestPairPin(address, controlPort)
                            }
                        val error = result.exceptionOrNull()?.message
                        when (result.getOrNull()) {
                            "pinSent" -> {
                                waiting = true
                                setStatus(R.string.pair_read_pin)
                            }
                            "pinReady" ->
                                setStatus(R.string.pair_pin_ready)
                            else ->
                                if (error != null) setStatus(R.string.pair_error, error) else setStatus(R.string.pair_no_pin)
                        }
                    }
                },
            ) {
                Text(if (waiting) stringResource(R.string.pair_waiting) else stringResource(R.string.pair_show_pin))
            }
            OutlinedTextField(
                value = pinEntry,
                onValueChange = { pinEntry = it.filter(Char::isDigit).take(6) },
                label = { Text(stringResource(R.string.pair_pin_label)) },
                keyboardOptions =
                    KeyboardOptions(keyboardType = KeyboardType.Number, imeAction = ImeAction.Done),
                // The keyboard's Done key pairs, as the button does.
                keyboardActions = KeyboardActions(onDone = { submitPin() }),
                visualTransformation =
                    if (revealed) VisualTransformation.None else PasswordVisualTransformation(),
                trailingIcon = {
                    IconButton(onClick = { revealed = !revealed }) {
                        LlIcon(
                            LlIcons.Eye,
                            stringResource(R.string.pair_reveal_pin),
                        )
                    }
                },
                singleLine = true,
                modifier = Modifier
                    .fillMaxWidth()
                    .focusRequester(pinFocus),
            )
            OutlinedButton(
                enabled = PinRules.canSubmit(pinEntry, success),
                onClick = { submitPin() },
            ) {
                Text(stringResource(R.string.pair_enter_pin))
            }
            TextButton(
                onClick = {
                    waiting = false
                    onDismiss()
                },
            ) {
                Text(stringResource(R.string.cancel))
            }
        }
    }
}
