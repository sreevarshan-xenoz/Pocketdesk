package dev.pocketdesk.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class NavigationTest {
    @Test
    fun backGoesUpToHomeFromEachScreenAndLeavesFromHome() {
        for (route in listOf(Route.FORM, Route.SETTINGS, Route.SCAN)) {
            assertEquals(route, Route.HOME, Navigation.backTo(route))
        }
        assertNull(Navigation.backTo(Route.HOME))
    }
}
