package dev.pocketdesk.android.ask

import android.app.KeyguardManager
import android.hardware.biometrics.BiometricManager
import android.hardware.biometrics.BiometricPrompt
import android.os.Build
import android.os.Bundle
import android.os.CancellationSignal
import android.text.format.DateFormat
import android.util.Log
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.ui.plainErrorText
import dev.pocketdesk.android.ui.theme.PocketdeskTheme
import java.util.Date
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * A question the computer put to this phone (BF3), with Yes and No. No is
 * sent at a tap; Yes first takes the phone's fingerprint or screen lock
 * (a phone with none cannot say Yes, and is told so).
 *
 * An approval request (approve with my phone: sudo, polkit, the lock
 * screen) shows what it is for, and its Yes is a signature over the request
 * by this phone's Keystore key, which the fingerprint (or the screen lock,
 * unless "Fingerprint only" is on) unlocks for that one signature.
 */
class AskActivity : ComponentActivity() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private var said by mutableStateOf<String?>(null)
    // Asking for the lock or sending: the buttons wait (BF3 review: a
    // double tap on Yes opened two prompts).
    private var sending by mutableStateOf(false)
    private var prompt: CancellationSignal? = null

    private lateinit var host: String
    private var port = 0
    private lateinit var id: String
    private lateinit var question: String
    private var expiresUnix = 0L
    private var approval: ApproveRule.Approval? = null

    /** The screen lock's own screen, below Android 11. */
    private val confirm = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) {
        if (it.resultCode == RESULT_OK) {
            send(yes = true)
        } else {
            sending = false
            said = getString(R.string.ask_not_confirmed)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        host = intent.getStringExtra(Asks.EXTRA_HOST) ?: return finish()
        port = intent.getIntExtra(Asks.EXTRA_PORT, 0)
        id = intent.getStringExtra(Asks.EXTRA_ID) ?: return finish()
        question = intent.getStringExtra(Asks.EXTRA_QUESTION) ?: return finish()
        expiresUnix = intent.getLongExtra(Asks.EXTRA_EXPIRES, 0L)
        approval = ApproveRule.parse(intent.getStringExtra(Asks.EXTRA_APPROVE))
        if (intent.hasExtra(Asks.EXTRA_APPROVE) && approval == null) return finish()
        val asked = approval
        val computer = HostStore.shownName(this, host)
        val until = DateFormat.getTimeFormat(this).format(Date(expiresUnix * 1000))
        setContent {
            PocketdeskTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    Column(
                        // Scrolls: a long command is shown whole.
                        modifier = Modifier.verticalScroll(rememberScrollState()).padding(24.dp),
                        verticalArrangement = Arrangement.spacedBy(16.dp),
                    ) {
                        Text(stringResource(R.string.ask_from_computer, computer), style = MaterialTheme.typography.titleMedium)
                        if (asked == null) {
                            Text(question, style = MaterialTheme.typography.headlineSmall)
                        } else {
                            // What the signature covers, as the computer
                            // sent it: never the free words of the question.
                            Text(
                                when {
                                    asked.pin -> stringResource(R.string.approve_pin, computer, asked.user)
                                    asked.unlock -> stringResource(R.string.approve_unlock, asked.user)
                                    else -> stringResource(R.string.approve_admin, asked.service, asked.user)
                                },
                                style = MaterialTheme.typography.headlineSmall,
                            )
                            // Every field is shown whole, in the computer's
                            // printable form (hidden characters as ⟨U+XXXX⟩).
                            @Composable
                            fun line(label: Int, value: String) {
                                if (value.isEmpty()) return
                                Text(stringResource(label), style = MaterialTheme.typography.labelMedium)
                                Text(value, fontFamily = FontFamily.Monospace)
                            }
                            line(R.string.approve_command, asked.command)
                            if (asked.requester.isNotEmpty() && asked.requester != asked.user) {
                                line(R.string.approve_requester, asked.requester)
                            }
                            line(R.string.approve_action, asked.action)
                            line(R.string.approve_tty, asked.tty)
                            line(R.string.approve_origin, asked.origin)
                            Text(
                                stringResource(if (asked.pin) R.string.approve_pin_signs else R.string.approve_signs),
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                        Text(
                            stringResource(R.string.ask_waits_until, until),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        said?.let {
                            Text(
                                it,
                                color = MaterialTheme.colorScheme.error,
                                modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite },
                            )
                        }
                        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                            OutlinedButton(onClick = { answer(yes = false) }, enabled = !sending) {
                                Text(stringResource(R.string.ask_no))
                            }
                            Button(onClick = { answer(yes = true) }, enabled = !sending) {
                                Text(stringResource(R.string.ask_yes))
                            }
                        }
                    }
                }
            }
        }
    }

    override fun onDestroy() {
        prompt?.cancel()
        scope.cancel()
        super.onDestroy()
    }

    private fun expired(): Boolean = System.currentTimeMillis() / 1000 >= expiresUnix

    private fun answer(yes: Boolean) {
        if (sending) return
        if (expired()) {
            said = getString(R.string.ask_expired)
            return
        }
        if (!yes) return send(yes = false)
        approval?.let { return approve(it) }
        val keyguard = getSystemService(KeyguardManager::class.java)
        if (!AskRule.mayAnswerYes(keyguard.isDeviceSecure)) {
            said = getString(R.string.ask_needs_lock)
            Log.i(TAG, "Yes was not sent: this phone has no screen lock")
            return
        }
        val title = getString(R.string.ask_confirm_title)
        sending = true
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            val signal = CancellationSignal().also { prompt = it }
            BiometricPrompt.Builder(this)
                .setTitle(title)
                .setDescription(question)
                .setAllowedAuthenticators(
                    BiometricManager.Authenticators.BIOMETRIC_STRONG or BiometricManager.Authenticators.DEVICE_CREDENTIAL,
                )
                .build()
                .authenticate(
                    signal,
                    mainExecutor,
                    object : BiometricPrompt.AuthenticationCallback() {
                        override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                            send(yes = true)
                        }

                        override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
                            sending = false
                            said = getString(R.string.ask_not_confirmed)
                        }
                    },
                )
        } else {
            @Suppress("DEPRECATION")
            val unlock = keyguard.createConfirmDeviceCredentialIntent(title, question)
            if (unlock == null) {
                sending = false
                said = getString(R.string.ask_needs_lock)
            } else {
                confirm.launch(unlock)
            }
        }
    }

    /** Yes to an approval: the fingerprint (or lock) unlocks one signature over it. */
    private fun approve(asked: ApproveRule.Approval) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            said = getString(R.string.approve_needs_android_11)
            return
        }
        if (asked.pin) return pinKey(asked)
        val signature = ApproveKey.signature().getOrElse {
            val problem = (it as? ApproveKey.ProblemException)?.problem
            said = getString(
                if (problem == ApproveKey.Problem.Invalidated) R.string.approve_invalidated else R.string.approve_no_key,
            )
            Log.i(TAG, "an approval was not signed: ${problem ?: it.javaClass.simpleName}")
            return
        }
        val biometricOnly = Prefs.approveBiometricOnly(this)
        sending = true
        val signal = CancellationSignal().also { prompt = it }
        val builder = BiometricPrompt.Builder(this)
            .setTitle(getString(R.string.ask_confirm_title))
            .setDescription(asked.command.ifEmpty { question })
        if (biometricOnly) {
            builder.setAllowedAuthenticators(BiometricManager.Authenticators.BIOMETRIC_STRONG)
                .setNegativeButton(getString(R.string.ask_no), mainExecutor) { _, _ ->
                    sending = false
                    said = getString(R.string.ask_not_confirmed)
                }
        } else {
            builder.setAllowedAuthenticators(
                BiometricManager.Authenticators.BIOMETRIC_STRONG or BiometricManager.Authenticators.DEVICE_CREDENTIAL,
            )
        }
        builder.build().authenticate(
            BiometricPrompt.CryptoObject(signature),
            signal,
            mainExecutor,
            object : BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                    val signed = runCatching {
                        val s = result.cryptoObject?.signature ?: error("no signature")
                        s.update(ApproveRule.message(asked))
                        s.sign()
                    }.getOrElse {
                        sending = false
                        said = getString(R.string.approve_no_key)
                        Log.i(TAG, "an approval was not signed: ${it.javaClass.simpleName}")
                        return
                    }
                    sendBody(ApproveRule.signedAnswer(asked.id, signed) ?: return finish(), yes = true)
                }

                override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
                    sending = false
                    said = getString(R.string.ask_not_confirmed)
                }
            },
        )
    }

    /**
     * Root pins this phone: after the fingerprint (or lock), make the key
     * with the pin's challenge and answer with its attestation chain, which
     * root checks up to Google's roots.
     */
    @androidx.annotation.RequiresApi(Build.VERSION_CODES.R)
    private fun pinKey(asked: ApproveRule.Approval) {
        val biometricOnly = Prefs.approveBiometricOnly(this)
        sending = true
        val signal = CancellationSignal().also { prompt = it }
        val builder = BiometricPrompt.Builder(this)
            .setTitle(getString(R.string.ask_confirm_title))
            .setDescription(getString(R.string.approve_pin, HostStore.shownName(this, host), asked.user))
        if (biometricOnly) {
            builder.setAllowedAuthenticators(BiometricManager.Authenticators.BIOMETRIC_STRONG)
                .setNegativeButton(getString(R.string.ask_no), mainExecutor) { _, _ ->
                    sending = false
                    said = getString(R.string.ask_not_confirmed)
                }
        } else {
            builder.setAllowedAuthenticators(
                BiometricManager.Authenticators.BIOMETRIC_STRONG or BiometricManager.Authenticators.DEVICE_CREDENTIAL,
            )
        }
        builder.build().authenticate(
            signal,
            mainExecutor,
            object : BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                    scope.launch {
                        val body = withContext(Dispatchers.Default) {
                            runCatching {
                                val key = ApproveKey.create(biometricOnly, ApproveRule.pinChallenge(asked))
                                // Settings lists the new key too.
                                RustCore.sendPhonePacket(host, port, ApproveRule.KEY_PACKET, ApproveRule.keyBody(key, biometricOnly))
                                ApproveRule.pinAnswer(asked.id, ApproveKey.chain())
                            }.getOrNull()
                        }
                        if (body == null) {
                            sending = false
                            said = getString(R.string.settings_approve_no_key_made)
                            return@launch
                        }
                        sendBody(body, yes = true)
                    }
                }

                override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
                    sending = false
                    said = getString(R.string.ask_not_confirmed)
                }
            },
        )
    }

    private fun send(yes: Boolean) {
        val body = AskRule.body(id, yes) ?: return finish()
        sendBody(body, yes)
    }

    private fun sendBody(body: String, yes: Boolean) {
        // A prompt can outlive the question (BF3 review).
        if (expired()) {
            sending = false
            said = getString(R.string.ask_expired)
            return
        }
        sending = true
        scope.launch {
            val sent = withContext(Dispatchers.IO) { RustCore.sendPhonePacket(host, port, AskRule.ANSWER, body) }
            sending = false
            sent.onSuccess {
                Asks.cancel(this@AskActivity, id)
                // For the device checks; never the question.
                Log.i(TAG, "answered ${if (yes) "yes" else "no"}")
                Toast.makeText(this@AskActivity, getString(R.string.ask_sent), Toast.LENGTH_SHORT).show()
                finish()
            }.onFailure {
                said = getString(R.string.ask_not_sent, plainErrorText(it.message))
            }
        }
    }

    private companion object {
        const val TAG = "Asks"
    }
}
