package dev.pocketdesk.android.notify

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.media.AudioManager
import android.telecom.TelecomManager
import android.telephony.PhoneStateListener
import android.telephony.TelephonyManager
import android.util.Log
import dev.pocketdesk.android.Prefs
import org.json.JSONArray
import org.json.JSONObject

/**
 * Ringing calls on the computer (F5), run by the notification listener:
 * a call that rings is told to the phone's own computer as a notification
 * with "Silence" (and "Decline" when the phone may end calls); its end
 * takes it away. The computer's answers come to [act].
 */
class CallWatch(private val ctx: Context) {
    /** The id of the call note shown now, while its call rings. */
    @Volatile
    var ringing: String? = null
        private set

    private var serial = 0L
    private var mutedByUs = false

    /** Whether the listener is registered: [start] may run again. */
    @Volatile
    private var registered = false
    private val telephony = ctx.getSystemService(TelephonyManager::class.java)

    @Suppress("DEPRECATION") // TelephonyCallback gives no number; this, with the call log permission, does.
    private val listener = object : PhoneStateListener() {
        @Deprecated("Deprecated in Java")
        override fun onCallStateChanged(state: Int, phoneNumber: String?) {
            when (state) {
                TelephonyManager.CALL_STATE_RINGING -> rang(phoneNumber)
                else -> ended()
            }
        }
    }

    /**
     * Listen, when the switch is on and the phone's state may be read, if
     * not listening yet. Called at the listener's start and again on its
     * ticks: the permission is usually granted while it runs (F5 review).
     */
    @Suppress("DEPRECATION")
    fun start() {
        if (registered || !Prefs.callsOn(ctx) || !granted(ctx, Manifest.permission.READ_PHONE_STATE)) return
        runCatching { telephony?.listen(listener, PhoneStateListener.LISTEN_CALL_STATE) }
            .onSuccess { registered = true }
            .onFailure { Log.w(TAG, "could not listen to calls", it) }
    }

    @Suppress("DEPRECATION")
    fun stop() {
        runCatching { telephony?.listen(listener, PhoneStateListener.LISTEN_NONE) }
        registered = false
    }

    private fun rang(number: String?) {
        if (!Prefs.callsOn(ctx)) return
        // Ringing again (Android tells it once without the number, then
        // with it): the same note, updated, never a second (F5 review).
        val id = ringing?.removePrefix(CallRule.ID_PREFIX)?.toLongOrNull() ?: ++serial
        // Known as ringing at once, on this thread: an end that comes before
        // the note goes still takes it away (F5 review).
        ringing = CallRule.ID_PREFIX + id
        PhoneLink.worker.execute {
            val name = number?.takeIf { it.isNotBlank() }?.let { contactName(ctx, it) }
            val note = CallRule.note(id, number, name, mayDecline(ctx))
            // Ended while the name was looked up: the note is not sent.
            if (ringing != note.id) return@execute
            val body = JSONObject()
                .put("id", note.id)
                .put("app", "Phone")
                .put("title", note.title)
                .put("text", note.text)
                .put("time", System.currentTimeMillis().toString())
                .put("isClearable", false)
                .put("actions", JSONArray(note.actions))
            if (!PhoneLink.sendNow(ctx, "kdeconnect.notification", body)) Log.w(TAG, "a ringing call was not told to the computer")
        }
    }

    private fun ended() {
        val shown = ringing ?: return
        ringing = null
        if (mutedByUs) {
            mutedByUs = false
            runCatching { audio()?.adjustStreamVolume(AudioManager.STREAM_RING, AudioManager.ADJUST_UNMUTE, 0) }
        }
        PhoneLink.worker.execute {
            PhoneLink.sendNow(ctx, "kdeconnect.notification", JSONObject().put("id", shown).put("isCancel", true))
        }
    }

    /** Do [act] for the call ringing now: null when done, else why not. */
    fun act(act: CallRule.Act): String? = when (act) {
        CallRule.Act.Silence -> runCatching {
            audio()?.adjustStreamVolume(AudioManager.STREAM_RING, AudioManager.ADJUST_MUTE, 0)
            mutedByUs = true
        }.exceptionOrNull()?.let {
            Log.w(TAG, "could not silence the call", it)
            "the phone would not silence it: allow Pocketdesk Do Not Disturb access on the phone"
        }
        CallRule.Act.Decline -> if (android.os.Build.VERSION.SDK_INT < android.os.Build.VERSION_CODES.P) {
            "this phone's Android lets no app end calls"
        } else if (ctx.checkSelfPermission(Manifest.permission.ANSWER_PHONE_CALLS) != PackageManager.PERMISSION_GRANTED) {
            "Pocketdesk on the phone may not end calls"
        } else {
            val ended = runCatching {
                @Suppress("DEPRECATION")
                ctx.getSystemService(TelecomManager::class.java)?.endCall() == true
            }.getOrElse {
                Log.w(TAG, "could not decline the call", it)
                false
            }
            if (ended) null else "the phone would not end the call"
        }
        else -> CallRule.refusal(act)
    }

    private fun audio(): AudioManager? = ctx.getSystemService(AudioManager::class.java)

    companion object {
        private const val TAG = "Pocketdesk"

        fun granted(ctx: Context, permission: String): Boolean =
            ctx.checkSelfPermission(permission) == PackageManager.PERMISSION_GRANTED

        /** Whether the phone may end calls for the computer: the switch on and the permission granted. */
        fun mayDecline(ctx: Context): Boolean =
            android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.P &&
                Prefs.callsDecline(ctx) && granted(ctx, Manifest.permission.ANSWER_PHONE_CALLS)

        /** What calls on the computer need of Android: the phone's state, and the call log for the number. */
        val permissions = arrayOf(Manifest.permission.READ_PHONE_STATE, Manifest.permission.READ_CALL_LOG)
    }
}
