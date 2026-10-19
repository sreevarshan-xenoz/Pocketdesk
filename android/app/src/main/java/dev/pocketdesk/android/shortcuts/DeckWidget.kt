package dev.pocketdesk.android.shortcuts

import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.util.Log
import android.view.View
import android.widget.RemoteViews
import androidx.core.content.edit
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.ui.Deck
import dev.pocketdesk.android.ui.plainErrorText
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong
import kotlin.concurrent.thread

/**
 * Home-screen widget (G3): the last computer's first six deck buttons
 * (F1), each pressed from the home screen, with a line saying what the
 * last press did. ↻ asks for the buttons again; [DeckWidgetPlan] decides.
 */
class DeckWidget : AppWidgetProvider() {
    override fun onUpdate(context: Context, manager: AppWidgetManager, ids: IntArray) {
        list(context, DeckWidgetPlan.Why.Period)
    }

    override fun onReceive(context: Context, intent: Intent) {
        when (intent.action) {
            ACTION_REFRESH -> list(context, DeckWidgetPlan.Why.Refresh)
            ACTION_FOLLOW -> list(context, DeckWidgetPlan.Why.Follow)
            ACTION_PRESS -> press(
                context,
                intent.getStringExtra(EXTRA_COMPUTER),
                intent.getIntExtra(EXTRA_INDEX, -1),
                intent.getStringExtra(EXTRA_LABEL).orEmpty(),
            )
            else -> super.onReceive(context, intent)
        }
    }

    private fun list(context: Context, why: DeckWidgetPlan.Why) {
        val app = context.applicationContext
        val host = HostStore.lastHost(app) ?: return redrawAll(app)
        val kept = Kept.read(app, host.address)
        if (!DeckWidgetPlan.askList(why, kept.json != null || kept.failed)) return redrawAll(app)
        val ask = generation.incrementAndGet()
        showAsking(app, app.getString(R.string.deck_loading))
        val noAnswer = {
            val newest = synchronized(generation) {
                (generation.get() == ask).also { newest ->
                    if (newest) {
                        Kept.failed(app, host.address, app.getString(R.string.deck_failed, app.getString(R.string.deck_widget_no_answer)))
                        asking = null
                    }
                }
            }
            if (newest) redrawAll(app)
        }
        bounded("deck-widget-list", onTimeout = noAnswer) {
            val answer = ask(app) { RustCore.listDeck(host.address, host.controlPort) }
            val json = answer.getOrNull()?.takeIf { runCatching { Deck.parse(it) }.isSuccess }
            val why = app.plainErrorText(answer.exceptionOrNull()?.message)
            // Only the newest ask keeps what it heard, checked and written at once.
            synchronized(generation) {
                if (generation.get() != ask) return@bounded
                if (json != null) {
                    Kept.save(app, host.address, json)
                } else {
                    Kept.failed(app, host.address, app.getString(R.string.deck_failed, why))
                }
                asking = null
            }
            redrawAll(app)
        }
    }

    private fun press(context: Context, drawnFor: String?, index: Int, label: String) {
        val app = context.applicationContext
        val host = HostStore.lastHost(app) ?: return redrawAll(app)
        // A button drawn for one computer never presses another's (G3 review):
        // the last computer changed since, so it is drawn anew instead.
        if (!DeckWidgetPlan.keeps(drawnFor, host.address)) return redrawAll(app)
        // A press still running is not sent again.
        if (index < 0 || label.isBlank() || !pressing.add(index)) return
        showAsking(app, app.getString(R.string.deck_widget_said, label, app.getString(R.string.deck_loading)))
        val noAnswer = {
            if (index in pressing) {
                asking = null
                line(app, host.address, app.getString(R.string.deck_widget_said, label, app.getString(R.string.deck_widget_no_answer)))
            }
        }
        bounded("deck-widget-press", onEnd = { pressing.remove(index) }, onTimeout = noAnswer) {
            val answer = ask(app) { RustCore.pressDeck(host.address, host.controlPort, index, label) }
            val outcome = answer.mapCatching { Deck.outcome(it) }.getOrNull()
            val words = when (DeckWidgetPlan.said(outcome)) {
                DeckWidgetPlan.Said.Done -> app.getString(R.string.deck_done)
                DeckWidgetPlan.Said.Waiting -> app.getString(R.string.deck_waiting)
                DeckWidgetPlan.Said.Refused -> app.plainErrorText(outcome?.reason)
                DeckWidgetPlan.Said.Failed -> app.plainErrorText(answer.exceptionOrNull()?.message)
            }
            asking = null
            line(app, host.address, app.getString(R.string.deck_widget_said, label, words))
        }
    }

    /**
     * The bridge's [request], with the bridge started first: a widget's tap
     * may start the process, which then has no identity or pins until it is
     * (G3 review). An Error from the library is a failure, said on the line.
     */
    private fun ask(app: Context, request: () -> Result<String>): Result<String> = try {
        RustCore.start(app)
        request()
    } catch (e: Throwable) {
        Result.failure(e)
    }

    /**
     * Run [work] on a thread under the broadcast's goAsync, waiting at most
     * [ASK_SECONDS], then [onTimeout] if it still runs; an Error is caught,
     * never the app's crash. [work] draws what it heard itself, so a late
     * answer is drawn too.
     */
    private fun bounded(name: String, onEnd: () -> Unit = {}, onTimeout: () -> Unit, work: () -> Unit) {
        val pending = goAsync()
        thread(name = name) {
            try {
                val worker = thread(name = "$name-ask") {
                    try {
                        work()
                    } catch (e: Throwable) {
                        Log.w(TAG, "deck widget: $name failed", e)
                    } finally {
                        onEnd()
                    }
                }
                worker.join(TimeUnit.SECONDS.toMillis(ASK_SECONDS))
                // Said, not kept as "Asking…" for good (G3 review).
                if (worker.isAlive) onTimeout()
            } catch (e: Exception) {
                Log.w(TAG, "deck widget: $name failed", e)
            } finally {
                pending.finish()
            }
        }
    }

    /** The last computer's list and line, kept between draws. */
    private class Kept(val json: String?, val failed: Boolean, val line: String?) {
        companion object {
            private const val FILE = "deck_widget"
            private const val KEY_FOR = "for"
            private const val KEY_JSON = "json"
            private const val KEY_FAILED = "failed"
            private const val KEY_LINE = "line"

            private fun prefs(ctx: Context) = ctx.getSharedPreferences(FILE, Context.MODE_PRIVATE)

            @Synchronized
            fun read(ctx: Context, lastComputer: String?): Kept {
                val p = prefs(ctx)
                val listFor = p.getString(KEY_FOR, null)
                if (!DeckWidgetPlan.keeps(listFor, lastComputer)) {
                    // Another computer's list goes, so its buttons never press this one's.
                    if (listFor != null && lastComputer != null) p.edit { clear() }
                    return Kept(null, false, null)
                }
                // A list that no longer reads counts as a failure, so it is asked again.
                val json = p.getString(KEY_JSON, null)
                val readable = json?.takeIf { runCatching { Deck.parse(it) }.isSuccess }
                val failed = p.getBoolean(KEY_FAILED, false) || (json != null && readable == null)
                return Kept(readable, failed, p.getString(KEY_LINE, null))
            }

            @Synchronized
            fun save(ctx: Context, computer: String, json: String) {
                if (last(ctx, computer)) store(ctx, computer, json)
            }

            private fun store(ctx: Context, computer: String, json: String) = prefs(ctx).edit {
                putString(KEY_FOR, computer)
                putString(KEY_JSON, json)
                putBoolean(KEY_FAILED, false)
                remove(KEY_LINE)
            }

            /** Whether [computer] is still the last one: a late answer for another never writes. */
            private fun last(ctx: Context, computer: String) = HostStore.lastHost(ctx)?.address == computer

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

            @Synchronized
            fun line(ctx: Context, computer: String, line: String) {
                if (!last(ctx, computer)) return
                val p = prefs(ctx)
                p.edit {
                    if (p.getString(KEY_FOR, null) != computer) {
                        clear()
                        putString(KEY_FOR, computer)
                    }
                    putString(KEY_LINE, line)
                }
            }
        }
    }

    companion object {
        private const val TAG = "DeckWidget"
        private const val ACTION_REFRESH = "dev.pocketdesk.android.action.DECK_WIDGET_REFRESH"
        private const val ACTION_FOLLOW = "dev.pocketdesk.android.action.DECK_WIDGET_FOLLOW"
        private const val ACTION_PRESS = "dev.pocketdesk.android.action.DECK_WIDGET_PRESS"
        private const val EXTRA_COMPUTER = "computer"
        private const val EXTRA_INDEX = "index"
        private const val EXTRA_LABEL = "label"
        private const val ASK_SECONDS = 8L

        /** The newest list ask in this process: only it keeps what it heard. */
        private val generation = AtomicLong()

        /**
         * What an ask under way says on the line ("Asking the computer…"),
         * in memory only: a process that dies mid-ask leaves nothing stuck
         * (G3 review).
         */
        @Volatile
        private var asking: String? = null

        /** The buttons whose press is running. */
        private val pressing: MutableSet<Int> = ConcurrentHashMap.newKeySet()

        private val SLOT_IDS = intArrayOf(
            R.id.widget_deck_b0, R.id.widget_deck_b1, R.id.widget_deck_b2,
            R.id.widget_deck_b3, R.id.widget_deck_b4, R.id.widget_deck_b5,
        )

        /** The deck sheet's list (F1), kept for the widget too. */
        fun keep(context: Context, computer: String, json: String) {
            val app = context.applicationContext
            if (HostStore.lastHost(app)?.address != computer) return
            synchronized(generation) {
                generation.incrementAndGet()
                Kept.save(app, computer, json)
            }
            redrawAll(app)
        }

        /** The last computer may have changed: ask once when it was never asked. */
        fun follow(context: Context) {
            context.sendBroadcast(Intent(context, DeckWidget::class.java).setAction(ACTION_FOLLOW))
        }

        private fun showAsking(app: Context, text: String) {
            asking = text
            redrawAll(app)
        }

        private fun line(app: Context, computer: String, text: String) {
            Kept.line(app, computer, text)
            redrawAll(app)
        }

        private fun redrawAll(app: Context) {
            val manager = AppWidgetManager.getInstance(app) ?: return
            val ids = manager.getAppWidgetIds(ComponentName(app, DeckWidget::class.java))
            if (ids.isNotEmpty()) redraw(app, manager, ids)
        }

        private fun broadcast(context: Context, code: Int, intent: Intent): PendingIntent =
            PendingIntent.getBroadcast(
                context,
                code,
                intent.setClass(context, DeckWidget::class.java),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )

        /** Draw every deck widget from what is kept, asking nothing. */
        fun redraw(context: Context, manager: AppWidgetManager, ids: IntArray) {
            val host = HostStore.lastHost(context)
            val kept = Kept.read(context, host?.address)
            val buttons = kept.json?.let { runCatching { Deck.parse(it) }.getOrNull() }
            val views = RemoteViews(context.packageName, R.layout.widget_deck)
            views.setTextViewText(
                R.id.widget_deck_host,
                host?.let { HostStore.shownName(context, it.address) } ?: context.getString(R.string.tile_no_computer),
            )
            val line = asking?.takeIf { host != null } ?: kept.line ?: when {
                host == null -> null
                buttons == null -> context.getString(R.string.deck_loading)
                buttons.isEmpty() -> context.getString(R.string.deck_none)
                else -> null
            }
            views.setTextViewText(R.id.widget_deck_line, line.orEmpty())
            views.setViewVisibility(R.id.widget_deck_line, if (line == null) View.GONE else View.VISIBLE)
            DeckWidgetPlan.slots(buttons.orEmpty()).forEachIndexed { slot, button ->
                val id = SLOT_IDS[slot]
                if (button == null || host == null) {
                    views.setViewVisibility(id, View.INVISIBLE)
                } else {
                    views.setViewVisibility(id, View.VISIBLE)
                    views.setTextViewText(id, button.label)
                    views.setContentDescription(id, button.label)
                    views.setOnClickPendingIntent(
                        id,
                        broadcast(
                            context,
                            1 + slot,
                            Intent(ACTION_PRESS)
                                .putExtra(EXTRA_COMPUTER, host.address)
                                .putExtra(EXTRA_INDEX, button.index)
                                .putExtra(EXTRA_LABEL, button.label),
                        ),
                    )
                }
            }
            views.setViewVisibility(R.id.widget_deck_refresh, if (host == null) View.GONE else View.VISIBLE)
            views.setOnClickPendingIntent(R.id.widget_deck_refresh, broadcast(context, 0, Intent(ACTION_REFRESH)))
            // With no computer saved, a tap opens the app to add one.
            if (host == null) {
                views.setOnClickPendingIntent(
                    R.id.widget_deck_root,
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
