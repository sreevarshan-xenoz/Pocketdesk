package dev.linuxlink.android.stream

import java.util.concurrent.atomic.AtomicBoolean

/**
 * Whether a decode loop may run. Once stopped it stays stopped: a stream
 * stopped while it was still connecting (a screen switch, say) must not
 * have its loop start afterwards and feed a view that is gone.
 */
class DecodeGate {
    private val stopped = AtomicBoolean(false)
    private val running = AtomicBoolean(false)

    /** Start running, unless a stop came first. */
    fun open(): Boolean {
        if (stopped.get()) return false
        running.set(true)
        // A stop between the check and the set must still win.
        if (stopped.get()) {
            running.set(false)
            return false
        }
        return true
    }

    val isOpen: Boolean get() = running.get() && !stopped.get()

    fun stop() {
        stopped.set(true)
        running.set(false)
    }
}
