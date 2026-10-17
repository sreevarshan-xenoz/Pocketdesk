package dev.pocketdesk.android.ui

import dev.pocketdesk.android.R
import org.junit.Assert.assertEquals
import org.junit.Test

class PlainErrorTest {
    @Test
    fun theBridgesOwnTextBecomesAPlainSentence() {
        val cases = mapOf(
            "Connection failed: Connection refused (os error 111)" to R.string.err_host_unreachable,
            "connect timed out after 10s" to R.string.err_timeout,
            "TLS handshake failed: invalid peer certificate" to R.string.err_trust_mismatch,
            "quic::ConnectionError::ApplicationClosed" to R.string.err_link_dropped,
            "Failed to create endpoint: bind" to R.string.err_generic,
            "Connection not available after connect" to R.string.err_generic,
        )
        for ((raw, expected) in cases) {
            assertEquals(raw, Plain.Res(expected), plainError(raw))
        }
    }

    /** A session the computer's permissions turned down says which switch (N5). */
    @Test
    fun aRefusalForWantOfAPermissionNamesIt() {
        assertEquals(
            R.string.err_input_not_allowed,
            humanizeError("stream closed by the desktop: input not allowed"),
        )
        assertEquals(
            R.string.err_sound_not_allowed,
            humanizeError("stream closed by the desktop: sound not allowed"),
        )
        // The owner narrowed what the phone may do during the session.
        assertEquals(R.string.err_permissions_changed, humanizeError("permissions changed"))
    }

    @Test
    fun aSentenceForAPersonIsKept() {
        for (said in listOf(
            "The computer refused the folder: files over the internet are off on that computer",
            "Allow or deny it in the Pocketdesk app.",
            "This file is hidden.",
        )) {
            assertEquals(Plain.Sentence(said), plainError(said))
        }
    }

    /** The app's and the computer's own lowercase sentences (U2 review). */
    @Test
    fun aLowercaseSentenceForAPersonIsKeptWithACapital() {
        val cases = mapOf(
            "wake is off for this computer" to "Wake is off for this computer",
            "another phone holds privacy mode" to "Another phone holds privacy mode",
            "this phone already trusts a different computer at 192.0.2.1; remove it first" to
                "This phone already trusts a different computer at 192.0.2.1; remove it first",
            "screen lock is not supported on this OS yet" to "Screen lock is not supported on this OS yet",
        )
        for ((said, shown) in cases) {
            assertEquals(Plain.Sentence(shown), plainError(said))
        }
    }

    @Test
    fun nothingSaidIsTheGenericLine() {
        assertEquals(Plain.Res(R.string.err_generic), plainError(null))
        assertEquals(Plain.Res(R.string.err_generic), plainError("  "))
    }
}
