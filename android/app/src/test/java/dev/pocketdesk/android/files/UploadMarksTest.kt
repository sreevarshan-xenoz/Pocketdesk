package dev.pocketdesk.android.files

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class UploadMarksTest {
    /**
     * BB4: a file that failed, or never ran, goes again, up to three sends;
     * one sent or cancelled does not, and one still waiting is waited for.
     */
    @Test
    fun `what failed goes again, a few times`() {
        val how = mapOf(
            "a" to SendOutcome.SENT,
            "b" to SendOutcome.FAILED,
            "c" to SendOutcome.CANCELLED,
            "d" to SendOutcome.FAILED,
        )
        val handed = listOf("a" to 1, "b" to 1, "c" to 1, "d" to UploadRetry.MAX_SENDS)
        assertEquals(
            "b failed once: again; d gave out",
            listOf("b" to 1),
            UploadRetry.again(handed, 1_000) { how[it] },
        )
        assertEquals("not known: again", listOf("e" to 2), UploadRetry.again(listOf("e" to 2), 1_000) { null })
    }

    @Test
    fun `one still waiting is waited for, a while`() {
        val waiting: (String) -> SendOutcome = { SendOutcome.PENDING }
        assertNull(UploadRetry.again(listOf("a" to 1), 1_000, waiting))
        assertEquals(
            "never run by the service: again",
            listOf("a" to 1),
            UploadRetry.again(listOf("a" to 1), UploadRetry.START_GRACE_MS, waiting),
        )
    }
}
