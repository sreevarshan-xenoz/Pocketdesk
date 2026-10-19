package dev.pocketdesk.android.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class GamepadFitTest {
    @Test
    fun aNarrowPhoneLosesTheMiddleColumnAndASideOneTheBigSticks() {
        // A 360 dp phone upright: Select and Start above, smaller sticks.
        assertEquals(GamepadFit.Fit(narrow = true, stick = 96), GamepadFit.of(360f, 740f))
        // The A71 upright (412 dp): three columns, full sticks.
        assertEquals(GamepadFit.Fit(narrow = false, stick = 116), GamepadFit.of(412f, 860f))
        // On its side: wide, but short.
        assertEquals(GamepadFit.Fit(narrow = false, stick = 96), GamepadFit.of(860f, 380f))
    }
}
