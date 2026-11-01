package dev.pocketdesk.android.windows

import dev.pocketdesk.android.windows.WindowRule.Decision
import dev.pocketdesk.android.windows.WindowRule.Mode
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class WindowRuleTest {
    @Test
    fun windowsAreOnOnlyWithShizukuItsPermissionAndTheOwnersSwitch() {
        assertEquals(Mode.ON, WindowRule.mode(34, shizukuRunning = true, permitted = true, switchOn = true))
        // The owner's switch is off by default: nothing else makes it on.
        assertEquals(Mode.OFF, WindowRule.mode(34, shizukuRunning = true, permitted = true, switchOn = false))
        assertEquals(Mode.OFF, WindowRule.mode(34, shizukuRunning = true, permitted = false, switchOn = false))
        assertEquals(Mode.NEEDS_PERMISSION, WindowRule.mode(34, shizukuRunning = true, permitted = false, switchOn = true))
        assertEquals(Mode.NO_SHIZUKU, WindowRule.mode(34, shizukuRunning = false, permitted = true, switchOn = true))
        assertEquals(Mode.TOO_OLD, WindowRule.mode(28, shizukuRunning = true, permitted = true, switchOn = true))
        assertEquals(Mode.ON, WindowRule.mode(WindowRule.MIN_SDK, true, true, true))
    }

    @Test
    fun theRowIsShownOnlyWithShizukuRunningOrWhileItsSwitchIsOn() {
        assertTrue(WindowRule.rowShown(34, shizukuRunning = true, switchOn = false))
        assertFalse("nobody is told about Shizuku", WindowRule.rowShown(34, shizukuRunning = false, switchOn = false))
        assertTrue("it can always be turned off", WindowRule.rowShown(34, shizukuRunning = false, switchOn = true))
        assertFalse(WindowRule.rowShown(28, shizukuRunning = true, switchOn = true))
    }

    @Test
    fun aWindowIsMadeOnlyInModeOnWithinTheCapAndNeverAnIdTwice() {
        assertEquals(Decision.START, WindowRule.decide(Mode.ON, 1, emptySet()))
        assertEquals(Decision.START, WindowRule.decide(Mode.ON, 4, setOf(1, 2, 3)))
        assertEquals(Decision.LIMIT, WindowRule.decide(Mode.ON, 1, setOf(1)))
        assertEquals(Decision.LIMIT, WindowRule.decide(Mode.ON, 5, setOf(1, 2, 3, 4)))
        for (mode in Mode.entries.filter { it != Mode.ON }) {
            assertEquals("$mode", Decision.UNAVAILABLE, WindowRule.decide(mode, 1, emptySet()))
        }
    }

    @Test
    fun theListSaysWindowsAreOnOnlyInModeOn() {
        assertEquals(true, WindowRule.listBody(Mode.ON).getBoolean("on"))
        assertEquals(WindowRule.MAX_WINDOWS, WindowRule.listBody(Mode.ON).getInt("max"))
        for (mode in Mode.entries.filter { it != Mode.ON }) {
            assertEquals("$mode", false, WindowRule.listBody(mode).getBoolean("on"))
        }
    }

    @Test
    fun theDisplayIsOwnTrustedAndTouchableAsTheAndroidAllows() {
        val old = WindowRule.displayFlags(29)
        val newer = WindowRule.displayFlags(34)
        assertTrue("public and its own content", old and 1 != 0 && old and (1 shl 3) != 0)
        assertTrue("it takes touch", old and (1 shl 6) != 0)
        assertEquals("nothing trusted before 13", 0, old and (1 shl 10))
        assertTrue("trusted from 13", WindowRule.displayFlags(33) and (1 shl 10) != 0)
        assertEquals("no own focus before 14", 0, WindowRule.displayFlags(33) and (1 shl 14))
        assertTrue("own focus from 14", newer and (1 shl 14) != 0)
    }

    @Test
    fun onlyAPackageAndAClassReachTheShell() {
        assertTrue(WindowRule.componentOk("org.example.app/org.example.app.Main"))
        assertTrue(WindowRule.componentOk("org.example.app/.Main".replace("/.", "/")))
        assertTrue(WindowRule.componentOk("org.example.app/org.example.app.Main\$Inner"))
        for (bad in listOf(
            "",
            "org.example.app",
            "org.example.app/",
            "org.example.app/Main; reboot",
            "org.example.app/Main --user 0",
            "org.example.app /Main",
            "org.example.app/Main\nx",
            "../x/Main",
            "-n/Main",
            "noDots/Main",
        )) {
            assertFalse(bad, WindowRule.componentOk(bad))
        }
    }

    @Test
    fun anAskIsReadOnlyWhenSound() {
        val ok = JSONObject("""{"id":2,"width":720,"height":1280,"dpi":320}""")
        assertEquals(WindowRule.Ask(2, 720, 1280, 320), WindowRule.parseAsk(ok))
        assertNull(WindowRule.parseAsk(null))
        for (bad in listOf(
            """{"id":0,"width":720,"height":1280,"dpi":320}""",
            """{"id":5,"width":720,"height":1280,"dpi":320}""",
            """{"id":1,"width":10,"height":1280,"dpi":320}""",
            """{"id":1,"width":720,"height":9000,"dpi":320}""",
            """{"id":1,"width":720,"height":1280,"dpi":50}""",
            """{"id":1}""",
            """{}""",
        )) {
            assertNull(bad, WindowRule.parseAsk(JSONObject(bad)))
        }
    }
}
