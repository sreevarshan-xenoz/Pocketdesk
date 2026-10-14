package dev.pocketdesk.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ImageClipRuleTest {
    @Test
    fun anImageIsSentOnceAndNeverOneThePhonePutThere() {
        val rule = ImageClipRule()
        assertTrue(rule.shouldSend("a", ours = false))
        rule.moved("a")
        assertFalse("the same image again", rule.shouldSend("a", ours = false))
        assertFalse("an image this app put on the clipboard", rule.shouldSend("b", ours = true))
        assertTrue(rule.shouldSend("b", ours = false))
    }

    @Test
    fun afterTextMovesTheSameImageMayMoveAgain() {
        val rule = ImageClipRule()
        rule.moved("a")
        rule.textMoved()
        assertTrue("copied again after some text", rule.shouldSend("a", ours = false))
        assertTrue(rule.shouldFetch("a", size = 10, nowMs = 0))
    }

    @Test
    fun theDesktopsCopyOfWhatThePhoneSentIsNotFetchedBack() {
        val rule = ImageClipRule()
        rule.moved("a")
        assertFalse(rule.shouldFetch("a", size = 10, nowMs = 0))
        assertTrue(rule.shouldFetch("c", size = 10, nowMs = 0))
    }

    @Test
    fun aFailedFetchWaitsAMinuteBeforeTryingThatImageAgain() {
        val rule = ImageClipRule()
        rule.fetchFailed("c", nowMs = 1_000)
        assertFalse(rule.shouldFetch("c", size = 10, nowMs = 5_000))
        assertTrue("another image is not held up", rule.shouldFetch("d", size = 10, nowMs = 5_000))
        assertTrue(rule.shouldFetch("c", size = 10, nowMs = 1_000 + ImageClipRule.RETRY_MS))
    }

    @Test
    fun anImageOverTheLimitIsNeverFetched() {
        val rule = ImageClipRule()
        assertFalse(rule.shouldFetch("c", size = ImageClipRule.MAX_BYTES + 1, nowMs = 0))
        assertFalse(rule.shouldFetch("c", size = 0, nowMs = 0))
        assertTrue(rule.shouldFetch("c", size = ImageClipRule.MAX_BYTES, nowMs = 0))
    }

    @Test
    fun aLargePhotoIsDecodedAtMost4096PixelsOnItsLongSide() {
        assertEquals(1, ImageClipRule.sampleFor(1920, 1080))
        assertEquals(1, ImageClipRule.sampleFor(4096, 3072))
        assertEquals(2, ImageClipRule.sampleFor(8000, 6000))
        assertEquals(4, ImageClipRule.sampleFor(9248, 16384))
        assertEquals(1, ImageClipRule.sampleFor(0, 0))
    }

    @Test
    fun theIdIsTheHexSha256OfThePng() {
        assertEquals(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ImageClipRule.idOf("abc".toByteArray()),
        )
    }

    @Test
    fun onlyPngBytesArePassedThroughUnchanged() {
        val png = byteArrayOf(0x89.toByte(), 'P'.code.toByte(), 'N'.code.toByte(), 'G'.code.toByte(), 0x0D, 0x0A, 0x1A, 0x0A, 1)
        assertTrue(ImageClipRule.isPng(png))
        assertFalse(ImageClipRule.isPng("GIF89a".toByteArray()))
        assertFalse(ImageClipRule.isPng(ByteArray(0)))
    }
}
