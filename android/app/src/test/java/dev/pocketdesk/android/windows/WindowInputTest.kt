package dev.pocketdesk.android.windows

import dev.pocketdesk.android.windows.WindowInput.Event
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class WindowInputTest {
    @Test
    fun aFractionLandsOnTheFirstAndLastPixel() {
        assertEquals(0f, WindowInput.pixel(0, 720), 0f)
        assertEquals(719f, WindowInput.pixel(WindowInput.FRACTION_MAX, 720), 0f)
        assertEquals(359.5f, WindowInput.pixel(WindowInput.FRACTION_MAX / 2 + 1, 720), 0.6f)
        // Past the ends is held to them, and a one-pixel side has one place.
        assertEquals(0f, WindowInput.pixel(-5, 720), 0f)
        assertEquals(719f, WindowInput.pixel(99_999, 720), 0f)
        assertEquals(0f, WindowInput.pixel(30_000, 1), 0f)
    }

    @Test
    fun aTouchIsADownMoveOrUpAndNothingElse() {
        assertEquals(WindowInput.ACTION_DOWN, WindowInput.motionAction("down"))
        assertEquals(WindowInput.ACTION_MOVE, WindowInput.motionAction("move"))
        assertEquals(WindowInput.ACTION_UP, WindowInput.motionAction("up"))
        assertNull(WindowInput.motionAction("cancel"))
        assertNull(WindowInput.motionAction(""))
    }

    @Test
    fun theWheelTurnsTheWayAndroidsAxisDoes() {
        // The computer's notches are positive down; Android's vertical axis is positive up.
        assertEquals(-3f, WindowInput.verticalScroll(3), 0f)
        assertEquals(2f, WindowInput.verticalScroll(-2), 0f)
        assertEquals(-WindowInput.MAX_NOTCHES.toFloat(), WindowInput.verticalScroll(500), 0f)
    }

    @Test
    fun aWindowTakesBackAndNotHomeOrRecents() {
        assertEquals(WindowInput.KEYCODE_BACK, WindowInput.globalKey("back"))
        assertNull(WindowInput.globalKey("home"))
        assertNull(WindowInput.globalKey("recents"))
        assertNull(WindowInput.globalKey("notifications"))
    }

    private fun batch(vararg events: String) = JSONObject("""{"sent":1,"events":[${events.joinToString(",")}]}""")

    @Test
    fun aBatchForAWindowCarriesItsEventsWithTheWindow() {
        val got = WindowInput.forWindows(
            batch(
                """{"type":"window","window":2}""",
                """{"type":"touch","action":"down","x":10,"y":20}""",
                """{"type":"scroll","x":1,"y":2,"dx":0,"dy":3}""",
                """{"type":"text","text":"hi"}""",
                """{"type":"backspace"}""",
                """{"type":"enter"}""",
                """{"type":"global","action":"back"}""",
                """{"type":"global","action":"home"}""",
            ),
        )
        assertEquals(
            listOf(
                WindowInput.For(2, Event.Touch(WindowInput.ACTION_DOWN, 10, 20)),
                WindowInput.For(2, Event.Scroll(1, 2, 3)),
                WindowInput.For(2, Event.Text("hi")),
                WindowInput.For(2, Event.Key(WindowInput.KEYCODE_DEL)),
                WindowInput.For(2, Event.Key(WindowInput.KEYCODE_ENTER)),
                WindowInput.For(2, Event.Key(WindowInput.KEYCODE_BACK)),
            ),
            got,
        )
    }

    @Test
    fun aBatchNotForAWindowIsLeftToThePhonesScreen() {
        assertNull(WindowInput.forWindows(batch("""{"type":"touch","action":"down","x":1,"y":1}""")))
        assertNull(WindowInput.forWindows(batch()))
        assertNull(WindowInput.forWindows(JSONObject("""{"sent":1}""")))
        // A window event later in the batch does not make it a window's.
        assertNull(
            WindowInput.forWindows(
                batch("""{"type":"enter"}""", """{"type":"window","window":1}""", """{"type":"enter"}"""),
            ),
        )
    }

    @Test
    fun aWindowPastTheCapTakesNothingAndAnotherWindowTakesItsOwn() {
        val got = WindowInput.forWindows(
            batch(
                """{"type":"window","window":9}""",
                """{"type":"enter"}""",
                """{"type":"window","window":3}""",
                """{"type":"backspace"}""",
            ),
        )
        assertEquals(listOf(WindowInput.For(3, Event.Key(WindowInput.KEYCODE_DEL))), got)
    }

    @Test
    fun theCloseOfAWindowIsAnEventOfThatWindow() {
        val got = WindowInput.forWindows(batch("""{"type":"window","window":4}""", """{"type":"close"}"""))
        assertEquals(listOf(WindowInput.For(4, Event.Close)), got)
    }

    @Test
    fun typedTextIsCutToTheLimitAndEmptyTextIsNothing() {
        val long = "a".repeat(WindowInput.MAX_TEXT + 50)
        val got = WindowInput.forWindows(
            batch("""{"type":"window","window":1}""", """{"type":"text","text":"$long"}""", """{"type":"text","text":""}"""),
        )
        assertEquals(1, got?.size)
        assertEquals(WindowInput.MAX_TEXT, (got?.first()?.event as Event.Text).text.length)
    }
}
