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
        "certificate" in l || "untrusted" in l || "handshake" in l ->
            R.string.err_trust_mismatch
        "session refused" in l -> R.string.err_session_refused
        "declined" in l || "pairing" in l || "pin" in l -> R.string.err_pairing_declined
        "not connected" in l || "disconnected" in l || "closed" in l || "reset" in l ->
            R.string.err_link_dropped
        else -> R.string.err_generic
    }
}

/** Does this failure call for the "pair again" escape hatch? */
fun errorSuggestsRepair(raw: String): Boolean {
    val l = raw.lowercase()
    return "certificate" in l || "untrusted" in l || "handshake" in l ||
        "declined" in l || "pin" in l || "session refused" in l
}
