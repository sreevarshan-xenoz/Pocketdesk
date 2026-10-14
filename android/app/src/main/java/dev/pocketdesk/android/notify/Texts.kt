package dev.pocketdesk.android.notify

import android.Manifest
import android.app.Activity
import android.app.PendingIntent
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
            when (val decision = SmsReplyRule.decide(request, TextPrefs.reply(ctx), permitted)) {
                // The result is reported when the radio answers every part
                // (SmsSentReceiver), not when Android merely accepts the text.
                SmsReplyRule.Decision.Send ->
                    runCatching { send(request) }.onFailure {
                        Log.w(TAG, "reply ${request.id} not sent", it)
                        report(ctx, request.id, it.message ?: "the phone could not send it")
                    }
                is SmsReplyRule.Decision.Refuse -> report(ctx, request.id, decision.reason)
            }
        }
    }

    /** Hand the text to Android's SMS service, in parts when long, asking to hear how each went. */
    private fun send(request: SmsRequest) {
        val manager =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                ctx.getSystemService(SmsManager::class.java)
            } else {
                @Suppress("DEPRECATION")
                SmsManager.getDefault()
            } ?: error("this phone has no SMS service")
        val parts = manager.divideMessage(request.body.trim())
        SmsSentTracker.expect(request.id, parts.size)
        val sentIntents =
            ArrayList(
                parts.indices.map { part ->
                    val intent =
                        Intent(ctx, SmsSentReceiver::class.java)
                            .setAction(SmsSentReceiver.ACTION)
                            .putExtra(SmsSentReceiver.EXTRA_ID, request.id)
                    PendingIntent.getBroadcast(
                        ctx,
                        (request.id.hashCode() * 31) + part,
                        intent,
                        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                    )
                },
            )
        manager.sendMultipartTextMessage(request.address.trim(), null, parts, sentIntents, null)
    }

    companion object {
        private const val TAG = "SmsReplies"

        /** Tell the computer how reply [id] went: sent, or why not. */
        fun report(ctx: Context, id: String, error: String?) {
            val result = JSONObject().put("id", id).put("ok", error == null)
            if (error != null) result.put("error", error)
            PhoneLink.worker.execute { PhoneLink.sendNow(ctx, "kdeconnect.pocketdesk.sms_sent", result) }
        }
    }
}

/** The parts of each reply still waiting for the radio's answer, and the first failure. */
object SmsSentTracker {
    private val waiting = HashMap<String, Pair<Int, String?>>()

    @Synchronized
    fun expect(id: String, parts: Int) {
        waiting[id] = parts.coerceAtLeast(1) to null
    }

    /**
     * One part of [id] answered ([error] null: sent). Returns the reply's
     * outcome once every part has: `null` while some are still out, else
     * `Result` with the first error, if any.
     */
    @Synchronized
    fun answered(id: String, error: String?): kotlin.Result<String?>? {
        val (left, firstError) = waiting[id] ?: return null
        val now = firstError ?: error
        if (left > 1) {
            waiting[id] = (left - 1) to now
            return null
        }
        waiting.remove(id)
        return kotlin.Result.success(now)
    }
}

/** The radio's answer for one part of a reply (N5b). */
class SmsSentReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val id = intent.getStringExtra(EXTRA_ID) ?: return
        val error = if (resultCode == Activity.RESULT_OK) null else reason(resultCode)
        val outcome = SmsSentTracker.answered(id, error) ?: return
        SmsReplies.report(context.applicationContext, id, outcome.getOrNull())
    }

    companion object {
        const val ACTION = "dev.pocketdesk.android.SMS_SENT"
        const val EXTRA_ID = "id"

        /** What the radio's result code means, in words for the computer. */
        fun reason(code: Int): String =
            when (code) {
                SmsManager.RESULT_ERROR_NO_SERVICE -> "the phone has no mobile service"
                SmsManager.RESULT_ERROR_RADIO_OFF -> "the phone's mobile radio is off"
                SmsManager.RESULT_ERROR_NULL_PDU -> "the phone could not encode the text"
                SmsManager.RESULT_ERROR_GENERIC_FAILURE -> "the phone could not send it"
                SmsManager.RESULT_ERROR_LIMIT_EXCEEDED -> "the phone has sent too many texts for now"
                SmsManager.RESULT_ERROR_SHORT_CODE_NOT_ALLOWED,
                SmsManager.RESULT_ERROR_SHORT_CODE_NEVER_ALLOWED,
                -> "the phone does not allow texts to that short number"
                SmsManager.RESULT_RADIO_NOT_AVAILABLE -> "the phone's mobile radio is not available"
                SmsManager.RESULT_NO_DEFAULT_SMS_APP -> "the phone has no SIM chosen for texts"
                else -> "the phone could not send it (error $code)"
            }
    }
}
