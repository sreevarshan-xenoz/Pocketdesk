package dev.pocketdesk.android.shortcuts

import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.os.SystemClock
import android.text.format.DateUtils
import android.view.View
import android.widget.RemoteViews
import androidx.core.content.edit
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.ui.NowPlaying
import dev.pocketdesk.android.ui.parsePlaying
import dev.pocketdesk.android.ui.plainErrorText
import org.json.JSONObject
import java.util.concurrent.atomic.AtomicLong

/**
 * Home-screen widget (G5): what the last computer plays (T8c), with
 * Previous, Play/Pause and Next as its player offers them. ↻, placing the
 * widget or a new last computer asks; a press asks again after its
 * command. Never by the system's period ([MediaWidgetPlan]).
 */
class MediaWidget : AppWidgetProvider() {
    override fun onUpdate(context: Context, manager: AppWidgetManager, ids: IntArray) {
        read(context, MediaWidgetPlan.Why.Period)
    }

    override fun onReceive(context: Context, intent: Intent) {
        when (intent.action) {
            ACTION_REFRESH -> read(context, MediaWidgetPlan.Why.Refresh)
            ACTION_FOLLOW -> read(context, MediaWidgetPlan.Why.Follow)
            ACTION_PRESS -> press(
                context,
                intent.getStringExtra(EXTRA_COMPUTER),
                MediaWidgetPlan.button(intent.getStringExtra(EXTRA_COMMAND)),
            )
            else -> super.onReceive(context, intent)
        }
    }

    private fun read(context: Context, why: MediaWidgetPlan.Why) {
        val app = context.applicationContext
        val host = HostStore.lastHost(app) ?: return redrawAll(app)
        val kept = Kept.read(app, host.address)
        if (!MediaWidgetPlan.ask(why, kept.json != null || kept.failed)) return redrawAll(app)
        ask(app, host, command = null)
    }

    private fun press(context: Context, drawnFor: String?, button: MediaWidgetPlan.Button?) {
        val app = context.applicationContext
        val host = HostStore.lastHost(app) ?: return redrawAll(app)
        // A button drawn for one computer never presses another's (G3's lesson).
        if (button == null || !MediaWidgetPlan.keeps(drawnFor, host.address)) return redrawAll(app)
        ask(app, host, command = MediaWidgetPlan.command(button))
    }

    /** Send [command] when there is one, then ask what plays; only the newest ask writes. */
    private fun ask(app: Context, host: HostStore.Host, command: String?) {
        // Taken with the flag at once, so an older ask's end never clears it (G5 review).
        val ask = synchronized(generation) {
            asking = true
            generation.incrementAndGet()
        }
        val started = SystemClock.elapsedRealtime()
        redrawAll(app)
        val noAnswer = {
            finish(app, host.address, ask) { Kept.failed(app, host.address, app.getString(R.string.deck_widget_no_answer)) }
        }
        // A worker that threw never leaves "Asking…" up (G5 review).
        val ended = {
            val stuck = synchronized(generation) {
                (generation.get() == ask && asking).also { if (it) asking = false }
            }
            if (stuck) redrawAll(app)
        }
        WidgetAsk.bounded(this, "media-widget", onEnd = ended, onTimeout = noAnswer) {
            if (command != null) {
                val sent = WidgetAsk.ask(app) { audio(host, JSONObject().put("action", "mediaControl").put("command", command)) }
                val refusal = sent.fold({ refusal(it) }, { app.plainErrorText(it.message) })
                if (refusal != null) {
                    finish(app, host.address, ask) { Kept.failed(app, host.address, refusal) }
                    return@bounded
                }
                // The player takes a moment to say what changed; a command that
                // took most of the wait leaves the reading to ↻ (G5 review).
                if (SystemClock.elapsedRealtime() - started > READ_AFTER_MS) {
                    finish(app, host.address, ask) {}
                    return@bounded
                }
                Thread.sleep(SETTLE_MS)
            }
            val answer = WidgetAsk.ask(app) { audio(host, JSONObject().put("action", "media")) }
            val json = answer.getOrNull()
            val refusal = answer.fold({ refusal(it) }, { app.plainErrorText(it.message) })
            finish(app, host.address, ask) {
                if (json != null && refusal == null) Kept.save(app, host.address, json) else Kept.failed(app, host.address, refusal.orEmpty())
            }
        }
    }

    /** Keep what ask [ask] heard with [write], if it is still the newest, and draw. */
    private fun finish(app: Context, computer: String, ask: Long, write: () -> Unit) {
        val newest = synchronized(generation) {
            (generation.get() == ask).also {
                if (it) {
                    write()
                    asking = false
                }
            }
        }
        if (newest && HostStore.lastHost(app)?.address == computer) redrawAll(app)
    }

    private fun audio(host: HostStore.Host, body: JSONObject): Result<String> =
        RustCore.audioControl(host.address, host.controlPort, body.toString())

    /** The computer's refusal in an answer (Audio off for this phone), or null. */
    private fun refusal(json: String): String? = runCatching {
        val body = JSONObject(json)
        if (body.optBoolean("ok", true)) null else body.optString("error").ifBlank { "refused" }
    }.getOrElse { "an answer that did not read" }

    /** What plays on the last computer, kept between draws. */
    private class Kept(val json: String?, val readAt: Long?, val failed: Boolean, val line: String?) {
        companion object {
            private const val FILE = "media_widget"
            private const val KEY_FOR = "for"
            private const val KEY_JSON = "json"
            private const val KEY_AT = "at"
            private const val KEY_FAILED = "failed"
            private const val KEY_LINE = "line"

            private fun prefs(ctx: Context) = ctx.getSharedPreferences(FILE, Context.MODE_PRIVATE)

            @Synchronized
            fun read(ctx: Context, lastComputer: String?): Kept {
                val p = prefs(ctx)
                val readFor = p.getString(KEY_FOR, null)
                if (!MediaWidgetPlan.keeps(readFor, lastComputer)) {
                    if (readFor != null && lastComputer != null) p.edit { clear() }
                    return Kept(null, null, false, null)
                }
                val at = p.getLong(KEY_AT, 0L)
                return Kept(p.getString(KEY_JSON, null), at.takeIf { it > 0 }, p.getBoolean(KEY_FAILED, false), p.getString(KEY_LINE, null))
            }

            /** Whether [computer] is still the last one: a late answer for another never writes. */
            private fun last(ctx: Context, computer: String) = HostStore.lastHost(ctx)?.address == computer

            @Synchronized
            fun save(ctx: Context, computer: String, json: String) {
                if (last(ctx, computer)) store(ctx, computer, json)
            }

            private fun store(ctx: Context, computer: String, json: String) = prefs(ctx).edit {
                clear()
                putString(KEY_FOR, computer)
                putString(KEY_JSON, json)
                putLong(KEY_AT, System.currentTimeMillis())
            }

            @Synchronized
            fun failed(ctx: Context, computer: String, line: String) {
                if (!last(ctx, computer)) return
                val p = prefs(ctx)
                p.edit {
                    if (p.getString(KEY_FOR, null) != computer) {
                        clear()
                        putString(KEY_FOR, computer)
                    }
                    putBoolean(KEY_FAILED, true)
                    putString(KEY_LINE, line)
                }
            }
        }
    }

    companion object {
        private const val ACTION_REFRESH = "dev.pocketdesk.android.action.MEDIA_WIDGET_REFRESH"
        private const val ACTION_FOLLOW = "dev.pocketdesk.android.action.MEDIA_WIDGET_FOLLOW"
        private const val ACTION_PRESS = "dev.pocketdesk.android.action.MEDIA_WIDGET_PRESS"
        private const val EXTRA_COMPUTER = "computer"
        private const val EXTRA_COMMAND = "command"
        private const val SETTLE_MS = 300L

        /** Past this, a press does not read what plays: the 8 s wait would end first. */
        private const val READ_AFTER_MS = 4_000L

        /** The newest ask in this process: only it keeps what it heard. */
        private val generation = AtomicLong()

        /** An ask is under way, said on the line; in memory only (G3's lesson). */
        @Volatile
        private var asking = false

        /** The last computer may have changed: ask once when it was never asked. */
        fun follow(context: Context) {
            context.sendBroadcast(Intent(context, MediaWidget::class.java).setAction(ACTION_FOLLOW))
        }

        private fun redrawAll(app: Context) {
            val manager = AppWidgetManager.getInstance(app) ?: return
            val ids = manager.getAppWidgetIds(ComponentName(app, MediaWidget::class.java))
            if (ids.isNotEmpty()) redraw(app, manager, ids)
        }

        private fun broadcast(context: Context, code: Int, intent: Intent): PendingIntent =
            PendingIntent.getBroadcast(
                context,
                code,
                intent.setClass(context, MediaWidget::class.java),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )

        /** Draw every media widget from what is kept, asking nothing. */
        fun redraw(context: Context, manager: AppWidgetManager, ids: IntArray) {
            val host = HostStore.lastHost(context)
            val kept = Kept.read(context, host?.address)
            val playing: NowPlaying? = kept.json?.let { json -> runCatching { parsePlaying(JSONObject(json)) }.getOrNull() }
            val views = RemoteViews(context.packageName, R.layout.widget_media)
            views.setTextViewText(
                R.id.widget_media_host,
                host?.let { HostStore.shownName(context, it.address) } ?: context.getString(R.string.tile_no_computer),
            )
            val title = when {
                host == null -> ""
                playing != null -> playing.headline()
                kept.json != null -> context.getString(R.string.media_widget_nothing)
                else -> ""
            }
            views.setTextViewText(R.id.widget_media_title, title)
            views.setTextViewText(R.id.widget_media_subline, playing?.subline().orEmpty())
            val line = when {
                host == null -> null
                asking -> context.getString(R.string.deck_loading)
                kept.failed -> kept.line
                kept.readAt != null -> context.getString(
                    R.string.media_widget_read,
                    DateUtils.formatSameDayTime(kept.readAt, System.currentTimeMillis(), java.text.DateFormat.SHORT, java.text.DateFormat.SHORT),
                )
                else -> context.getString(R.string.deck_loading)
            }
            views.setTextViewText(R.id.widget_media_line, line.orEmpty())

            val offered = MediaWidgetPlan.buttons(playing)
            val toggle = MediaWidgetPlan.toggle(playing)
            fun button(id: Int, button: MediaWidgetPlan.Button, shown: Boolean, slot: Int) {
                views.setViewVisibility(id, if (shown && host != null) View.VISIBLE else View.INVISIBLE)
                if (!shown || host == null) return
                views.setOnClickPendingIntent(
                    id,
                    broadcast(
                        context,
                        1 + slot,
                        Intent(ACTION_PRESS)
                            .putExtra(EXTRA_COMPUTER, host.address)
                            .putExtra(EXTRA_COMMAND, MediaWidgetPlan.command(button)),
                    ),
                )
            }
            button(R.id.widget_media_previous, MediaWidgetPlan.Button.Previous, offered.previous, 0)
            button(R.id.widget_media_play, MediaWidgetPlan.Button.PlayPause, offered.playPause, 1)
            button(R.id.widget_media_next, MediaWidgetPlan.Button.Next, offered.next, 2)
            views.setTextViewText(
                R.id.widget_media_play,
                context.getString(if (toggle == MediaWidgetPlan.Toggle.Pause) R.string.media_widget_pause_glyph else R.string.media_widget_play_glyph),
            )
            views.setContentDescription(
                R.id.widget_media_play,
                context.getString(if (toggle == MediaWidgetPlan.Toggle.Pause) R.string.media_widget_pause else R.string.media_widget_play),
            )
            views.setViewVisibility(R.id.widget_media_refresh, if (host == null) View.GONE else View.VISIBLE)
            views.setOnClickPendingIntent(R.id.widget_media_refresh, broadcast(context, 0, Intent(ACTION_REFRESH)))
            if (host == null) {
                views.setOnClickPendingIntent(
                    R.id.widget_media_root,
                    PendingIntent.getActivity(
                        context,
                        0,
                        Shortcuts.connectLast(context),
                        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                    ),
                )
            }
            manager.updateAppWidget(ids, views)
        }
    }
}
