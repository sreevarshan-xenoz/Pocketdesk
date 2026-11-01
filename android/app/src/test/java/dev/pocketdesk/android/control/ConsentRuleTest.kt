package dev.pocketdesk.android.control

import dev.pocketdesk.android.control.ConsentRule.Node
import dev.pocketdesk.android.control.ConsentRule.Step
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ConsentRuleTest {
    private fun node(
        text: String? = null,
        id: String? = null,
        cls: String = "android.widget.TextView",
        clickable: Boolean = false,
        description: String? = null,
    ) = Node(id, text, description, cls, clickable)

    private val message = node("Pocketdesk will start capturing everything that is displayed on your screen")
    private fun start(text: String = "Start now", clickable: Boolean = true) =
        node(text, "android:id/button1", "android.widget.Button", clickable)
    private val cancel = node("Cancel", "android:id/button2", "android.widget.Button", true)

    // ---- when it may act at all

    @Test
    fun `it acts only with the opt-in, the apps switch, the own computer and the service`() {
        assertTrue(ConsentRule.mayArm(autoOn = true, appsOn = true, own = true, service = true))
        assertFalse(ConsentRule.mayArm(autoOn = false, appsOn = true, own = true, service = true))
        assertFalse(ConsentRule.mayArm(autoOn = true, appsOn = false, own = true, service = true))
        assertFalse(ConsentRule.mayArm(autoOn = true, appsOn = true, own = false, service = true))
        assertFalse(ConsentRule.mayArm(autoOn = true, appsOn = true, own = true, service = false))
    }

    @Test
    fun `only the system's own windows count, never Pocketdesk's`() {
        assertEquals(setOf("com.android.systemui"), ConsentRule.packages(null, "dev.pocketdesk.android"))
        assertEquals(setOf("com.android.systemui"), ConsentRule.packages("  ", "dev.pocketdesk.android"))
        assertEquals(
            setOf("com.android.systemui", "com.example.oem.projection"),
            ConsentRule.packages("com.example.oem.projection", "dev.pocketdesk.android"),
        )
        // The question resolved to this very app: it is not a window to press in.
        assertEquals(setOf("com.android.systemui"), ConsentRule.packages("dev.pocketdesk.android", "dev.pocketdesk.android"))
    }

    // ---- the five seconds

    @Test
    fun `the question has five seconds to show, then eight to be answered`() {
        assertTrue(ConsentRule.live(armedAt = 1_000, seenAt = null, now = 1_000))
        assertTrue(ConsentRule.live(armedAt = 1_000, seenAt = null, now = 1_000 + ConsentRule.APPEAR_MS))
        assertFalse(ConsentRule.live(armedAt = 1_000, seenAt = null, now = 1_001 + ConsentRule.APPEAR_MS))
        // Once seen, its own clock runs, not the first.
        assertTrue(ConsentRule.live(armedAt = 1_000, seenAt = 5_000, now = 5_000 + ConsentRule.STEPS_MS))
        assertFalse(ConsentRule.live(armedAt = 1_000, seenAt = 5_000, now = 5_001 + ConsentRule.STEPS_MS))
    }

    // ---- which window is the question

    @Test
    fun `the question names Pocketdesk, any case, and a short or blank name matches nothing`() {
        assertTrue(ConsentRule.namesApp(listOf("Start recording", "POCKETDESK will capture"), "Pocketdesk"))
        assertFalse(ConsentRule.namesApp(listOf("Another app will capture your screen"), "Pocketdesk"))
        assertFalse(ConsentRule.namesApp(listOf("anything at all"), ""))
        assertFalse(ConsentRule.namesApp(listOf("anything at all"), "  "))
        assertFalse(ConsentRule.namesApp(listOf("it is an app"), "ap"))
    }

    // ---- what to press

    @Test
    fun `with no chooser, the confirm button is pressed`() {
        val nodes = listOf(message, start(), cancel)
        assertEquals(Step.Confirm(1), ConsentRule.next(nodes, namesApp = true, seen = true, opens = 0))
        // Android 14 worded it Share screen.
        val shareScreen = listOf(message, start("Share screen"), cancel)
        assertEquals(Step.Confirm(1), ConsentRule.next(shareScreen, namesApp = true, seen = true, opens = 0))
    }

    @Test
    fun `a window that does not name Pocketdesk is never pressed`() {
        val other = listOf(node("Another app will capture your screen"), start(), cancel)
        assertEquals(Step.Wait, ConsentRule.next(other, namesApp = false, seen = false, opens = 0))
        // Not even after the question was seen: only the chooser's own row is allowed then.
        assertEquals(Step.Wait, ConsentRule.next(other, namesApp = false, seen = true, opens = 0))
    }

    @Test
    fun `a button that cannot be clicked, or is not offered, is waited for`() {
        assertEquals(Step.Wait, ConsentRule.next(listOf(message, start(clickable = false), cancel), true, true, 0))
        assertEquals(Step.Wait, ConsentRule.next(listOf(message), true, true, 0))
        assertEquals(Step.Wait, ConsentRule.next(emptyList(), true, true, 0))
    }

    @Test
    fun `next and cancel are never the confirm button`() {
        assertEquals(Step.Wait, ConsentRule.next(listOf(message, start("Next"), cancel), true, true, 0))
        // Only the cancel button of a window is never taken for it either.
        assertEquals(Step.Wait, ConsentRule.next(listOf(message, cancel), true, true, 0))
        // A button found by its words alone, with no id, in the system's wording.
        val byWords = listOf(message, node("Start", null, "android.widget.Button", true))
        assertEquals(Step.Confirm(1), ConsentRule.next(byWords, true, true, 0))
        // A plain label that happens to say Start, in no button, is not one.
        val label = listOf(message, node("Start", null, "android.widget.TextView", true))
        assertEquals(Step.Wait, ConsentRule.next(label, true, true, 0))
    }

    @Test
    fun `with a chooser, entire screen is chosen before anything is confirmed`() {
        val spinner = "android.widget.Spinner"
        val one = node("A single app", "com.android.systemui:id/screen_share_mode_options", spinner, true)
        val entire = node("Share entire screen", "com.android.systemui:id/screen_share_mode_options", spinner, true)
        // Saying one app: open it, twice at most, never confirm.
        assertEquals(Step.OpenChooser(1), ConsentRule.next(listOf(message, one, start("Next"), cancel), true, true, 0))
        assertEquals(Step.OpenChooser(1), ConsentRule.next(listOf(message, one, start(), cancel), true, true, 1))
        assertEquals(Step.Wait, ConsentRule.next(listOf(message, one, start(), cancel), true, true, ConsentRule.MAX_CHOOSER_OPENS))
        // Saying entire screen: confirm.
        assertEquals(Step.Confirm(2), ConsentRule.next(listOf(message, entire, start("Share screen"), cancel), true, true, 0))
        // Saying something this does not read (another language): nothing is pressed.
        val unknown = node("Una sola app o toda", "com.android.systemui:id/screen_share_mode_options", spinner, true)
        assertEquals(Step.Wait, ConsentRule.next(listOf(message, unknown, start(), cancel), true, true, 0))
        // A chooser that cannot be clicked is not opened.
        val stuck = node("A single app", null, spinner, false)
        assertEquals(Step.Wait, ConsentRule.next(listOf(message, stuck, start(), cancel), true, true, 0))
    }

    @Test
    fun `the chooser's drop-down has only its entire screen row pressed, and only after the question was seen`() {
        val row = node("Share entire screen", null, "android.widget.CheckedTextView", true)
        val oneApp = node("Share one app", null, "android.widget.CheckedTextView", true)
        val rows = listOf(oneApp, row)
        assertEquals(Step.PickEntire(1), ConsentRule.next(rows, namesApp = false, seen = true, opens = 1))
        // Before the question was ever seen, no window is pressed.
        assertEquals(Step.Wait, ConsentRule.next(rows, namesApp = false, seen = false, opens = 0))
        // A row that cannot be clicked, or a window with no such row, is waited on.
        assertEquals(Step.Wait, ConsentRule.next(listOf(oneApp, node("Share entire screen")), false, true, 1))
        assertEquals(Step.Wait, ConsentRule.next(listOf(oneApp), false, true, 1))
        // The confirm button of a window that does not name Pocketdesk is never taken here.
        assertEquals(Step.Wait, ConsentRule.next(listOf(start(), cancel), false, true, 1))
    }

    @Test
    fun `its outcomes are three plain words`() {
        assertEquals(listOf("pressed", "not_found", "not_allowed"), listOf(ConsentRule.PRESSED, ConsentRule.NOT_FOUND, ConsentRule.NOT_ALLOWED))
    }
}
