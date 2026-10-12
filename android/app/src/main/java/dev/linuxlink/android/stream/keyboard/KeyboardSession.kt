package dev.linuxlink.android.stream.keyboard

import android.content.Context
import android.util.Log
import android.view.inputmethod.InputMethodManager
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.launch

/**
 * One session's keyboard: the latch, the controller, the sink view the IME
 * types into, and one ordered worker that puts [KeyAction]s on the wire
 * (spec K10). [transport] blocks, so it only ever runs on that worker.
 */
class KeyboardSession(
    context: Context,
    scope: CoroutineScope,
    private val transport: (KeyAction) -> Result<Unit>,
) {
    private val imm = context.getSystemService(InputMethodManager::class.java)
    private val queue = Channel<List<KeyAction>>(Channel.UNLIMITED)
    private var sink: KeyboardSinkView? = null

    val latch = ModifierLatch()
    val controller = KeyboardController(latch, { queue.trySend(it) }) {
        sink?.let { imm.restartInput(it) }
    }

    init {
        scope.launch(Dispatchers.IO) {
            for (batch in queue) {
                for (action in batch) {
                    val sent = transport(action)
                    if (sent.isFailure) {
                        // The session's own error card reports a dead link;
                        // the rest of this batch would fail the same way.
                        Log.w(TAG, "key send failed: ${sent.exceptionOrNull()?.message}")
                        break
                    }
                }
            }
        }
    }

    fun attach(view: KeyboardSinkView) {
        sink = view
        view.keyboard = controller
    }

    /** Show the phone keyboard, typing into a fresh field. */
    fun open() {
        val view = sink ?: return
        controller.reset()
        view.isFocusable = true
        view.isFocusableInTouchMode = true
        view.requestFocus()
        imm.restartInput(view)
        imm.showSoftInput(view, 0)
    }

    fun close() {
        val view = sink ?: return
        imm.hideSoftInputFromWindow(view.windowToken, 0)
        view.clearFocus()
        view.isFocusable = false
        view.isFocusableInTouchMode = false
        latch.reset()
    }

    /** A key from the bar, with the latched modifiers. */
    fun key(keyCode: Int) = controller.key(keyCode)

    /** Send one combo as it is, ignoring the latch (Ctrl+Alt+Del). */
    fun combo(keyCode: Int, vararg modifiers: KeyModifier) {
        queue.trySend(listOf(KeyAction.Combo(modifiers.map { it.keyCode }, keyCode)))
    }

    private companion object {
        const val TAG = "Keyboard"
    }
}
