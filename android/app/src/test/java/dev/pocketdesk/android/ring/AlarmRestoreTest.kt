package dev.pocketdesk.android.ring

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AlarmRestoreTest {
    @Test
    fun aRaisedVolumeGoesBackToWhatItWas() {
        for (afterDeath in listOf(false, true)) {
            assertEquals(5, AlarmRestore.target(before = 5, now = 16, max = 16, min = 1, afterDeath = afterDeath))
            assertEquals(6, AlarmRestore.target(before = 6, now = 16, max = 16, min = 6, afterDeath = afterDeath))
            assertEquals(9, AlarmRestore.target(before = 9, now = 16, max = 16, min = 6, afterDeath = afterDeath))
        }
    }

    /** S2, on the OPPO: Android 14 lets an app set the alarm no lower than 6. */
    @Test
    fun itGoesNoLowerThanTheAppMaySet() {
        assertEquals(6, AlarmRestore.target(before = 1, now = 16, max = 16, min = 6, afterDeath = true))
    }

    /** S2 review: a volume key pressed during the ring, or the stream muted, at its end. */
    @Test
    fun theRingsOwnEndAlwaysPutsItBack() {
        assertEquals(5, AlarmRestore.target(before = 5, now = 12, max = 16, min = 1, afterDeath = false))
        assertEquals(5, AlarmRestore.target(before = 5, now = 0, max = 16, min = 1, afterDeath = false))
    }

    /** After a process death, a volume changed since the ring is the owner's. */
    @Test
    fun afterADeathAVolumeChangedSinceIsLeftAlone() {
        assertNull(AlarmRestore.target(before = 1, now = 6, max = 16, min = 6, afterDeath = true))
        assertNull(AlarmRestore.target(before = 3, now = 9, max = 16, min = 1, afterDeath = true))
    }

    @Test
    fun aRingAtTheMaximumAlreadyIsPutBackAsItWas() {
        assertEquals(16, AlarmRestore.target(before = 16, now = 16, max = 16, min = 1, afterDeath = true))
    }

    /** S2 review: a put-back Android keeps ignoring is dropped, not retried for good. */
    @Test
    fun aPutBackThatNeverHoldsIsGivenUp() {
        assertFalse(AlarmRestore.giveUp(AlarmRestore.MAX_TRIES - 1))
        assertTrue(AlarmRestore.giveUp(AlarmRestore.MAX_TRIES))
    }

    /** S2 review: a saved volume from a ring cut short, then the owner's own. */
    @Test
    fun aNewRingKeepsAnEarlierSavedVolumeOnlyWhileItsRaiseStands() {
        assertTrue(AlarmRestore.keepsSaved(now = 16, max = 16))
        assertFalse(AlarmRestore.keepsSaved(now = 9, max = 16))
    }
}
