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

    private val pkg = "com.android.systemui"
    private val cls = "com.android.systemui.mediaprojection.permission.MediaProjectionPermissionActivity"
    private val own = "dev.pocketdesk.android"

    // ---- when it may act at all

    @Test
    fun `it acts only with the opt-in, the apps switch, the own computer and the service`() {
        assertTrue(ConsentRule.mayArm(autoOn = true, appsOn = true, own = true, service = true))
        assertFalse(ConsentRule.mayArm(autoOn = false, appsOn = true, own = true, service = true))
        assertFalse(ConsentRule.mayArm(autoOn = true, appsOn = false, own = true, service = true))
        assertFalse(ConsentRule.mayArm(autoOn = true, appsOn = true, own = false, service = true))
        assertFalse(ConsentRule.mayArm(autoOn = true, appsOn = true, own = true, service = false))
    }

    // ---- which window is the question: the activity Pocketdesk's intent resolved to, exactly

    @Test
    fun `the question's window is the resolved activity's package and class, exactly`() {
        assertTrue(ConsentRule.isQuestionWindow(pkg, cls, pkg, cls, own))
        // Another window of the same package (a notification shade, a volume panel, another dialog).
        assertFalse(ConsentRule.isQuestionWindow(pkg, "com.android.systemui.volume.VolumeDialog", pkg, cls, own))
        assertFalse(ConsentRule.isQuestionWindow(pkg, "android.app.AlertDialog", pkg, cls, own))
        // A class that only starts like it, or a package that only starts like it.
        assertFalse(ConsentRule.isQuestionWindow(pkg, cls + "2", pkg, cls, own))
        assertFalse(ConsentRule.isQuestionWindow(pkg + ".x", cls, pkg, cls, own))
        // Another app with the same class name.
        assertFalse(ConsentRule.isQuestionWindow("com.example.other", cls, pkg, cls, own))
        // Nothing known on either side never matches.
        assertFalse(ConsentRule.isQuestionWindow(null, cls, pkg, cls, own))
        assertFalse(ConsentRule.isQuestionWindow(pkg, null, pkg, cls, own))
        assertFalse(ConsentRule.isQuestionWindow(pkg, cls, null, cls, own))
        assertFalse(ConsentRule.isQuestionWindow(pkg, cls, pkg, null, own))
        assertFalse(ConsentRule.isQuestionWindow(pkg, cls, "", cls, own))
        assertFalse(ConsentRule.isQuestionWindow(pkg, cls, pkg, " ", own))
        // Never one of Pocketdesk's own windows.
        assertFalse(ConsentRule.isQuestionWindow(own, cls, own, cls, own))
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

    // ---- the label, as a whole word

    @Test
    fun `the question names Pocketdesk as a whole word or phrase, not inside another`() {
        assertTrue(ConsentRule.namesApp(listOf("Start recording", "Pocketdesk will capture"), "Pocketdesk"))
        assertTrue(ConsentRule.namesApp(listOf("Share your screen with Pocketdesk?"), "Pocketdesk"))
        assertTrue(ConsentRule.namesApp(listOf("\"Pocketdesk\" will capture"), "Pocketdesk"))
        // Inside a longer word or name: not it.
        assertFalse(ConsentRule.namesApp(listOf("Pocketdesk2 will capture your screen"), "Pocketdesk"))
        assertFalse(ConsentRule.namesApp(listOf("NotPocketdesk will capture your screen"), "Pocketdesk"))
        assertFalse(ConsentRule.namesApp(listOf("Pocketdeskpro will capture your screen"), "Pocketdesk"))
        // The exact label, as written: another case is another name.
        assertFalse(ConsentRule.namesApp(listOf("POCKETDESK will capture"), "Pocketdesk"))
        // A label with punctuation is matched literally.
        assertTrue(ConsentRule.namesApp(listOf("Pocket.desk will capture"), "Pocket.desk"))
        assertFalse(ConsentRule.namesApp(listOf("PocketXdesk will capture"), "Pocket.desk"))
        assertFalse(ConsentRule.namesApp(listOf("Another app will capture your screen"), "Pocketdesk"))
        // A short or blank label matches nothing.
        assertFalse(ConsentRule.namesApp(listOf("anything at all"), ""))
        assertFalse(ConsentRule.namesApp(listOf("anything at all"), "  "))
        assertFalse(ConsentRule.namesApp(listOf("it is an app"), "ap"))
    }

    // ---- what to press, in the question's window

    @Test
    fun `with no chooser, the confirm button is pressed when it says a known confirm word`() {
        assertEquals(Step.Confirm(1), ConsentRule.next(listOf(message, start(), cancel)))
        assertEquals(Step.Confirm(1), ConsentRule.next(listOf(message, start("Share screen"), cancel)))
        // Another language of the phone, and the case the system wrote it in.
        assertEquals(Step.Confirm(1), ConsentRule.next(listOf(message, start("Iniciar ahora"), cancel)))
        assertEquals(Step.Confirm(1), ConsentRule.next(listOf(message, start("START NOW"), cancel)))
    }

    @Test
    fun `a button that says anything unknown is never pressed`() {
        // An unknown language, a made-up word, "Next" (which goes on to choose an app), and empty.
        for (text in listOf("Zxqv", "Next", "Siguiente", "Cancel", "Delete everything", "", "Start now please")) {
            assertEquals(text, Step.Wait, ConsentRule.next(listOf(message, start(text), cancel)))
        }
        // A button with no words at all.
        assertEquals(Step.Wait, ConsentRule.next(listOf(message, node(null, "android:id/button1", "android.widget.Button", true))))
    }

    @Test
    fun `another dialog's button1 is not pressed when its words are not a confirm word`() {
        val other = listOf(node("Allow Another app to read your photos?"), start("Delete"), cancel)
        assertEquals(Step.Wait, ConsentRule.next(other))
    }

    @Test
    fun `a button that cannot be clicked, or is not offered, is waited for`() {
        assertEquals(Step.Wait, ConsentRule.next(listOf(message, start(clickable = false), cancel)))
        assertEquals(Step.Wait, ConsentRule.next(listOf(message)))
        assertEquals(Step.Wait, ConsentRule.next(emptyList()))
        // The cancel button alone is never taken for it.
        assertEquals(Step.Wait, ConsentRule.next(listOf(message, cancel)))
    }

    @Test
    fun `a view that is not a button is never pressed, whatever it says`() {
        val label = listOf(message, node("Start", null, "android.widget.TextView", true))
        assertEquals(Step.Wait, ConsentRule.next(label))
        val byWords = listOf(message, node("Start", null, "android.widget.Button", true))
        assertEquals(Step.Confirm(1), ConsentRule.next(byWords))
    }

    @Test
    fun `with a chooser, only entire screen is confirmed, and no row is ever opened or picked`() {
        val spinner = "android.widget.Spinner"
        val sid = "com.android.systemui:id/screen_share_mode_options"
        val one = node("A single app", sid, spinner, true)
        val entire = node("Share entire screen", sid, spinner, true)
        // Saying one app: nothing is pressed (no chooser is opened and no row picked).
        assertEquals(Step.Wait, ConsentRule.next(listOf(message, one, start("Next"), cancel)))
        assertEquals(Step.Wait, ConsentRule.next(listOf(message, one, start(), cancel)))
        // Saying entire screen: confirm.
        assertEquals(Step.Confirm(2), ConsentRule.next(listOf(message, entire, start("Share screen"), cancel)))
        // Saying something this does not read: nothing is pressed.
        val unknown = node("Zxqv Qwerty", sid, spinner, true)
        assertEquals(Step.Wait, ConsentRule.next(listOf(message, unknown, start(), cancel)))
        // A chooser with no words at all (an OEM layout nothing is read from): nothing is pressed.
        val bare = node(null, sid, spinner, true)
        assertEquals(Step.Wait, ConsentRule.next(listOf(message, bare, start(), cancel)))
    }

    @Test
    fun `its outcomes are three plain words`() {
        assertEquals(
            listOf("pressed", "not_found", "not_allowed"),
            listOf(ConsentRule.PRESSED, ConsentRule.NOT_FOUND, ConsentRule.NOT_ALLOWED),
        )
    }
}
