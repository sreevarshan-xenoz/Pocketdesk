package dev.pocketdesk.android

import android.content.Context
import android.os.Build
import android.provider.Settings
import dev.pocketdesk.android.bridge.RustCore

/**
 * What this phone calls itself to the computers it connects to (U3): the
 * name set in Pocketdesk's Settings, else Android's device name (Settings ›
 * About phone), else the model. It had been a fixed "Pocketdesk Android
 * Client", so every phone looked the same on a computer.
 */
object PhoneName {
    /** Android's own name for this phone. */
    fun deviceName(context: Context): String =
        ComputerName.clean(Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME))
            ?: ComputerName.clean(Build.MODEL)
            ?: "Android phone"

    /** The name sent: the owner's choice, else [deviceName]. */
    fun current(context: Context): String =
        ComputerName.clean(Prefs.phoneName(context)) ?: deviceName(context)

    /** Tell the bridge, for the next connection's identity. */
    fun apply(context: Context) {
        RustCore.setDeviceName(current(context))
    }
}
