package dev.pocketdesk.android.ui

import android.app.KeyguardManager
import android.content.Context
import android.os.Build
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.ask.ApproveKey
import dev.pocketdesk.android.ask.ApproveRule
import dev.pocketdesk.android.bridge.RustCore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Approve with my phone: "Approve my computer's requests" (off) makes this
 * phone's Keystore key and gives its public key to the computer it works
 * with, where root pins it; "Fingerprint only, not the PIN" makes a new key
 * that takes only the fingerprint (so it is pinned again). The key's
 * fingerprint shows here to compare with what the computer printed.
 */
@Composable
fun ApproveSetting() {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var on by remember { mutableStateOf(Prefs.approveRequests(context) && ApproveKey.exists(context)) }
    var biometricOnly by remember { mutableStateOf(Prefs.approveBiometricOnly(context)) }
    var fingerprint by remember { mutableStateOf(ApproveKey.publicKey(ApproveKey.current(context))?.let(ApproveRule::fingerprint)) }
    // A pin's new key, until a request names it as pinned.
    val waiting = remember { ApproveKey.publicKey(ApproveKey.spare(context))?.let(ApproveRule::code) }
    var said by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }

    fun turn(want: Boolean, onlyBiometric: Boolean) {
        busy = true
        scope.launch {
            val result = withContext(Dispatchers.IO) { setUp(context, want, onlyBiometric) }
            busy = false
            said = result.problem
            on = want && result.problem == null
            if (result.problem == null) biometricOnly = onlyBiometric
            fingerprint = result.fingerprint
        }
    }

    val summary = when {
        !ApproveKey.supported -> stringResource(R.string.approve_needs_android_11)
        said != null -> said
        on && waiting != null -> stringResource(R.string.settings_approve_waiting, waiting)
        on && fingerprint != null -> stringResource(R.string.settings_approve_on, fingerprint!!)
        else -> stringResource(R.string.settings_approve_summary)
    }
    SettingSwitch(
        stringResource(R.string.settings_approve),
        summary,
        on,
        { turn(it, biometricOnly) },
        enabled = ApproveKey.supported && !busy,
        warning = said != null,
    )
    if (on) {
        SettingSwitch(
            stringResource(R.string.settings_approve_biometric_only),
            stringResource(R.string.settings_approve_biometric_only_summary),
            biometricOnly,
            { turn(true, it) },
            enabled = !busy,
            indent = true,
        )
    }
}

private class SetUp(val problem: String?, val fingerprint: String?)

/** Make (or drop) the key and tell the computer; a problem to show, or null. */
private fun setUp(context: Context, on: Boolean, biometricOnly: Boolean): SetUp {
    val host = HostStore.lastHost(context)
    if (!on) {
        ApproveKey.deleteAll(context)
        Prefs.setApproveRequests(context, false)
        host?.let {
            RustCore.sendPhonePacket(it.address, it.controlPort, ApproveRule.KEY_PACKET, ApproveRule.keyBody(null, false))
        }
        return SetUp(null, null)
    }
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
        return SetUp(context.getString(R.string.approve_needs_android_11), null)
    }
    if (host == null) return SetUp(context.getString(R.string.settings_approve_no_computer), null)
    if (!context.getSystemService(KeyguardManager::class.java).isDeviceSecure) {
        return SetUp(context.getString(R.string.ask_needs_lock), null)
    }
    // A new key in place of both: unpinned until root pins again.
    ApproveKey.deleteAll(context)
    val key = runCatching { ApproveKey.create(ApproveKey.current(context), biometricOnly) }.getOrElse {
        // Fingerprint only with no fingerprint enrolled, for one.
        return SetUp(context.getString(R.string.settings_approve_no_key_made), null)
    }
    val sent = RustCore.sendPhonePacket(
        host.address,
        host.controlPort,
        ApproveRule.KEY_PACKET,
        ApproveRule.keyBody(key, biometricOnly),
    )
    if (sent.isFailure) {
        ApproveKey.deleteAll(context)
        return SetUp(context.getString(R.string.ask_not_sent, context.plainErrorText(sent.exceptionOrNull()?.message)), null)
    }
    Prefs.setApproveRequests(context, true)
    Prefs.setApproveBiometricOnly(context, biometricOnly)
    return SetUp(null, ApproveRule.fingerprint(key))
}
