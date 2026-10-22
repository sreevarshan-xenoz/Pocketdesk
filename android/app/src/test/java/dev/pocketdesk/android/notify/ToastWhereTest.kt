package dev.pocketdesk.android.notify

import dev.pocketdesk.android.notify.ToastWhere.Answer
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ToastWhereTest {
    private val desk = ToastWhere.key("192.0.2.10", 1716)
    private val laptop = ToastWhere.key("192.0.2.11", 1716)

    @Test
    fun anIdleComputerIsMarkedWhileAnotherIsInUse() {
        val answers = mapOf(desk to Answer(false, 1_000), laptop to Answer(true, 2_000))
        assertTrue(ToastWhere.marks(desk, answers, on = true))
        assertFalse("the one in use toasts", ToastWhere.marks(laptop, answers, on = true))
        assertFalse("the switch off", ToastWhere.marks(desk, answers, on = false))
    }

    @Test
    fun withNoComputerKnownInUseEveryOneToasts() {
        assertFalse(ToastWhere.marks(desk, mapOf(desk to Answer(false, 0), laptop to Answer(false, 0)), true))
        assertFalse("alone", ToastWhere.marks(desk, mapOf(desk to Answer(false, 0)), true))
        assertFalse("the other does not say", ToastWhere.marks(desk, mapOf(desk to Answer(false, 0), laptop to Answer(null, 0)), true))
    }

    @Test
    fun anOldOrSilentAnswerIsNeverCounted() {
        val stale = ToastWhere.FRESH_MS + 1
        assertFalse("the other's word is old", ToastWhere.marks(desk, mapOf(desk to Answer(false, 0), laptop to Answer(true, stale)), true))
        assertFalse("its own word is old", ToastWhere.marks(desk, mapOf(desk to Answer(false, stale), laptop to Answer(true, 0)), true))
        assertFalse("it does not say", ToastWhere.marks(desk, mapOf(desk to Answer(null, 0), laptop to Answer(true, 0)), true))
        assertFalse("never answered", ToastWhere.marks(desk, mapOf(laptop to Answer(true, 0)), true))
    }

    @Test
    fun eachComputerIsAskedEveryFifteenSecondsWhileOn() {
        assertTrue(ToastWhere.askDue(true, ToastWhere.ASK_EVERY_MS))
        assertFalse(ToastWhere.askDue(true, ToastWhere.ASK_EVERY_MS - 1))
        assertFalse(ToastWhere.askDue(false, Long.MAX_VALUE))
    }
}
