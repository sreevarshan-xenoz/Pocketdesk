package dev.pocketdesk.android.bluetooth

import android.content.Context
import android.media.AudioDeviceInfo
import android.media.AudioManager
import android.os.Build

/**
 * Giving way to earbuds (the Bluetooth fallback): while the phone plays to
 * a Bluetooth headset or is in a call, the radio is the audio's, and no
 * transfer starts over the Bluetooth road
 * ([BluetoothRoadRule.transferMayStart]); the control link's small packets
 * still go.
 *
 * Read from [AudioManager] alone, with no permission: whether music plays
 * while a Bluetooth audio output (A2DP, LE audio, a hearing aid) is
 * connected, and whether the phone is in a call (the mode Android sets for
 * a phone call or a voice or video call in an app).
 */
object BluetoothAudio {
    /** Whether audio holds the radio: music to a Bluetooth headset ([a2dpPlaying]) or a call ([inCall]). */
    fun busy(a2dpPlaying: Boolean, inCall: Boolean): Boolean = a2dpPlaying || inCall

    /** Whether the phone's audio holds the radio now; false when it cannot be read. */
    fun busyNow(ctx: Context): Boolean {
        val audio = ctx.getSystemService(AudioManager::class.java) ?: return false
        val inCall = runCatching { inCall(audio.mode) }.getOrDefault(false)
        val playing = runCatching {
            audio.isMusicActive &&
                audio.getDevices(AudioManager.GET_DEVICES_OUTPUTS).any { it.type in bluetoothOutputs }
        }.getOrDefault(false)
        return busy(playing, inCall)
    }

    /** Whether [mode] is a call: a phone call, a voice or video call in an app, or one being screened. */
    private fun inCall(mode: Int): Boolean =
        mode == AudioManager.MODE_IN_CALL ||
            mode == AudioManager.MODE_IN_COMMUNICATION ||
            (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R && mode == AudioManager.MODE_CALL_SCREENING)

    /** The Bluetooth audio outputs this Android knows of. */
    private val bluetoothOutputs: Set<Int> = buildSet {
        add(AudioDeviceInfo.TYPE_BLUETOOTH_A2DP)
        add(AudioDeviceInfo.TYPE_BLUETOOTH_SCO)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) add(AudioDeviceInfo.TYPE_HEARING_AID)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            add(AudioDeviceInfo.TYPE_BLE_HEADSET)
            add(AudioDeviceInfo.TYPE_BLE_SPEAKER)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) add(AudioDeviceInfo.TYPE_BLE_BROADCAST)
    }
}
