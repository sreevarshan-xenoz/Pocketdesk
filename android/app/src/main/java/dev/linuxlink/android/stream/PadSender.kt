package dev.linuxlink.android.stream

import dev.linuxlink.android.bridge.RustCore

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
