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
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.bridge.RustCore
import org.json.JSONArray
import org.json.JSONObject

/**
 * Texts (SMS) on the computer (N5b): whether incoming texts are shown there,
 * and whether the computer may send replies through this phone. Both off
 * until the user turns them on. Also the little state that must survive the
 * app's process: texts not yet delivered, replies whose parts the radio has
 * not answered yet, and the replies already handled.
 */
object TextPrefs {
    private const val PREFS = "pocketdesk-texts"
    private const val KEY_SHOW = "show"
    private const val KEY_REPLY = "reply"
    private const val KEY_NAMES = "names"
    private const val KEY_QUEUE = "queue"
    private const val KEY_PARTS = "parts"
    private const val KEY_HANDLED = "handled"

    /** Texts kept for a computer out of reach, at most. */
    private const val MAX_QUEUED = 200

    /** Reply ids remembered as handled, at most. */
    private const val MAX_HANDLED = 200

    /** Marks a reply handed to the radio whose parts have not all answered. */
    const val PENDING = "…"

    private fun prefs(ctx: Context) = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    fun show(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_SHOW, false)

    /** Turned off, the texts kept for the computer go too (BC4: they were sent once it was on again). */
    fun setShow(ctx: Context, on: Boolean) = prefs(ctx).edit {
        putBoolean(KEY_SHOW, on)
        if (!on) remove(KEY_QUEUE)
    }

    fun reply(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_REPLY, false)

    fun setReply(ctx: Context, on: Boolean) = prefs(ctx).edit { putBoolean(KEY_REPLY, on) }

    /** Whether the own computer may ask for this phone's contacts, names and numbers only (P10). Off. */
    fun contactsToComputer(ctx: Context): Boolean = prefs(ctx).getBoolean("contacts_to_computer", false)

    fun setContactsToComputer(ctx: Context, on: Boolean) = prefs(ctx).edit { putBoolean("contacts_to_computer", on) }

    /** Whether texts carry the sender's name from this phone's contacts (T8e). Off by default. */
    fun names(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_NAMES, false)

    fun setNames(ctx: Context, on: Boolean) = prefs(ctx).edit { putBoolean(KEY_NAMES, on) }

    fun granted(ctx: Context, permission: String): Boolean =
        ContextCompat.checkSelfPermission(ctx, permission) == PackageManager.PERMISSION_GRANTED

    private fun array(ctx: Context, key: String) =
        runCatching { JSONArray(prefs(ctx).getString(key, "[]")) }.getOrDefault(JSONArray())

    private fun obj(ctx: Context, key: String) =
        runCatching { JSONObject(prefs(ctx).getString(key, "{}") ?: "{}") }.getOrDefault(JSONObject())

    /** Keep [texts] until the computer takes them; the oldest go past [MAX_QUEUED]. */
    @Synchronized
    fun enqueue(ctx: Context, texts: List<JSONObject>) {
        val queue = array(ctx, KEY_QUEUE)
        texts.forEach { queue.put(it) }
        val kept = JSONArray()
        for (i in maxOf(0, queue.length() - MAX_QUEUED) until queue.length()) kept.put(queue.get(i))
        prefs(ctx).edit { putString(KEY_QUEUE, kept.toString()) }
    }

    /**
     * Up to [TextQueue.BATCH] texts waiting for [own] (the own computer's
     * paired id, null when there is none), or kept for one among
     * [refusing]; those kept for a computer no longer among [saved] are
     * dropped (BC4).
     */
    @Synchronized
    fun queued(ctx: Context, own: String?, saved: Set<String>, refusing: Set<String>): List<JSONObject> {
        val queue = array(ctx, KEY_QUEUE)
        val texts = List(queue.length()) { queue.getJSONObject(it) }
        val split = TextQueue.split(texts.map { it.optString(TextQueue.FOR) }, own, saved, refusing)
        if (split.drop.isNotEmpty()) {
            val kept = JSONArray()
            texts.forEachIndexed { i, text -> if (i !in split.drop) kept.put(text) }
            prefs(ctx).edit { putString(KEY_QUEUE, kept.toString()) }
        }
        return split.send.map { texts[it] }
    }

    /** These queued texts were delivered (each once, as [queued] gave them). */
    @Synchronized
    fun dequeue(ctx: Context, sent: List<JSONObject>) {
        val left = sent.map { it.toString() }.toMutableList()
        val queue = array(ctx, KEY_QUEUE)
        val kept = JSONArray()
        for (i in 0 until queue.length()) {
            val text = queue.getJSONObject(i)
            if (!left.remove(text.toString())) kept.put(text)
        }
        prefs(ctx).edit { putString(KEY_QUEUE, kept.toString()) }
    }

    /** Reply [id] went to the radio in [parts] parts, for the computer at [host]:[port]. */
    @Synchronized
    fun expectParts(ctx: Context, id: String, parts: Int, host: String, port: Int) {
        val all = obj(ctx, KEY_PARTS)
        all.put(id, JSONObject().put("left", parts.coerceAtLeast(1)).put("host", host).put("port", port))
        prefs(ctx).edit { putString(KEY_PARTS, all.toString()) }
    }

    /**
     * One part of reply [id] answered ([error] null: sent). Once all have,
     * returns the outcome (the first error, or null) and where to report it.
     */
    @Synchronized
    fun partAnswered(ctx: Context, id: String, error: String?): Triple<String?, String, Int>? {
        val all = obj(ctx, KEY_PARTS)
        val entry = all.optJSONObject(id) ?: return null
        val first = entry.optString("error").ifEmpty { null } ?: error
        val left = entry.optInt("left", 1) - 1
        if (left > 0) {
            entry.put("left", left)
            if (first != null) entry.put("error", first)
            all.put(id, entry)
            prefs(ctx).edit { putString(KEY_PARTS, all.toString()) }
            return null
        }
        all.remove(id)
        prefs(ctx).edit { putString(KEY_PARTS, all.toString()) }
        return Triple(first, entry.optString("host"), entry.optInt("port"))
    }

    /** What became of reply [id], if handled before: "" sent, [PENDING], else the error. */
    @Synchronized
    fun handled(ctx: Context, id: String): String? {
        val all = obj(ctx, KEY_HANDLED)
        return if (all.has(id)) all.optString(id) else null
    }

    @Synchronized
    fun setHandled(ctx: Context, id: String, outcome: String) {
        val all = obj(ctx, KEY_HANDLED)
        all.remove(id)
        all.put(id, outcome)
        while (all.length() > MAX_HANDLED) all.remove(all.keys().next())
        prefs(ctx).edit { putString(KEY_HANDLED, all.toString()) }
    }
}

/**
 * Keeps each incoming text, when the user turned it on, until the computer
 * takes it: a computer that is asleep or away gets it later, not never.
 */
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
        // Kept for the own computer of this moment, and sent to it alone (BC4).
        val forId = PhoneLink.target(app)?.let { HostStore.pairedDesktopId(app, it.address) }.orEmpty()
        TextPrefs.enqueue(
            app,
            texts.map { (address, body) ->
                JSONObject().put("address", address).put("body", body).put("date", now).put("event", "received")
                    .put(TextQueue.FOR, forId)
            },
        )
        RustCore.start(app)
        PhoneLink.worker.execute { Texts.flush(app) }
    }
}

object Texts {
    private const val TAG = "Texts"

    /** Deliver the texts waiting for the computer, if it is reachable. */
    fun flush(ctx: Context) {
        if (!TextPrefs.show(ctx)) return
        val own = PhoneLink.target(ctx)?.let { HostStore.pairedDesktopId(ctx, it.address) }
        val hosts = HostStore.hosts(ctx)
        val saved = hosts.mapNotNull { HostStore.pairedDesktopId(ctx, it.address) }.toSet()
        val refusing = hosts.filter { RustCore.unpairedBy(it.address) }
            .mapNotNull { HostStore.pairedDesktopId(ctx, it.address) }.toSet()
        val batch = TextPrefs.queued(ctx, own, saved, refusing)
        if (batch.isEmpty()) return
        // The sender's name is looked up as the text leaves (T8e), here on the
        // worker, not where it arrived on the main thread, and only if the
        // switch is still on now: a text queued with names on goes without
        // one once they are turned off.
        val named = batch.map { text ->
            JSONObject(text.toString()).apply {
                remove("name")
                remove(TextQueue.FOR)
                contactName(ctx, optString("address"))?.let { put("name", it) }
            }
        }
        // To that computer only (review: the own one could change between).
        val sent = PhoneLink.sendNow(ctx, "kdeconnect.sms.messages", JSONObject().put("messages", JSONArray(named)), own)
        if (sent) TextPrefs.dequeue(ctx, batch)
    }

    /** Tell the computer at [host]:[port] how reply [id] went: sent (null), or why not. */
    fun report(ctx: Context, id: String, error: String?, host: String, port: Int) {
        TextPrefs.setHandled(ctx, id, error ?: "")
        val result = JSONObject().put("id", id).put("ok", error == null)
        if (error != null) result.put("error", error)
        // A result is not held back by a pause after some failed send, and
        // goes to the computer that asked.
        PhoneLink.worker.execute {
            RustCore.sendPhonePacket(host, port, "kdeconnect.pocketdesk.sms_sent", result.toString())
                .onFailure { Log.d(TAG, "result of $id not delivered: ${it.message}") }
        }
    }
}

/**
 * The replies the computer asks this phone to text (N5b). The notification
 * listener runs it every few seconds on a thread of its own. It first
 * delivers texts that waited. While "Reply from my computer" is on, it opens
 * a control connection to the phone's computer when there is none (at most
 * once a minute; a session's connection to another computer is left alone),
 * takes the queued requests and sends each allowed one. While it is off,
 * requests are dropped.
 */
class SmsReplies(private val ctx: Context) {
    private var lastConnectMs = 0L
    private val budget = SmsReplyRule.SendBudget()

    fun tick() {
        Texts.flush(ctx)
        if (!TextPrefs.reply(ctx)) {
            RustCore.takeSmsRequests()
            return
        }
        val own = PhoneLink.target(ctx) ?: return
        val now = SystemClock.elapsedRealtime()
        if (RustCore.controlPeer() == null && now - lastConnectMs >= 60_000L) {
            lastConnectMs = now
            RustCore.connectToPeer(own.address, own.controlPort, HostStore.wanIdentity(ctx, own.address))
        }
        for (request in RustCore.takeSmsRequests()) {
            Log.i(TAG, "reply ${request.id} asked for by ${request.fromHost}")
            handle(request, own, now)
        }
    }

    private fun handle(request: SmsRequest, own: HostStore.Host, now: Long) {
        val fromOwn = request.fromHost == own.address && request.fromPort == own.controlPort
        if (!fromOwn) {
            // Not this phone's computer: ignored, without a word to it.
            Log.w(TAG, "reply request from ${request.fromHost} ignored: not this phone's computer")
            return
        }
        // Handed over again (the computer did not hear back): never sent twice.
        when (val before = TextPrefs.handled(ctx, request.id)) {
            null -> {}
            TextPrefs.PENDING -> return
            else -> {
                Texts.report(ctx, request.id, before.ifEmpty { null }, request.fromHost, request.fromPort)
                return
            }
        }
        val permitted = TextPrefs.granted(ctx, Manifest.permission.SEND_SMS)
        when (val decision = SmsReplyRule.decide(request, TextPrefs.reply(ctx), permitted, fromOwn)) {
            SmsReplyRule.Decision.Send ->
                if (!budget.take(now)) {
                    Texts.report(ctx, request.id, SmsReplyRule.TOO_MANY, request.fromHost, request.fromPort)
                } else {
                    runCatching { send(request) }.onFailure {
                        Log.w(TAG, "reply ${request.id} not sent", it)
                        Texts.report(
                            ctx,
                            request.id,
                            it.message ?: "the phone could not send it",
                            request.fromHost,
                            request.fromPort,
                        )
                    }
                }
            is SmsReplyRule.Decision.Refuse -> Texts.report(ctx, request.id, decision.reason, request.fromHost, request.fromPort)
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
        TextPrefs.setHandled(ctx, request.id, TextPrefs.PENDING)
        TextPrefs.expectParts(ctx, request.id, parts.size, request.fromHost, request.fromPort)
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

    private companion object {
        const val TAG = "SmsReplies"
    }
}

/**
 * The radio's answer for one part of a reply (N5b). The parts left and where
 * to report are kept in [TextPrefs], so a process Android recreated to
 * deliver this still reports.
 */
class SmsSentReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val id = intent.getStringExtra(EXTRA_ID) ?: return
        val app = context.applicationContext
        val error = if (resultCode == Activity.RESULT_OK) null else reason(resultCode)
        val (outcome, host, port) = TextPrefs.partAnswered(app, id, error) ?: return
        RustCore.start(app)
        Texts.report(app, id, outcome, host, port)
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
