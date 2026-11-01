package dev.pocketdesk.android.cast

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class CameraFeedTest {
    @Test
    fun theLargestWideSizeWithinFullHdIsChosen() {
        val sizes = listOf(4000 to 3000, 1920 to 1080, 1440 to 1080, 1280 to 720, 640 to 480)
        assertEquals(1920 to 1080, CameraFeed.choose(sizes))
        // No 16:9 within the limit: the largest that fits.
        assertEquals(1440 to 1080, CameraFeed.choose(listOf(4000 to 3000, 1440 to 1080, 640 to 480)))
        // Nothing fits: the smallest offered.
        assertEquals(2560 to 1440, CameraFeed.choose(listOf(4000 to 3000, 2560 to 1440)))
        assertNull(CameraFeed.choose(emptyList()))
    }

    /** F6 part 2: the computer's camera at 720p takes 1280 by 720, not the largest. */
    @Test
    fun aCameraAt720pTakesNoMore() {
        val sizes = listOf(4000 to 3000, 1920 to 1080, 1280 to 720, 640 to 480)
        assertEquals(1280 to 720, CameraFeed.choose(sizes, 1280, 720))
        assertEquals(1920 to 1080, CameraFeed.choose(sizes, 1920, 1080))
    }

    @Test
    fun theFrameRateIsSteadyWhereItCanBe() {
        val ranges = listOf(15 to 15, 7 to 30, 15 to 30, 30 to 30, 24 to 24)
        assertEquals(30 to 30, CameraFeed.chooseFps(ranges, 30))
        assertEquals(24 to 24, CameraFeed.chooseFps(ranges, 24))
        assertEquals("the narrowest reaching it", 15 to 30, CameraFeed.chooseFps(listOf(7 to 30, 15 to 30), 30))
        assertEquals("the nearest top", 15 to 30, CameraFeed.chooseFps(listOf(7 to 15, 15 to 30), 24))
        assertNull(CameraFeed.chooseFps(emptyList(), 30))
    }
}
