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
        assertTrue("its own content", old and (1 shl 3) != 0)
        for (sdk in listOf(29, 33, 34)) {
            assertEquals("not public at $sdk", 0, WindowRule.displayFlags(sdk) and 1)
            assertEquals("not a presentation at $sdk", 0, WindowRule.displayFlags(sdk) and (1 shl 1))
        }
        assertTrue("it takes touch", old and (1 shl 6) != 0)
        assertEquals("nothing trusted before 13", 0, old and (1 shl 10))
        assertTrue("trusted from 13", WindowRule.displayFlags(33) and (1 shl 10) != 0)
        assertEquals("no own focus before 14", 0, WindowRule.displayFlags(33) and (1 shl 14))
        assertTrue("own focus from 14", newer and (1 shl 14) != 0)
    }

    @Test
    fun anEventIsInjectedOnlyWhenItCanBeGivenItsDisplayAndSent() {
        // The display call returns nothing: its presence, not its result, is what counts.
        val given = mutableListOf<Pair<String, Int>>()
        val sent = mutableListOf<String>()
        val setDisplay: (String, Int) -> Unit = { e, id -> given.add(e to id) }
        val send: (String) -> Boolean = { e -> sent.add(e); true }
        assertTrue(WindowRule.inject("tap", 7, setDisplay, send))
        assertEquals(listOf("tap" to 7), given)
        assertEquals(listOf("tap"), sent)
        // Without the display call an event would land on the phone's own screen: nothing is sent.
        assertFalse(WindowRule.inject("tap", 7, null, send))
        assertFalse(WindowRule.inject("tap", 7, setDisplay, null))
        assertEquals(listOf("tap"), sent)
        // The display is given before the event is sent, and a refusal is passed on.
        assertFalse(WindowRule.inject("tap", 7, setDisplay) { false })
    }

    @Test
    fun aSessionOfWindowsAloneDropsEveryBatchThatIsNotForAWindow() {
        val t = WindowRule.Target.values()
        assertEquals(WindowRule.Target.WINDOWS, WindowRule.target(windowsOnly = true, forWindows = true, screenControlOn = true))
        assertEquals(WindowRule.Target.DROP, WindowRule.target(windowsOnly = true, forWindows = false, screenControlOn = true))
        assertEquals(WindowRule.Target.DROP, WindowRule.target(windowsOnly = true, forWindows = false, screenControlOn = false))
        // A cast of the screen: its control as before.
        assertEquals(WindowRule.Target.SCREEN, WindowRule.target(windowsOnly = false, forWindows = false, screenControlOn = true))
        assertEquals(WindowRule.Target.DROP, WindowRule.target(windowsOnly = false, forWindows = false, screenControlOn = false))
        assertEquals(WindowRule.Target.WINDOWS, WindowRule.target(windowsOnly = false, forWindows = true, screenControlOn = false))
        assertEquals(3, t.size)
    }

    @Test
    fun theHelperTakesCallsFromThisAppAndOnlyDestroyFromShizuku() {
        assertTrue(WindowRule.callerAllowed(10_123, 10_123, destroy = false))
        assertFalse("another app", WindowRule.callerAllowed(10_999, 10_123, destroy = false))
        assertFalse("the shell is not this app", WindowRule.callerAllowed(2000, 10_123, destroy = false))
        assertFalse("nobody while the app's uid is unknown", WindowRule.callerAllowed(10_123, -1, destroy = false))
        assertTrue("Shizuku's server may end it", WindowRule.callerAllowed(2000, 10_123, destroy = true))
        assertTrue(WindowRule.callerAllowed(0, -1, destroy = true))
        assertFalse("another app may not end it", WindowRule.callerAllowed(10_999, 10_123, destroy = true))
    }

    @Test
    fun theHelperPressesBackEnterAndDeleteOnly() {
        assertTrue(WindowRule.keyAllowed(WindowInput.KEYCODE_BACK))
        assertTrue(WindowRule.keyAllowed(WindowInput.KEYCODE_ENTER))
        assertTrue(WindowRule.keyAllowed(WindowInput.KEYCODE_DEL))
        for (code in listOf(3 /* home */, 26 /* power */, 187 /* app switch */, 4 + 1, 0, -1)) {
            assertFalse("$code", WindowRule.keyAllowed(code))
        }
    }

    @Test
    fun theSystemsOwnScreensAreNeverOpenedInAWindow() {
        val own = "dev.pocketdesk.android"
        for (pkg in listOf(
            own, "com.android.settings", "com.android.systemui", "com.android.packageinstaller",
            "com.google.android.permissioncontroller", "moe.shizuku.privileged.api", "com.android.vending",
        )) {
            assertTrue(pkg, WindowRule.denied(pkg, own, emptySet()))
        }
        // What the phone's own intents resolve to is refused too, whatever its name.
        assertTrue(WindowRule.denied("com.maker.settings", own, setOf("com.maker.settings")))
        assertFalse(WindowRule.denied("org.example.notes", own, setOf("com.maker.settings")))
        assertFalse(WindowRule.denied("org.example.notes", "dev.pocketdesk.android", emptySet()))
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
