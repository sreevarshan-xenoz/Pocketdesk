package dev.pocketdesk.android.control

import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import android.view.accessibility.AccessibilityNodeInfo

/**
 * Hands-free: accepts Android's screen-sharing question once, for an open
 * the phone's own computer asked for, through the control service
 * ([ConsentRule] says what may be pressed). Made by [ControlService.armConsent]
 * right as Pocketdesk launches the question, lives at most
 * [ConsentRule.APPEAR_MS] + [ConsentRule.STEPS_MS], and ends at its first
 * press, so it can never act when no ask is waiting.
 *
 * It looks at the window in front every [ConsentRule.POLL_MS], and only
 * when that window belongs to [packages]; it presses by the node's own
 * click action, on a view found by its id or its words, never by a
 * coordinate. What it sees is compared and dropped: nothing of it is kept
 * or logged but the outcome.
 */
internal class ConsentPress(
    private val service: ControlService,
    private val label: String,
    private val packages: Set<String>,
    /** Told once, on the main thread: [ConsentRule.PRESSED] or [ConsentRule.NOT_FOUND]. */
    private val done: (String) -> Unit,
) {
    private val main = Handler(Looper.getMainLooper())
    private val armedAt = SystemClock.elapsedRealtime()
    private var seenAt: Long? = null
    private var opens = 0
    private var looks = 0
    private var finished = false

    private val tick = object : Runnable {
        override fun run() {
            if (finished) return
            val now = SystemClock.elapsedRealtime()
            if (!ConsentRule.live(armedAt, seenAt, now)) return end(ConsentRule.NOT_FOUND)
            looks++
            runCatching { look(now) }.onFailure { Log.w(TAG, "a look at the window failed") }
            if (!finished) main.postDelayed(this, ConsentRule.POLL_MS)
        }
    }

    fun start() {
        main.post(tick)
    }

    /** End it without a result (the cast came up by itself, or the service is going). */
    fun cancel() {
        if (finished) return
        finished = true
        main.removeCallbacks(tick)
    }

    private fun end(result: String) {
        if (finished) return
        finished = true
        main.removeCallbacks(tick)
        Log.i(TAG, "screen-sharing question: $result after $looks looks")
        done(result)
    }

    /** One view of the window, kept with the real node so a click can reach it. */
    private class Seen(val view: ConsentRule.Node, val node: AccessibilityNodeInfo)

    private fun look(now: Long) {
        val root = service.rootInActiveWindow ?: return
        // Only the system's own window, never another app's or Pocketdesk's.
        if (root.packageName?.toString() !in packages) return
        val seen = flatten(root)
        val nodes = seen.map { it.view }
        val texts = nodes.flatMap { listOfNotNull(it.text, it.description) }
        val names = ConsentRule.namesApp(texts, label)
        if (names && seenAt == null) seenAt = now
        when (val step = ConsentRule.next(nodes, names, seenAt != null, opens)) {
            ConsentRule.Step.Wait -> {}
            is ConsentRule.Step.OpenChooser -> if (click(seen[step.index].node)) opens++
            is ConsentRule.Step.PickEntire -> click(seen[step.index].node)
            is ConsentRule.Step.Confirm -> if (click(seen[step.index].node)) end(ConsentRule.PRESSED)
        }
    }

    /** The node's own click, or its nearest clickable parent's (three up at most). */
    private fun click(node: AccessibilityNodeInfo): Boolean {
        var at: AccessibilityNodeInfo? = node
        var up = 0
        while (at != null && up <= PARENTS) {
            if (at.isClickable) return at.performAction(AccessibilityNodeInfo.ACTION_CLICK)
            at = at.parent
            up++
        }
        return false
    }

    /** The window's views, breadth first, at most [ConsentRule.MAX_NODES]. */
    private fun flatten(root: AccessibilityNodeInfo): List<Seen> {
        val out = ArrayList<Seen>()
        val queue = ArrayDeque<AccessibilityNodeInfo>()
        queue.add(root)
        while (queue.isNotEmpty() && out.size < ConsentRule.MAX_NODES) {
            val node = queue.removeFirst()
            val className = node.className?.toString()
            // A chooser shows its selected row as a child: read it as the chooser's words.
            val text = node.text?.toString()
                ?: if (className?.contains("Spinner") == true) childTexts(node) else null
            out.add(
                Seen(
                    ConsentRule.Node(
                        id = node.viewIdResourceName,
                        text = text,
                        description = node.contentDescription?.toString(),
                        className = className,
                        clickable = clickableWithin(node),
                    ),
                    node,
                ),
            )
            for (i in 0 until node.childCount) node.getChild(i)?.let { queue.add(it) }
        }
        return out
    }

    private fun childTexts(node: AccessibilityNodeInfo): String? {
        val words = ArrayList<String>()
        for (i in 0 until node.childCount) node.getChild(i)?.text?.toString()?.let { words.add(it) }
        return words.takeIf { it.isNotEmpty() }?.joinToString(" ")
    }

    private fun clickableWithin(node: AccessibilityNodeInfo): Boolean {
        var at: AccessibilityNodeInfo? = node
        var up = 0
        while (at != null && up <= PARENTS) {
            if (at.isClickable) return true
            at = at.parent
            up++
        }
        return false
    }

    private companion object {
        const val TAG = "ConsentPress"
        const val PARENTS = 3
    }
}
