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

    /** Review: only one that failed for the last time is given up. */
    @Test
    fun `only what failed at its last send is given up`() {
        val how = mapOf("a" to SendOutcome.FAILED, "b" to SendOutcome.SENT, "c" to SendOutcome.FAILED)
        val handed = listOf("a" to UploadRetry.MAX_SENDS, "b" to UploadRetry.MAX_SENDS, "c" to 1)
        assertEquals(1, UploadRetry.givenUp(handed) { how[it] })
    }

    /** Review: one that failed waits with one still running, within the grace. */
    @Test
    fun `a failure waits for the rest of its batch`() {
        val how = mapOf("a" to SendOutcome.FAILED, "b" to SendOutcome.PENDING)
        assertNull(UploadRetry.again(listOf("a" to 1, "b" to 1), 1_000) { how[it] })
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

    /**
     * A file held for the Bluetooth road (over 8 MiB, or a call or music)
     * goes again however often it was held, and is never given up: its
     * held send is taken back from its count.
     */
    @Test
    fun `one held for the Bluetooth road goes again and is never given up`() {
        val how = mapOf("a" to SendOutcome.WAITED, "b" to SendOutcome.WAITED, "c" to SendOutcome.FAILED)
        val handed = listOf("a" to 1, "b" to UploadRetry.MAX_SENDS, "c" to UploadRetry.MAX_SENDS)
        assertEquals(
            "held ones again, their sends not counted; c gave out",
            listOf("a" to 0, "b" to UploadRetry.MAX_SENDS - 1),
            UploadRetry.again(handed, 1_000) { how[it] },
        )
        assertEquals("only c is given up", 1, UploadRetry.givenUp(handed) { how[it] })
        // Handed again (the uploader adds one), held again: the count stays.
        var sends = UploadRetry.MAX_SENDS
        repeat(5) {
            sends = UploadRetry.again(listOf("a" to sends + 1), 1_000) { SendOutcome.WAITED }!!.single().second
        }
        assertEquals(UploadRetry.MAX_SENDS, sends)
    }
}
