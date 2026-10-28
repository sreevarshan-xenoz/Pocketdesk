package dev.pocketdesk.android.control

import dev.pocketdesk.android.control.PointerPlan.Act
import dev.pocketdesk.android.control.PointerPlan.Event
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PointerPlanTest {
    private fun enter(x: Int = 100, y: Int = 200) = Event("enter", x = x, y = y)
    private fun pointer(x: Int, y: Int) = Event("pointer", x = x, y = y)
    private fun button(button: String, action: String) = Event("button", button = button, action = action)

    @Test
    fun `taken from its own computer with the switch and the service on`() {
        assertEquals(PointerPlan.Decision.TAKE, PointerPlan.decide(on = true, service = true, own = true))
        assertEquals(PointerPlan.Decision.OFF, PointerPlan.decide(on = false, service = true, own = true))
        assertEquals(PointerPlan.Decision.SERVICE_OFF, PointerPlan.decide(on = true, service = false, own = true))
        assertEquals(
            PointerPlan.Decision.NOT_OWN_COMPUTER,
            PointerPlan.decide(on = true, service = true, own = false),
        )
    }

    @Test
    fun `the pointer shows where the cursor came on and follows it`() {
        val p = PointerPlan.Pointer()
        assertEquals(listOf(Act.Pointer(100, 200)), p.take(enter(), 0))
        assertTrue(p.on)
        assertEquals(listOf(Act.Pointer(300, 400)), p.take(pointer(300, 400), 10))
        // A ping only says the computer is there.
        assertEquals(emptyList<Act>(), p.take(Event("ping"), 20))
    }

    @Test
    fun `a left click taps and a drag swipes`() {
        val p = PointerPlan.Pointer()
        p.take(enter(), 0)
        assertEquals(listOf(Act.Touch("down", 100, 200)), p.take(button("left", "down"), 1))
        assertEquals(
            listOf(Act.Pointer(150, 260), Act.Touch("move", 150, 260)),
            p.take(pointer(150, 260), 2),
        )
        assertEquals(listOf(Act.Touch("up", 150, 260)), p.take(button("left", "up"), 3))
        // A release with no press is nothing; moves after it touch nothing.
        assertEquals(emptyList<Act>(), p.take(button("left", "up"), 4))
        assertEquals(listOf(Act.Pointer(10, 10)), p.take(pointer(10, 10), 5))
    }

    @Test
    fun `right is Back, middle is Home, the wheel scrolls where the pointer is`() {
        val p = PointerPlan.Pointer()
        p.take(enter(500, 600), 0)
        assertEquals(listOf(Act.Global("back")), p.take(button("right", "down"), 1))
        assertEquals(emptyList<Act>(), p.take(button("right", "up"), 2))
        assertEquals(listOf(Act.Global("home")), p.take(button("middle", "down"), 3))
        assertEquals(listOf(Act.Scroll(500, 600, 3)), p.take(Event("wheel", dy = 3), 4))
        assertEquals(emptyList<Act>(), p.take(Event("wheel", dx = 2), 5))
    }

    @Test
    fun `keys type, edit the field, and Esc is Back`() {
        val p = PointerPlan.Pointer()
        p.take(enter(), 0)
        assertEquals(listOf(Act.Text("hé")), p.take(Event("text", text = "hé"), 1))
        assertEquals(listOf(Act.Key("backspace")), p.take(Event("key", key = "backspace"), 2))
        assertEquals(listOf(Act.Key("left")), p.take(Event("key", key = "left"), 3))
        assertEquals(listOf(Act.Global("back")), p.take(Event("key", key = "escape"), 4))
        assertEquals(emptyList<Act>(), p.take(Event("key", key = "f13"), 5))
        assertEquals(emptyList<Act>(), p.take(Event("laser"), 6))
    }

    @Test
    fun `leaving lifts a finger down and hides the pointer`() {
        val p = PointerPlan.Pointer()
        p.take(enter(), 0)
        p.take(button("left", "down"), 1)
        assertEquals(listOf(Act.Touch("up", 100, 200), Act.Hide), p.take(Event("leave"), 2))
        assertFalse(p.on)
        // Nothing more to let go of.
        assertEquals(emptyList<Act>(), p.take(Event("leave"), 3))
        assertEquals(emptyList<Act>(), p.release())
    }

    @Test
    fun `a silent computer is let go of, and an idle pointer hides`() {
        val p = PointerPlan.Pointer()
        p.take(enter(), 0)
        // Idle a while, the computer still pinging: the pointer hides, the
        // visit stays, and it shows again with the next move.
        p.take(Event("ping"), PointerPlan.IDLE_HIDE_MS - 100)
        assertEquals(listOf(Act.Hide), p.tick(PointerPlan.IDLE_HIDE_MS))
        assertTrue(p.on)
        assertEquals(emptyList<Act>(), p.tick(PointerPlan.IDLE_HIDE_MS + 10))
        assertEquals(
            listOf(Act.Pointer(1, 1)),
            p.take(pointer(1, 1), PointerPlan.IDLE_HIDE_MS + 20),
        )
        // A finger held keeps the pointer up, however idle.
        p.take(button("left", "down"), 10_000)
        p.take(Event("ping"), 10_000 + PointerPlan.IDLE_HIDE_MS)
        assertEquals(emptyList<Act>(), p.tick(10_000 + PointerPlan.IDLE_HIDE_MS + 1))
        // Nothing for a second: lifted and hidden.
        val last = 10_000 + PointerPlan.IDLE_HIDE_MS
        assertEquals(emptyList<Act>(), p.tick(last + PointerPlan.SILENCE_MS - 1))
        assertEquals(listOf(Act.Touch("up", 1, 1), Act.Hide), p.tick(last + PointerPlan.SILENCE_MS))
        assertFalse(p.on)
    }

    @Test
    fun `a pointer event with no enter before it still comes on`() {
        val p = PointerPlan.Pointer()
        assertEquals(listOf(Act.Pointer(5, 6)), p.take(pointer(5, 6), 0))
        assertTrue(p.on)
        // The switch going off lets go too.
        p.take(button("left", "down"), 1)
        assertEquals(listOf(Act.Touch("up", 5, 6), Act.Hide), p.release())
    }

    /**
     * The review's I1: a drag at the pump's most (a batch every 8 ms, one
     * move each) for 3 s is never held back, the finger nor the pointer.
     */
    @Test
    fun `a drag at 125 moves a second is never held back`() {
        val p = PointerPlan.Pointer()
        val pacer = PointerPlan.Pacer()
        pacer.admit(p.take(enter(0, 0), 0), 0)
        pacer.admit(p.take(button("left", "down"), 1), 1)
        var dropped = 0
        for (i in 1..375) {
            val now = i * 8L
            val acts = p.take(pointer(i, i), now)
            if (pacer.admit(acts, now) != acts) dropped++
        }
        assertEquals(0, dropped)
        // A press or release is never held back, however busy.
        val up = p.take(button("left", "up"), 3_001)
        assertEquals(up, pacer.admit(up, 3_001))
    }

    /** The review's M6: said again at each new link, not only on a change or every 15 s. */
    @Test
    fun `the screen is said on a change, a new computer, a new link, and every 15 s`() {
        val due = { json: String, said: String?, computer: Boolean, link: Boolean, on: Boolean, since: Long ->
            PointerPlan.reportDue(json, said, computer, link, on, since)
        }
        assertFalse(due("a", "a", false, false, true, 1_000))
        assertTrue("changed", due("b", "a", false, false, true, 1_000))
        assertTrue("never said", due("a", null, false, false, false, 0))
        assertTrue("another computer", due("a", "a", true, false, true, 1_000))
        assertTrue("a new link", due("a", "a", false, true, true, 1_000))
        assertTrue("15 s on", due("a", "a", false, false, true, PointerPlan.REPORT_EVERY_MS))
        assertFalse("off: said once", due("a", "a", false, false, false, PointerPlan.REPORT_EVERY_MS))
    }

    @Test
    fun `delete takes the character after the cursor, or the selection`() {
        assertEquals(ControlPlan.Edited("ac", 1), PointerPlan.deleteForward("abc", 1, 1))
        assertEquals(ControlPlan.Edited("abc", 3), PointerPlan.deleteForward("abc", 3, 3))
        assertEquals(ControlPlan.Edited("a", 1), PointerPlan.deleteForward("abc", 1, 3))
        // A surrogate pair is one character.
        assertEquals(ControlPlan.Edited("a", 1), PointerPlan.deleteForward("a😀", 1, 1))
        // No selection known: at the end, nothing to take.
        assertEquals(ControlPlan.Edited("abc", 3), PointerPlan.deleteForward("abc", -1, -1))
    }

    @Test
    fun `the arrows move the cursor a character, collapsing a selection`() {
        assertEquals(1, PointerPlan.caret("abc", 2, 2, -1))
        assertEquals(3, PointerPlan.caret("abc", 2, 2, 1))
        assertEquals(0, PointerPlan.caret("abc", 0, 0, -1))
        assertEquals(3, PointerPlan.caret("abc", 3, 3, 1))
        // A selection collapses to the side the arrow points to.
        assertEquals(1, PointerPlan.caret("abcd", 1, 3, -1))
        assertEquals(3, PointerPlan.caret("abcd", 1, 3, 1))
        assertEquals(1, PointerPlan.caret("a😀", 3, 3, -1))
        assertEquals(3, PointerPlan.caret("abc", -1, -1, 1))
    }
}
