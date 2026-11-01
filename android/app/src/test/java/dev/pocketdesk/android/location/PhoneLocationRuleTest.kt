package dev.pocketdesk.android.location

import dev.pocketdesk.android.location.PhoneLocationRule.Precision
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Made-up places only (open sea off West Africa), never a real one. */
class PhoneLocationRuleTest {
    @Test
    fun aPrecisionIsCityUnlessExactIsSaid() {
        assertEquals(Precision.EXACT, Precision.of("exact"))
        assertEquals(Precision.EXACT, Precision.of(" EXACT "))
        assertEquals(Precision.CITY, Precision.of("city"))
        assertEquals(Precision.CITY, Precision.of("street"))
        assertEquals(Precision.CITY, Precision.of(""))
        assertEquals(Precision.CITY, Precision.of(null))
    }

    @Test
    fun exactNeedsTheAskAndTheOwnersChoice() {
        assertEquals(Precision.EXACT, PhoneLocationRule.heldTo(Precision.EXACT, exactAllowed = true))
        assertEquals(Precision.CITY, PhoneLocationRule.heldTo(Precision.EXACT, exactAllowed = false))
        assertEquals(Precision.CITY, PhoneLocationRule.heldTo(Precision.CITY, exactAllowed = true))
        assertEquals(Precision.CITY, PhoneLocationRule.heldTo(Precision.CITY, exactAllowed = false))
        // The owner's choice counts only with the fine permission.
        assertTrue(PhoneLocationRule.exactAllowed(ownerAllows = true, fineGranted = true))
        assertFalse(PhoneLocationRule.exactAllowed(ownerAllows = true, fineGranted = false))
        assertFalse(PhoneLocationRule.exactAllowed(ownerAllows = false, fineGranted = true))
    }

    @Test
    fun aCityFixIsRoundedToAboutAKilometre() {
        val fix = PhoneLocationRule.fixFor(1.23456, -20.98765, 12f, Precision.CITY)!!
        assertEquals(1.23, fix.lat, 0.0)
        assertEquals(-20.99, fix.lon, 0.0)
        assertEquals(PhoneLocationRule.CITY_ACCURACY_M, fix.accuracyM)
        assertEquals(Precision.CITY, fix.precision)
        // Never moved by more than half a step.
        assertTrue(Math.abs(fix.lat - 1.23456) <= PhoneLocationRule.CITY_STEP_DEGREES / 2 + 1e-9)
        assertTrue(Math.abs(fix.lon - -20.98765) <= PhoneLocationRule.CITY_STEP_DEGREES / 2 + 1e-9)
        // A looser accuracy than a kilometre stays.
        assertEquals(5000, PhoneLocationRule.fixFor(1.0, 2.0, 5000f, Precision.CITY)!!.accuracyM)
    }

    @Test
    fun anExactFixKeepsTheReading() {
        val fix = PhoneLocationRule.fixFor(1.23456, -20.98765, 7.2f, Precision.EXACT)!!
        assertEquals(1.23456, fix.lat, 0.0)
        assertEquals(-20.98765, fix.lon, 0.0)
        assertEquals(8, fix.accuracyM)
        assertEquals(Precision.EXACT, fix.precision)
    }

    @Test
    fun roundingIsSteadyAndMeetsTheEdges() {
        val once = PhoneLocationRule.fixFor(89.999, 179.999, 5f, Precision.CITY)!!
        assertEquals(90.0, once.lat, 0.0)
        assertEquals(180.0, once.lon, 0.0)
        val twice = PhoneLocationRule.fixFor(once.lat, once.lon, once.accuracyM.toFloat(), Precision.CITY)!!
        assertEquals(once, twice)
        val south = PhoneLocationRule.fixFor(-0.004, -0.006, 5f, Precision.CITY)!!
        assertEquals(0.0, Math.abs(south.lat), 0.0)
        assertEquals(-0.01, south.lon, 0.0)
    }

    @Test
    fun aReadingThatIsNotAPlaceIsNoFix() {
        assertNull(PhoneLocationRule.fixFor(Double.NaN, 1.0, 5f, Precision.EXACT))
        assertNull(PhoneLocationRule.fixFor(1.0, Double.POSITIVE_INFINITY, 5f, Precision.EXACT))
        assertNull(PhoneLocationRule.fixFor(90.5, 1.0, 5f, Precision.EXACT))
        assertNull(PhoneLocationRule.fixFor(1.0, -180.5, 5f, Precision.EXACT))
        assertNotNull(PhoneLocationRule.fixFor(-90.0, 180.0, 5f, Precision.EXACT))
    }

    @Test
    fun theAccuracyIsHeldToWhatAFixMayClaim() {
        fun accuracy(value: Float) = PhoneLocationRule.fixFor(1.0, 2.0, value, Precision.EXACT)!!.accuracyM
        assertEquals(1, accuracy(0f))
        assertEquals(13, accuracy(12.2f))
        assertEquals(PhoneLocationRule.WORST_ACCURACY_M, accuracy(1e12f))
        assertEquals(PhoneLocationRule.WORST_ACCURACY_M, accuracy(-5f))
        assertEquals(PhoneLocationRule.WORST_ACCURACY_M, accuracy(Float.NaN))
    }

    @Test
    fun theSwitchAndThePermissionDecideWhetherItAnswers() {
        assertEquals("off", PhoneLocationRule.refusal(on = false, coarseGranted = true, fineGranted = true))
        assertEquals("permission", PhoneLocationRule.refusal(on = true, coarseGranted = false, fineGranted = false))
        assertNull(PhoneLocationRule.refusal(on = true, coarseGranted = true, fineGranted = false))
        // Fine implies coarse on Android, so fine alone also lets it answer.
        assertNull(PhoneLocationRule.refusal(on = true, coarseGranted = false, fineGranted = true))
    }

    @Test
    fun noFixSaysWhyOnlyWhenBackgroundHidItAndThePhoneIsNotOnScreen() {
        assertEquals("background", PhoneLocationRule.noFix(backgroundGranted = false, appOnScreen = false))
        assertEquals("unavailable", PhoneLocationRule.noFix(backgroundGranted = true, appOnScreen = false))
        assertEquals("unavailable", PhoneLocationRule.noFix(backgroundGranted = false, appOnScreen = true))
    }

    @Test
    fun aFixGoesNoMoreOftenThanEveryHalfMinute() {
        val every = PhoneLocationRule.ANSWER_EVERY_MS
        assertTrue(PhoneLocationRule.mayAnswer(0L, 1L))
        assertFalse(PhoneLocationRule.mayAnswer(1_000L, 1_000L + every - 1))
        assertTrue(PhoneLocationRule.mayAnswer(1_000L, 1_000L + every))
    }
}
