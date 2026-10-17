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
            "no file descriptor" to R.string.err_generic,
            "without the key" to R.string.err_generic,
            "quic::ConnectionError::ApplicationClosed" to R.string.err_link_dropped,
        )
        for ((raw, expected) in cases) {
            assertEquals(raw, Plain.Res(expected), plainError(raw))
        }
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

    @Test
    fun nothingSaidIsTheGenericLine() {
        assertEquals(Plain.Res(R.string.err_generic), plainError(null))
        assertEquals(Plain.Res(R.string.err_generic), plainError("  "))
    }
}
