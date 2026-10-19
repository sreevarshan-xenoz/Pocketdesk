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
}
