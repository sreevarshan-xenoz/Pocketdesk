package dev.linuxlink.android.stream

import dev.linuxlink.android.bridge.RustCore
import kotlinx.coroutines.launch

/**
 * Put a [TrackpadGesture]'s outputs on the wire through [send], the caller's
 * ordered worker (the bridge calls block). Shared by the in-stream trackpad
 * and the foldable dock.
 */
fun sendPadOutputs(address: String, port: Int, outputs: List<PadOutput>, send: (() -> Unit) -> Unit) {
    for (out in outputs) {
        when (out) {
            is PadOutput.Move -> send {
                RustCore.sendMouseEvent(address, port, out.dx.toFloat(), out.dy.toFloat(), 0, false)
            }
            is PadOutput.Scroll -> send {
                RustCore.sendMouseEvent(address, port, out.dx.toFloat(), out.dy.toFloat(), 2, false)
            }
            is PadOutput.Click -> send {
                RustCore.sendMouseClick(out.button, true)
                RustCore.sendMouseClick(out.button, false)
            }
            is PadOutput.Button -> send { RustCore.sendMouseClick(out.button, out.pressed) }
        }
    }
}

/**
 * One ordered dispatcher for every pad send, so a release queued at teardown
 * lands after the moves already queued before it.
 */
val padOrdered: kotlinx.coroutines.CoroutineDispatcher = kotlinx.coroutines.Dispatchers.IO.limitedParallelism(1)

/** Outlives any composition: teardown must still be able to release a button. */
private val padScope = kotlinx.coroutines.CoroutineScope(kotlinx.coroutines.SupervisorJob() + padOrdered)

/** Release whatever [pad] holds, from a view that is going away (cancelled coroutines cannot send). */
fun cancelPad(address: String, port: Int, pad: TrackpadGesture) {
    sendPadOutputs(address, port, pad.cancel()) { block -> padScope.launch { block() } }
}

/**
 * After a gesture ends, send the held-back one-finger click when its window
 * closes ([TrackpadGesture.expire]). Runs on this scope's thread, the one the
 * engine is used from.
 */
fun kotlinx.coroutines.CoroutineScope.scheduleHeldClick(pad: TrackpadGesture, send: (List<PadOutput>) -> Unit) {
    val deadline = pad.pendingDeadline() ?: return
    launch {
        kotlinx.coroutines.delay((deadline - android.os.SystemClock.uptimeMillis()).coerceAtLeast(0))
        send(pad.expire(android.os.SystemClock.uptimeMillis()))
    }
}
