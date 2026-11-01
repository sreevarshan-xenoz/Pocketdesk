package dev.pocketdesk.android.windows

import dev.pocketdesk.android.windows.WindowInput.ACTION_DOWN
import dev.pocketdesk.android.windows.WindowInput.ACTION_MOVE
import dev.pocketdesk.android.windows.WindowInput.ACTION_POINTER_DOWN
import dev.pocketdesk.android.windows.WindowInput.ACTION_POINTER_INDEX_SHIFT
import dev.pocketdesk.android.windows.WindowInput.ACTION_POINTER_UP
import dev.pocketdesk.android.windows.WindowInput.ACTION_UP
import org.json.JSONObject
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PointerTrackerTest {
    private fun at(index: Int, action: Int) = action or (index shl ACTION_POINTER_INDEX_SHIFT)

    @Test
    fun oneFingerIsDownMoveUp() {
        val t = PointerTracker()
        val down = t.apply(0, ACTION_DOWN, 10, 20, 1000)!!
        assertEquals(ACTION_DOWN, down.action)
        assertArrayEquals(intArrayOf(0), down.ids)
        assertTrue(t.active)
        val move = t.apply(0, ACTION_MOVE, 11, 20, 1010)!!
        assertEquals(ACTION_MOVE, move.action)
        assertEquals(1000L, move.downTime)
        val up = t.apply(0, ACTION_UP, 11, 20, 1020)!!
        assertEquals(ACTION_UP, up.action)
        assertFalse(t.active)
    }

    @Test
    fun aStickAnAimAndATapAreOneGestureOfThreeFingers() {
        val t = PointerTracker()
        assertEquals(ACTION_DOWN, t.apply(1, ACTION_DOWN, 100, 100, 5)!!.action)
        val aim = t.apply(2, ACTION_DOWN, 900, 100, 6)!!
        assertEquals(at(1, ACTION_POINTER_DOWN), aim.action)
        assertArrayEquals(intArrayOf(1, 2), aim.ids)
        assertEquals(5L, aim.downTime)
        val tap = t.apply(3, ACTION_DOWN, 500, 500, 7)!!
        assertEquals(at(2, ACTION_POINTER_DOWN), tap.action)
        assertArrayEquals(intArrayOf(1, 2, 3), tap.ids)
        assertArrayEquals(intArrayOf(100, 900, 500), tap.xs)
        // A move of one finger carries all three.
        val move = t.apply(2, ACTION_MOVE, 910, 105, 8)!!
        assertEquals(ACTION_MOVE, move.action)
        assertArrayEquals(intArrayOf(100, 910, 500), move.xs)
        assertArrayEquals(intArrayOf(100, 105, 500), move.ys)
        // The finger in the middle lifts: its index, and it is still in that event.
        val lift = t.apply(2, ACTION_UP, 910, 105, 9)!!
        assertEquals(at(1, ACTION_POINTER_UP), lift.action)
        assertArrayEquals(intArrayOf(1, 2, 3), lift.ids)
        assertEquals(2, t.count)
        val next = t.apply(3, ACTION_UP, 500, 500, 10)!!
        assertEquals(at(1, ACTION_POINTER_UP), next.action)
        assertArrayEquals(intArrayOf(1, 3), next.ids)
        // The last one up is a plain UP.
        assertEquals(ACTION_UP, t.apply(1, ACTION_UP, 100, 100, 11)!!.action)
        assertFalse(t.active)
        // A new gesture starts fresh, with its own time.
        assertEquals(20L, t.apply(4, ACTION_DOWN, 1, 1, 20)!!.downTime)
    }

    @Test
    fun whatMeansNothingIsLeftOut() {
        val t = PointerTracker()
        assertNull("a move with nothing down", t.apply(1, ACTION_MOVE, 5, 5, 1))
        assertNull("an up with nothing down", t.apply(1, ACTION_UP, 5, 5, 1))
        assertNull("a finger that is not one", t.apply(WindowInput.MAX_POINTERS, ACTION_DOWN, 5, 5, 1))
        assertNull("a finger below zero", t.apply(-1, ACTION_DOWN, 5, 5, 1))
        assertNull("a cancel", t.apply(1, 3, 5, 5, 1))
        assertNotNull(t.apply(1, ACTION_DOWN, 5, 5, 1))
        assertNull("a move that did not move", t.apply(1, ACTION_MOVE, 5, 5, 2))
        // A second down of the same finger is a move, never a second finger.
        assertEquals(ACTION_MOVE, t.apply(1, ACTION_DOWN, 6, 5, 3)!!.action)
        assertEquals(1, t.count)
    }

    @Test
    fun noMoreFingersThanTheCap() {
        val t = PointerTracker()
        for (id in 0 until WindowInput.MAX_POINTERS) assertNotNull(t.apply(id, ACTION_DOWN, id, id, 1))
        assertEquals(WindowInput.MAX_POINTERS, t.count)
        assertEquals(WindowInput.MAX_POINTERS, t.apply(0, ACTION_MOVE, 100, 100, 2)!!.ids.size)
    }

    @Test
    fun releasingAllLiftsEveryFingerAndEndsInUp() {
        val t = PointerTracker()
        t.apply(1, ACTION_DOWN, 1, 1, 1)
        t.apply(2, ACTION_DOWN, 2, 2, 1)
        t.apply(3, ACTION_DOWN, 3, 3, 1)
        val frames = t.releaseAll()
        assertEquals(3, frames.size)
        assertEquals(at(2, ACTION_POINTER_UP), frames[0].action)
        assertEquals(at(1, ACTION_POINTER_UP), frames[1].action)
        assertEquals(ACTION_UP, frames[2].action)
        assertFalse(t.active)
        assertTrue(t.releaseAll().isEmpty())
    }

    @Test
    fun aPointerEventNamesItsFingerAndKeepsToTheCap() {
        val ok = WindowInput.parse(JSONObject("""{"type":"pointer","pointer":3,"action":"down","x":5,"y":6}"""))
        assertEquals(WindowInput.Event.Pointer(3, ACTION_DOWN, 5, 6), ok)
        assertNull(WindowInput.parse(JSONObject("""{"type":"pointer","pointer":10,"action":"down","x":5,"y":6}""")))
        assertNull(WindowInput.parse(JSONObject("""{"type":"pointer","pointer":-1,"action":"down","x":5,"y":6}""")))
        assertNull(WindowInput.parse(JSONObject("""{"type":"pointer","action":"down","x":5,"y":6}""")))
        assertNull(WindowInput.parse(JSONObject("""{"type":"pointer","pointer":1,"action":"cancel","x":5,"y":6}""")))
        // In a batch it is for the window named, and a batch for the screen never holds it.
        val batch = JSONObject(
            """{"sent":1,"events":[{"type":"window","window":2},{"type":"pointer","pointer":1,"action":"down","x":1,"y":2}]}""",
        )
        val got = WindowInput.forWindows(batch)!!
        assertEquals(WindowInput.For(2, WindowInput.Event.Pointer(1, ACTION_DOWN, 1, 2)), got.single())
        val screen = JSONObject("""{"sent":1,"events":[{"type":"pointer","pointer":1,"action":"down","x":1,"y":2}]}""")
        assertNull(WindowInput.forWindows(screen))
    }

    @Test
    fun aFingerGoingDownOrUpIsNeverRateLimited() {
        val edge = { e: WindowInput.Event -> WindowInput.isEdge(e) }
        assertTrue(edge(WindowInput.Event.Pointer(2, ACTION_DOWN, 1, 1)))
        assertTrue(edge(WindowInput.Event.Pointer(2, ACTION_UP, 1, 1)))
        assertFalse(edge(WindowInput.Event.Pointer(2, ACTION_MOVE, 1, 1)))
        assertTrue(edge(WindowInput.Event.Touch(ACTION_UP, 1, 1)))
        assertFalse(edge(WindowInput.Event.Touch(ACTION_MOVE, 1, 1)))
        assertTrue(edge(WindowInput.Event.Close))
        assertFalse(edge(WindowInput.Event.Scroll(1, 1, 3)))
        assertFalse(edge(WindowInput.Event.Key(WindowInput.KEYCODE_ENTER)))
        assertFalse(edge(WindowInput.Event.Text("a")))
    }

    @Test
    fun theHelperTakesOnlySoundFingerSets() {
        val d = ACTION_DOWN
        val ids1 = intArrayOf(1)
        val f1 = floatArrayOf(10f)
        assertTrue(WindowRule.pointersOk(d, ids1, f1, f1, 720, 1280))
        assertTrue(WindowRule.pointersOk(ACTION_UP, ids1, f1, f1, 720, 1280))
        val two = intArrayOf(1, 2)
        val f2 = floatArrayOf(10f, 20f)
        assertTrue(WindowRule.pointersOk(ACTION_MOVE, two, f2, f2, 720, 1280))
        assertTrue(WindowRule.pointersOk(at(1, ACTION_POINTER_DOWN), two, f2, f2, 720, 1280))
        assertTrue(WindowRule.pointersOk(at(0, ACTION_POINTER_UP), two, f2, f2, 720, 1280))
        // A first DOWN with two fingers, an index past the fingers, a POINTER_DOWN of one.
        assertFalse(WindowRule.pointersOk(d, two, f2, f2, 720, 1280))
        assertFalse(WindowRule.pointersOk(at(2, ACTION_POINTER_DOWN), two, f2, f2, 720, 1280))
        assertFalse(WindowRule.pointersOk(at(0, ACTION_POINTER_DOWN), ids1, f1, f1, 720, 1280))
        assertFalse(WindowRule.pointersOk(d or (1 shl ACTION_POINTER_INDEX_SHIFT), ids1, f1, f1, 720, 1280))
        // Another action, a repeated or past-the-cap id, odd lengths, no fingers, bad numbers.
        assertFalse(WindowRule.pointersOk(WindowInput.ACTION_SCROLL, ids1, f1, f1, 720, 1280))
        assertFalse(WindowRule.pointersOk(3, ids1, f1, f1, 720, 1280))
        assertFalse(WindowRule.pointersOk(ACTION_MOVE, intArrayOf(1, 1), f2, f2, 720, 1280))
        assertFalse(WindowRule.pointersOk(d, intArrayOf(WindowInput.MAX_POINTERS), f1, f1, 720, 1280))
        assertFalse(WindowRule.pointersOk(d, intArrayOf(-1), f1, f1, 720, 1280))
        assertFalse(WindowRule.pointersOk(ACTION_MOVE, two, f1, f2, 720, 1280))
        assertFalse(WindowRule.pointersOk(ACTION_MOVE, IntArray(0), FloatArray(0), FloatArray(0), 720, 1280))
        assertFalse(WindowRule.pointersOk(ACTION_MOVE, IntArray(11) { it }, FloatArray(11), FloatArray(11), 720, 1280))
        assertFalse(WindowRule.pointersOk(d, ids1, floatArrayOf(Float.NaN), f1, 720, 1280))
        assertFalse(WindowRule.pointersOk(d, ids1, f1, floatArrayOf(Float.POSITIVE_INFINITY), 720, 1280))
        assertFalse(WindowRule.pointersOk(d, ids1, f1, f1, 0, 1280))
        // A point past an edge is held to it.
        assertEquals(719f, WindowRule.clampPixel(5000f, 720), 0f)
        assertEquals(0f, WindowRule.clampPixel(-4f, 720), 0f)
    }
}
