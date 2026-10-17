package dev.pocketdesk.android.ui

import android.content.Context
import android.util.Log
import androidx.annotation.StringRes
import dev.pocketdesk.android.R

/**
 * What a person sees for a failure (U2). The bridge's own text ("Connection
 * failed: … (os error 111)", TLS and QUIC noise, a timeout) becomes the
 * plain sentence [humanizeError] gives, and the raw text goes to logcat; a
 * sentence the computer or the app wrote for a person ("The computer refused
 * the folder: files over the internet are off on that computer", "wake is
 * off for this computer") is kept, with a capital, since it says more than
 * any generic line could. Only the marks below make text technical: the
 * app's and the computer's own sentences often start in lower case (U2
 * review: a rule that took that for Rust's text hid them).
 */
sealed interface Plain {
    /** A sentence meant for a person, shown as it is. */
    data class Sentence(val text: String) : Plain

    /** Technical text: this resource is shown in its place. */
    data class Res(@StringRes val id: Int) : Plain
}

/** Marks of text written for a developer, not a person. */
private val TECHNICAL = listOf(
    "os error", "error:", "io error", "errno", "::", "tls", "quic",
    "certificate", "handshake", "timed out", "timeout", "connection refused",
    "connection reset", "connection failed", "broken pipe", "no route to host",
    "unreachable", "panicked", "exception", "jni", "eof", "rustls", "alpn",
    "iroh", "failed to ", "not available after", "no active streaming session",
)

/** Whether `text` reads as a developer's message rather than a person's. */
fun looksTechnical(text: String): Boolean {
    val l = text.lowercase()
    return TECHNICAL.any { it in l }
}

/** What to show for the failure `raw` (null or blank: a generic line). */
fun plainError(raw: String?): Plain {
    val text = raw?.trim().orEmpty()
    if (text.isEmpty()) return Plain.Res(R.string.err_generic)
    return if (looksTechnical(text)) {
        Plain.Res(humanizeError(text))
    } else {
        Plain.Sentence(text.replaceFirstChar { it.uppercase() })
    }
}

/**
 * Why a link closed (the transport's own words, "session refused" among
 * them), as [humanizeError] says it; the raw reason goes to logcat.
 */
fun Context.closeReasonText(raw: String): String {
    Log.w("Pocketdesk", "link closed: $raw")
    return getString(humanizeError(raw))
}

/** [plainError] as text, the raw failure logged when it is not shown. */
fun Context.plainErrorText(raw: String?): String = when (val plain = plainError(raw)) {
    is Plain.Sentence -> plain.text
    is Plain.Res -> {
        Log.w("Pocketdesk", "shown as plain text: $raw")
        getString(plain.id)
    }
}
