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
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import dev.pocketdesk.android.HostStore
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
 */
class AskActivity : ComponentActivity() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private var said by mutableStateOf<String?>(null)
    private var sending by mutableStateOf(false)

    private lateinit var host: String
    private var port = 0
    private lateinit var id: String
    private lateinit var question: String
    private var expiresUnix = 0L

    /** The screen lock's own screen, below Android 11. */
    private val confirm = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) {
        if (it.resultCode == RESULT_OK) send(yes = true) else said = getString(R.string.ask_not_confirmed)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        host = intent.getStringExtra(Asks.EXTRA_HOST) ?: return finish()
        port = intent.getIntExtra(Asks.EXTRA_PORT, 0)
        id = intent.getStringExtra(Asks.EXTRA_ID) ?: return finish()
        question = intent.getStringExtra(Asks.EXTRA_QUESTION) ?: return finish()
        expiresUnix = intent.getLongExtra(Asks.EXTRA_EXPIRES, 0L)
        val computer = HostStore.shownName(this, host)
        val until = DateFormat.getTimeFormat(this).format(Date(expiresUnix * 1000))
        setContent {
            PocketdeskTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    Column(
                        modifier = Modifier.padding(24.dp),
                        verticalArrangement = Arrangement.spacedBy(16.dp),
                    ) {
                        Text(stringResource(R.string.ask_from_computer, computer), style = MaterialTheme.typography.titleMedium)
                        Text(question, style = MaterialTheme.typography.headlineSmall)
                        Text(
                            stringResource(R.string.ask_waits_until, until),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        said?.let { Text(it, color = MaterialTheme.colorScheme.error) }
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
        scope.cancel()
        super.onDestroy()
    }

    private fun answer(yes: Boolean) {
        if (System.currentTimeMillis() / 1000 >= expiresUnix) {
            said = getString(R.string.ask_expired)
            return
        }
        if (!yes) return send(yes = false)
        val keyguard = getSystemService(KeyguardManager::class.java)
        if (!AskRule.mayAnswerYes(keyguard.isDeviceSecure)) {
            said = getString(R.string.ask_needs_lock)
            Log.i(TAG, "Yes was not sent: this phone has no screen lock")
            return
        }
        val title = getString(R.string.ask_confirm_title)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            BiometricPrompt.Builder(this)
                .setTitle(title)
                .setDescription(question)
                .setAllowedAuthenticators(
                    BiometricManager.Authenticators.BIOMETRIC_STRONG or BiometricManager.Authenticators.DEVICE_CREDENTIAL,
                )
                .build()
                .authenticate(
                    CancellationSignal(),
                    mainExecutor,
                    object : BiometricPrompt.AuthenticationCallback() {
                        override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                            send(yes = true)
                        }

                        override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
                            said = getString(R.string.ask_not_confirmed)
                        }
                    },
                )
        } else {
            @Suppress("DEPRECATION")
            val unlock = keyguard.createConfirmDeviceCredentialIntent(title, question)
            if (unlock == null) said = getString(R.string.ask_needs_lock) else confirm.launch(unlock)
        }
    }

    private fun send(yes: Boolean) {
        val body = AskRule.body(id, yes) ?: return finish()
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
