package dev.pocketdesk.android.stream

import android.content.Context
import android.hardware.input.InputManager
import android.os.Handler
import android.os.Looper
import android.view.InputDevice
import android.view.MotionEvent
import android.view.View
import android.view.ViewTreeObserver
import dev.pocketdesk.android.compat.Android17

/**
 * T9c: a mouse connected to the phone (USB or Bluetooth) drives the
 * desktop's cursor. While [enabled] says so, a mouse is present and the
 * window has focus, the session's video [view] captures the pointer, so
 * Android hands it raw relative motion instead of moving its own pointer;
 * [CapturedMouse] turns that into the trackpad's outputs. The touchscreen
 * keeps working, and Android releases the capture when the window loses
 * focus. Returns the function that undoes it all.
 *
 * Captured events go to whichever view has focus (the keyboard's sink takes
 * it while the on-screen keyboard is open), so the handler follows focus.
 */
fun attachMouseCapture(
    view: View,
    address: String,
    port: Int,
    enabled: () -> Boolean,
): () -> Unit {
    val context = view.context
    val inputs = context.getSystemService(InputManager::class.java)
    val mouse = CapturedMouse()
    val handlers = mutableSetOf<View>()
    var captured = false

    fun send(outputs: List<PadOutput>) = sendPadOutputs(address, port, outputs) { launchPadSend(it) }

    fun released() {
        captured = false
        CapturedMouse.captured = false
        send(mouse.release())
    }

    val onCaptured = View.OnCapturedPointerListener { _, event ->
        // Captured mouse events are relative; anything else is not ours.
        if (!event.isFromSource(InputDevice.SOURCE_MOUSE_RELATIVE)) return@OnCapturedPointerListener false
        val scrolled = event.actionMasked == MotionEvent.ACTION_SCROLL
        val history = 0 until event.historySize
        send(
            mouse.event(
                dx = if (scrolled) 0f else CapturedMouse.batched(history.map { event.getHistoricalX(it) }, event.x),
                dy = if (scrolled) 0f else CapturedMouse.batched(history.map { event.getHistoricalY(it) }, event.y),
                buttons = event.buttonState,
                vscroll = event.getAxisValue(MotionEvent.AXIS_VSCROLL),
                hscroll = event.getAxisValue(MotionEvent.AXIS_HSCROLL),
            ),
        )
        true
    }

    fun handle(target: View) {
        if (handlers.add(target)) target.setOnCapturedPointerListener(onCaptured)
    }

    fun update() {
        val want = enabled() && view.hasWindowFocus() && mousePresent(context)
        if (want && !captured) {
            // Captured events go to the focused view: start there.
            view.isFocusable = true
            view.isFocusableInTouchMode = true
            if (view.rootView.findFocus() == null) view.requestFocus()
            handle(view)
            view.rootView.findFocus()?.let { handle(it) }
            view.requestPointerCapture()
            captured = true
            CapturedMouse.captured = true
        } else if (!want && captured) {
            view.releasePointerCapture()
            released()
        }
    }

    val focus = ViewTreeObserver.OnWindowFocusChangeListener { hasFocus ->
        if (!hasFocus && captured) {
            // Android has released the capture: let go of what was held.
            released()
        }
        update()
    }
    // The keyboard's sink, or any view, may take focus: the mouse follows.
    val focusMoves = ViewTreeObserver.OnGlobalFocusChangeListener { _, newFocus ->
        if (captured && newFocus != null) handle(newFocus)
    }
    val devices = object : InputManager.InputDeviceListener {
        override fun onInputDeviceAdded(deviceId: Int) = update()
        override fun onInputDeviceRemoved(deviceId: Int) = update()
        override fun onInputDeviceChanged(deviceId: Int) = update()
    }
    // Leaving the session detaches the view before Compose disposes of it,
    // and a detached view can no longer release the window's capture.
    val attach = object : View.OnAttachStateChangeListener {
        override fun onViewAttachedToWindow(v: View) = Unit
        override fun onViewDetachedFromWindow(v: View) {
            if (captured) {
                v.releasePointerCapture()
                released()
            }
        }
    }
    view.viewTreeObserver.addOnWindowFocusChangeListener(focus)
    view.viewTreeObserver.addOnGlobalFocusChangeListener(focusMoves)
    view.addOnAttachStateChangeListener(attach)
    inputs?.registerInputDeviceListener(devices, Handler(Looper.getMainLooper()))
    view.post { update() }

    return {
        inputs?.unregisterInputDeviceListener(devices)
        view.removeOnAttachStateChangeListener(attach)
        if (view.viewTreeObserver.isAlive) {
            view.viewTreeObserver.removeOnWindowFocusChangeListener(focus)
            view.viewTreeObserver.removeOnGlobalFocusChangeListener(focusMoves)
        }
        handlers.forEach { it.setOnCapturedPointerListener(null) }
        handlers.clear()
        if (captured) {
            view.releasePointerCapture()
            released()
        }
    }
}

/**
 * Whether a real mouse is connected: not a virtual device; a touchpad only
 * from Android 17, where it reports like a captured mouse (N4).
 */
private fun mousePresent(context: Context): Boolean {
    val inputs = context.getSystemService(InputManager::class.java) ?: return false
    return inputs.inputDeviceIds.any { id ->
        val device = inputs.getInputDevice(id) ?: return@any false
        Android17.captures(
            sdk = android.os.Build.VERSION.SDK_INT,
            virtual = device.isVirtual,
            mouse = device.supportsSource(InputDevice.SOURCE_MOUSE),
            touchpad = device.supportsSource(InputDevice.SOURCE_TOUCHPAD),
        )
    }
}
