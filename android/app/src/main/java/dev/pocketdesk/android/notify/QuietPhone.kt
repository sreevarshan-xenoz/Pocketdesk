package dev.pocketdesk.android.notify

import android.content.Context
import android.os.SystemClock
import android.util.Log
import androidx.core.content.edit
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.files.PhoneFilesRule
import org.json.JSONObject

/** D2's switches and the computer's mute, kept across a process death. */
object QuietPrefs {
    private const val PREFS = "pocketdesk-quiet"
    private const val KEY_HOLD_BACK = "hold_back"
    private const val KEY_LET_MUTE = "let_computer_mute"
    private const val KEY_UNTIL = "mute_until_ms"
    private const val KEY_FILTER = "mute_filter"

    private fun prefs(ctx: Context) = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    /** Hold back notifications Do Not Disturb hides from the phone itself. On by default. */
    fun holdBack(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_HOLD_BACK, true)

    fun setHoldBack(ctx: Context, on: Boolean) = prefs(ctx).edit { putBoolean(KEY_HOLD_BACK, on) }

    /** Let the phone's own computer turn on Do Not Disturb for a while. Off by default. */
    fun letComputerMute(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_LET_MUTE, false)

    fun setLetComputerMute(ctx: Context, on: Boolean) = prefs(ctx).edit { putBoolean(KEY_LET_MUTE, on) }

    /** The computer's mute running now, if any. */
    fun mute(ctx: Context): QuietPlan.Mute? {
        val until = prefs(ctx).getLong(KEY_UNTIL, -1)
        if (until < 0) return null
        return QuietPlan.Mute(until, prefs(ctx).getInt(KEY_FILTER, QuietPlan.PRIORITY))
    }

    fun setMute(ctx: Context, mute: QuietPlan.Mute?) = prefs(ctx).edit {
        if (mute == null) {
            remove(KEY_UNTIL)
            remove(KEY_FILTER)
        } else {
            putLong(KEY_UNTIL, mute.untilMs)
            putInt(KEY_FILTER, mute.filter)
        }
    }
}

/**
 * The computer muting this phone (D2), on the notification listener's tick:
 * keeps the link to its own computer while "Let the computer mute this
 * phone" is on, carries out the newest mute from that computer, ends a mute
 * at its time while the filter is still the one it set, and tells the
 * computer when the phone's state changes or the link comes up.
 *
 * [filter] reads the listener's interruption filter; [setFilter] asks
 * Android for one (`requestInterruptionFilter`), which a listener may.
 */
class BackgroundQuiet(
    private val ctx: Context,
    private val filter: () -> Int,
    private val setFilter: (Int) -> Unit,
) {
    private var lastConnectMs = 0L

    /** What the computer was last told, and on which link; null forces a report. */
    private var told: Pair<QuietPlan.State, Pair<String, Int>?>? = null

    fun tick() {
        val on = QuietPrefs.letComputerMute(ctx)
        val own = PhoneLink.target(ctx)
        if (on && own != null && RustCore.controlPeer() == null) {
            val now = SystemClock.elapsedRealtime()
            if (now - lastConnectMs >= RECONNECT_MS) {
                lastConnectMs = now
                RustCore.connectToPeer(own.address, own.controlPort, HostStore.wanIdentity(ctx, own.address))
            }
        }
        take(on, own)
        end()
        if (on) report()
    }

    /** The interruption filter changed (the listener's callback): tell the computer. */
    fun filterChanged() {
        if (QuietPrefs.letComputerMute(ctx)) report()
    }

    private fun take(on: Boolean, own: HostStore.Host?) {
        val asked = runCatching { JSONObject(RustCore.takeQuiet()) }.getOrNull() ?: return
        val host = asked.optString("host")
        val port = asked.optInt("port")
        if (!on || !PhoneFilesRule.fromOwnComputer(host, port, own?.address, own?.controlPort)) {
            Log.i(TAG, "a mute from the computer, not taken (switch off, or not its own computer)")
            // The computer is told the phone's state, so it shows nothing changed.
            told = null
            return
        }
        val minutes = QuietPlan.minutes(asked.optInt("minutes", -1)) ?: return
        if (filter() == QuietPlan.UNKNOWN) {
            // Android has not connected the listener: a filter asked for now
            // would do nothing, and a mute kept would end the owner's later.
            Log.i(TAG, "a mute from the computer, not taken: the listener is not connected")
            return
        }
        if (minutes == 0) {
            val mute = QuietPrefs.mute(ctx)
            // Only the computer's own mute is ended, and only while it stands.
            if (mute != null && filter() == mute.filter) setFilter(QuietPlan.ALL)
            QuietPrefs.setMute(ctx, null)
        } else if (QuietPlan.mayMute(filter())) {
            setFilter(QuietPlan.PRIORITY)
            QuietPrefs.setMute(ctx, QuietPlan.Mute(System.currentTimeMillis() + minutes * 60_000L, QuietPlan.PRIORITY))
        } else {
            // The owner's own Do Not Disturb stands; the computer is told it.
            Log.i(TAG, "a mute from the computer, not taken: already in Do Not Disturb")
        }
        told = null
    }

    private fun end() {
        val mute = QuietPrefs.mute(ctx) ?: return
        when (QuietPlan.next(mute, System.currentTimeMillis(), filter())) {
            QuietPlan.Next.END -> {
                setFilter(QuietPlan.ALL)
                QuietPrefs.setMute(ctx, null)
                told = null
            }
            QuietPlan.Next.FORGET -> {
                QuietPrefs.setMute(ctx, null)
                told = null
            }
            QuietPlan.Next.WAIT -> {}
        }
    }

    private fun report() {
        val peer = RustCore.controlPeer() ?: return
        val state = QuietPlan.state(filter(), QuietPrefs.mute(ctx)) ?: return
        if (told == state to peer) return
        val body = JSONObject().put("quiet", state.quiet).put("byComputer", state.byComputer)
        state.untilUnix?.let { body.put("until", it) }
        if (PhoneLink.sendNow(ctx, STATE, body)) told = state to peer
    }

    private companion object {
        const val TAG = "BackgroundQuiet"
        const val STATE = "kdeconnect.pocketdesk.quiet_state"

        // As the other background features retry: a computer away costs a dial a minute.
        const val RECONNECT_MS = 60_000L
    }
}
