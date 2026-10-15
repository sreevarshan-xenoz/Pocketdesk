package dev.pocketdesk.android.stream

import android.content.Context
import android.hardware.input.InputManager
import android.os.Handler
import android.os.Looper
import android.view.InputDevice
import android.view.MotionEvent
import android.view.View
import android.view.ViewTreeObserver

/**
 * T9c: a mouse connected to the phone (USB or Bluetooth) drives the
 * desktop's cursor. While [enabled] says so, a mouse is present and the
 * window has focus, the session's video [view] captures the pointer, so
 * Android hands it raw relative motion instead of moving its own pointer;
 * [CapturedMouse] turns that into the trackpad's outputs. The touchscreen
 * keeps working, and Android releases the capture when the window loses
 * focus. Returns the function that undoes it all.
 */
fun attachMouseCapture(
    view: View,
    address: String,
    port: Int,
    enabled: () -> Boolean,
): () -> Unit {
    val context = view.context
    val inputs = context.getSystemService(InputManager::class.java)
    var mouse = CapturedMouse()

    fun send(outputs: List<PadOutput>) = sendPadOutputs(address, port, outputs) { launchPadSend(it) }

    fun update() {
        val want = enabled() && view.hasWindowFocus() && mousePresent(context)
        if (want && !view.hasPointerCapture()) {
            // Captured events go to the focused view.
            view.isFocusable = true
            view.isFocusableInTouchMode = true
            view.requestFocus()
            view.requestPointerCapture()
        } else if (!want && view.hasPointerCapture()) {
            view.releasePointerCapture()
            send(mouse.release())
        }
    }

    view.setOnCapturedPointerListener { _, event ->
        if (event.isFromSource(InputDevice.SOURCE_MOUSE_RELATIVE) || event.isFromSource(InputDevice.SOURCE_MOUSE)) {
            val scrolled = event.actionMasked == MotionEvent.ACTION_SCROLL
            send(
                mouse.event(
                    dx = if (scrolled) 0f else event.x,
                    dy = if (scrolled) 0f else event.y,
                    buttons = event.buttonState,
                    vscroll = event.getAxisValue(MotionEvent.AXIS_VSCROLL),
                    hscroll = event.getAxisValue(MotionEvent.AXIS_HSCROLL),
                ),
            )
            true
        } else {
            false
        }
    }

    val focus = ViewTreeObserver.OnWindowFocusChangeListener { hasFocus ->
        if (!hasFocus) {
            // Android has released the capture: let go of what was held.
            send(mouse.release())
        }
        update()
    }
    val devices = object : InputManager.InputDeviceListener {
        override fun onInputDeviceAdded(deviceId: Int) = update()
        override fun onInputDeviceRemoved(deviceId: Int) = update()
        override fun onInputDeviceChanged(deviceId: Int) = update()
    }
    view.viewTreeObserver.addOnWindowFocusChangeListener(focus)
    inputs?.registerInputDeviceListener(devices, Handler(Looper.getMainLooper()))
    view.post { update() }

    return {
        inputs?.unregisterInputDeviceListener(devices)
        if (view.viewTreeObserver.isAlive) view.viewTreeObserver.removeOnWindowFocusChangeListener(focus)
        view.setOnCapturedPointerListener(null)
        if (view.hasPointerCapture()) view.releasePointerCapture()
        send(mouse.release())
        mouse = CapturedMouse()
    }
}

/** Whether a real mouse (not a touchpad emulating one) is connected. */
private fun mousePresent(context: Context): Boolean {
    val inputs = context.getSystemService(InputManager::class.java) ?: return false
    return inputs.inputDeviceIds.any { id ->
        val device = inputs.getInputDevice(id) ?: return@any false
        !device.isVirtual && device.supportsSource(InputDevice.SOURCE_MOUSE)
    }
}
