package dev.pocketdesk.android.notify

import android.Manifest
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.SystemClock
import android.provider.Telephony
import android.telephony.SmsManager
import android.util.Log
import androidx.core.content.ContextCompat
import androidx.core.content.edit
import dev.pocketdesk.android.bridge.RustCore
import org.json.JSONArray
import org.json.JSONObject

/**
 * Texts (SMS) on the computer (N5b): whether incoming texts are shown there,
 * and whether the computer may send replies through this phone. Both off
 * until the user turns them on.
 */
object TextPrefs {
    private const val PREFS = "pocketdesk-texts"
    private const val KEY_SHOW = "show"
    private const val KEY_REPLY = "reply"

    private fun prefs(ctx: Context) = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    fun show(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_SHOW, false)

    fun setShow(ctx: Context, on: Boolean) = prefs(ctx).edit { putBoolean(KEY_SHOW, on) }

    fun reply(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_REPLY, false)

    fun setReply(ctx: Context, on: Boolean) = prefs(ctx).edit { putBoolean(KEY_REPLY, on) }

    fun granted(ctx: Context, permission: String): Boolean =
        ContextCompat.checkSelfPermission(ctx, permission) == PackageManager.PERMISSION_GRANTED
}

/** Forwards each incoming text to the computer, when the user turned it on. */
class SmsReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Telephony.Sms.Intents.SMS_RECEIVED_ACTION) return
        val app = context.applicationContext
        if (!TextPrefs.show(app) || !TextPrefs.granted(app, Manifest.permission.RECEIVE_SMS)) return
        val parts =
            runCatching { Telephony.Sms.Intents.getMessagesFromIntent(intent) }.getOrNull()
                ?.mapNotNull { m -> m?.originatingAddress?.let { it to (m.messageBody ?: "") } }
                .orEmpty()
        val texts = SmsReplyRule.joinParts(parts)
        if (texts.isEmpty()) return
        val now = System.currentTimeMillis()
        val messages = JSONArray()
        texts.forEach { (address, body) ->
            messages.put(JSONObject().put("address", address).put("body", body).put("date", now).put("event", "received"))
        }
        val pending = goAsync()
        RustCore.start(app.filesDir)
        PhoneLink.worker.execute {
            try {
                PhoneLink.sendNow(app, "kdeconnect.sms.messages", JSONObject().put("messages", messages))
            } finally {
                pending.finish()
            }
        }
    }
}

/**
 * The replies the computer asks this phone to text (N5b). The notification
 * listener runs it every few seconds while "Reply from my computer" is on:
 * it keeps the control connection to the computer up (trying at most once a
 * minute), takes the queued requests, sends each allowed one and reports the
 * result.
 */
class SmsReplies(private val ctx: Context) {
    private var lastConnectMs = 0L

    fun tick() {
        if (!TextPrefs.reply(ctx)) return
        val host = PhoneLink.target(ctx) ?: return
        val now = SystemClock.elapsedRealtime()
        if (now - lastConnectMs >= 60_000L) {
            lastConnectMs = now
            RustCore.connectToPeer(host.address, host.controlPort)
        }
        for (request in RustCore.takeSmsRequests()) {
            val permitted = TextPrefs.granted(ctx, Manifest.permission.SEND_SMS)
            val result = JSONObject().put("id", request.id)
            when (val decision = SmsReplyRule.decide(request, TextPrefs.reply(ctx), permitted)) {
                SmsReplyRule.Decision.Send ->
                    runCatching { send(request) }
                        .onSuccess { result.put("ok", true) }
                        .onFailure {
                            Log.w(TAG, "reply ${request.id} not sent", it)
                            result.put("ok", false).put("error", it.message ?: "the phone could not send it")
                        }
                is SmsReplyRule.Decision.Refuse -> result.put("ok", false).put("error", decision.reason)
            }
            PhoneLink.sendNow(ctx, "kdeconnect.pocketdesk.sms_sent", result)
        }
    }

    /** Hand the text to Android's SMS service, in parts when long. */
    private fun send(request: SmsRequest) {
        val manager =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                ctx.getSystemService(SmsManager::class.java)
            } else {
                @Suppress("DEPRECATION")
                SmsManager.getDefault()
            } ?: error("this phone has no SMS service")
        val parts = manager.divideMessage(request.body.trim())
        manager.sendMultipartTextMessage(request.address.trim(), null, parts, null, null)
    }

    private companion object {
        const val TAG = "SmsReplies"
    }
}
