package dev.pocketdesk.android.ui

import androidx.annotation.StringRes
import dev.pocketdesk.android.R

/**
 * The bridge reports failures as raw Rust strings ("Connection failed: ...
 * Connection refused (os error 111)", certificate noise, server-declined
 * bodies). Users should never see those — map the known shapes to a
 * friendly message and log the raw text instead.
 */
@StringRes
fun humanizeError(raw: String): Int {
    val l = raw.lowercase()
    return when {
        "connection refused" in l -> R.string.err_host_unreachable
        "timed out" in l || "timeout" in l -> R.string.err_timeout
        "no cached wan identity" in l -> R.string.err_lan_only
        // The far side closed during the handshake: nothing there to judge.
        HANDSHAKE_CLOSED in l -> R.string.err_host_unreachable
        "certificate" in l || "untrusted" in l || "handshake" in l ->
            R.string.err_trust_mismatch
        "session refused" in l -> R.string.err_session_refused
        // The computer's permissions turned the session down: say which.
        "input not allowed" in l -> R.string.err_input_not_allowed
        "sound not allowed" in l -> R.string.err_sound_not_allowed
        "permissions changed" in l -> R.string.err_permissions_changed
        // The computer ended the session: it no longer pairs this phone.
        "not paired" in l -> R.string.err_unpaired
        // The computer ended it: its encoder gave up, or its own reasons;
        // not a dropped link (S2).
        "encoder failed" in l -> R.string.err_encoder_failed
        "session ended" in l -> R.string.err_session_ended
        "declined" in l || "pairing" in l || PIN.containsMatchIn(l) -> R.string.err_pairing_declined
        "not connected" in l || "disconnected" in l || "closed" in l || "reset" in l ->
            R.string.err_link_dropped
        else -> R.string.err_generic
    }
}

/** Does this failure call for the "pair again" escape hatch? */
fun errorSuggestsRepair(raw: String): Boolean {
    val l = raw.lowercase()
    if (HANDSHAKE_CLOSED in l) return false
    return "certificate" in l || "untrusted" in l || "handshake" in l ||
        "declined" in l || PIN.containsMatchIn(l) || "session refused" in l || "not paired" in l
}

/**
 * A handshake the far side closed (a tunnel with nothing behind it, a server
 * stopping): nothing answered, never a changed certificate. tokio-rustls's
 * words for it; a certificate it refuses reads "invalid peer certificate".
 */
private const val HANDSHAKE_CLOSED = "handshake eof"

/** "pin" as a word, not inside "ping", "mapping" or "stopping". */
private val PIN = Regex("""\bpin\b""")
