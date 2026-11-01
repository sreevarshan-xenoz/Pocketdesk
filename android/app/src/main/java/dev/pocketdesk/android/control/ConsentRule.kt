package dev.pocketdesk.android.control

/**
 * Hands-free: accepting Android's own screen-sharing question for the
 * phone's own computer, when the owner turned that on ("Accept screen
 * sharing for my computers", off by default). Pure, for JVM tests;
 * [ConsentPress] does it through the control service.
 *
 * What it may press is narrow on purpose. Only while Pocketdesk itself has
 * just launched the question for an open its own computer asked (and for
 * [APPEAR_MS] after), only in a window of the system's own screen-capture
 * question that names Pocketdesk, only its one confirm button, and, where
 * Android asks one app or the entire screen, only after "entire screen" is
 * chosen. Nothing in any other window or app is ever pressed, and when no
 * ask is waiting nothing is.
 */
object ConsentRule {
    /** The system's UI: where Android's screen-capture question lives on stock Android. */
    const val SYSTEM_UI = "com.android.systemui"

    /** How long after Pocketdesk launches the question it may show up. */
    const val APPEAR_MS = 5_000L

    /** How long, once it has shown, its choices and the confirm may take. */
    const val STEPS_MS = 8_000L

    /** How often the window is looked at. */
    const val POLL_MS = 250L

    /** The most views looked at in a window. */
    const val MAX_NODES = 150

    /** The chooser is opened this often at most. */
    const val MAX_CHOOSER_OPENS = 2

    /** What became of an arming. */
    const val PRESSED = "pressed"
    const val NOT_FOUND = "not_found"
    const val NOT_ALLOWED = "not_allowed"

    /**
     * Whether the question may be accepted for an open: the owner's opt-in
     * is on ([autoOn]), so is "Let the computer open my apps" ([appsOn]),
     * the computer that asked is the phone's own ([own]) and the control
     * service is bound ([service]).
     */
    fun mayArm(autoOn: Boolean, appsOn: Boolean, own: Boolean, service: Boolean): Boolean =
        autoOn && appsOn && own && service

    /**
     * The packages whose window may be the question: the system's UI, and
     * the app Android resolved the question to ([resolved], for makers who
     * moved it), never Pocketdesk itself ([own]).
     */
    fun packages(resolved: String?, own: String): Set<String> =
        setOfNotNull(SYSTEM_UI, resolved?.takeIf { it.isNotBlank() }).filter { it != own }.toSet()

    /**
     * Whether the window is live at [now]: armed at [armedAt] and, until
     * the question has been seen ([seenAt] null), within [APPEAR_MS]; after
     * it, within [STEPS_MS] of its first sight.
     */
    fun live(armedAt: Long, seenAt: Long?, now: Long): Boolean =
        if (seenAt == null) now - armedAt <= APPEAR_MS else now - seenAt <= STEPS_MS

    /** Whether any of [texts] names the app ([label]), any case. */
    fun namesApp(texts: List<String>, label: String): Boolean {
        val name = label.trim()
        if (name.length < 3) return false
        return texts.any { it.contains(name, ignoreCase = true) }
    }

    /** A view of the window as the control service saw it. */
    data class Node(
        val id: String?,
        val text: String?,
        val description: String?,
        val className: String?,
        /** It, or a parent within three, takes a click. */
        val clickable: Boolean,
    )

    /** What to do next. */
    sealed class Step {
        /** Nothing yet: look again. */
        object Wait : Step()

        /** Open the one-app-or-entire-screen chooser (the node at [index]). */
        data class OpenChooser(val index: Int) : Step()

        /** Choose "entire screen" (the node at [index]). */
        data class PickEntire(val index: Int) : Step()

        /** Press the question's confirm button (the node at [index]). */
        data class Confirm(val index: Int) : Step()
    }

    private val ENTIRE = listOf("entire screen", "whole screen", "full screen")
    private val SINGLE = listOf("single app", "one app", "an app")

    /** Button words that never confirm: "Next" goes on to choose an app. */
    private val NOT_CONFIRM = listOf("next", "cancel", "cancelar", "no thanks", "not now")

    /** Words on the confirm button of the question as stock Android words it. */
    private val CONFIRM = listOf("start now", "start", "share screen", "share", "allow", "start recording", "start casting")

    private fun words(node: Node): String =
        listOfNotNull(node.text, node.description).joinToString(" ").trim().lowercase()

    private fun isChooser(node: Node): Boolean =
        node.className?.contains("Spinner") == true || node.id?.contains("screen_share_mode", ignoreCase = true) == true

    private fun mentions(node: Node, phrases: List<String>): Boolean =
        words(node).let { w -> phrases.any { w.contains(it) } }

    private fun isConfirm(node: Node): Boolean {
        val w = words(node)
        if (NOT_CONFIRM.any { w == it }) return false
        val byId = node.id == "android:id/button1" || node.id?.endsWith(":id/button1") == true ||
            node.id?.endsWith(":id/button_start") == true
        return byId || (node.className?.contains("Button") == true && CONFIRM.any { w == it })
    }

    /**
     * What to do next in the window [nodes] ([namesApp]: it is the question,
     * naming Pocketdesk; false for the chooser's drop-down, a window of its
     * own, which names nothing, but only once the question [seen]), having
     * opened the chooser [opens] times.
     *
     * With a chooser, "entire screen" is chosen before the confirm button is
     * pressed, and the button is never pressed while the chooser says one
     * app or says something unreadable (a language this does not know).
     * Without one, the confirm button is pressed. Anything it cannot find,
     * it waits for: the window ends, and the one-tap notification takes over.
     */
    fun next(nodes: List<Node>, namesApp: Boolean, seen: Boolean, opens: Int): Step {
        val clickable = nodes.withIndex().filter { it.value.clickable }
        if (!namesApp) {
            // The drop-down of the chooser, after the question was seen: only its "entire screen" row.
            if (!seen) return Step.Wait
            val row = clickable.firstOrNull { !isChooser(it.value) && mentions(it.value, ENTIRE) }
            return if (row != null) Step.PickEntire(row.index) else Step.Wait
        }
        val chooser = nodes.withIndex().firstOrNull { isChooser(it.value) }
        if (chooser != null) {
            val value = chooser.value
            return when {
                mentions(value, ENTIRE) -> confirm(clickable)
                mentions(value, SINGLE) && value.clickable && opens < MAX_CHOOSER_OPENS -> Step.OpenChooser(chooser.index)
                else -> Step.Wait
            }
        }
        // A drop-down row may sit in the question's own window on some makers' screens.
        return confirm(clickable)
    }

    private fun confirm(clickable: List<IndexedValue<Node>>): Step {
        val button = clickable.firstOrNull { isConfirm(it.value) } ?: return Step.Wait
        return Step.Confirm(button.index)
    }
}
