package dev.pocketdesk.android.shortcuts

import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.net.ConnectivityManager
import android.text.format.DateUtils
import android.util.Log
import android.widget.RemoteViews
import androidx.core.content.edit
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.ui.ComputerState
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong
import kotlin.concurrent.thread

/**
 * Home-screen widget (G1): the last computer's CPU, memory, fullest disk
 * and battery, with when they were read. A tap asks again; the system's
 * period asks only while Settings lets it ([StateWidgetPlan] decides).
 */
class StateWidget : AppWidgetProvider() {
    override fun onUpdate(context: Context, manager: AppWidgetManager, ids: IntArray) {
        refresh(context, StateWidgetPlan.Why.Period)
    }

    override fun onReceive(context: Context, intent: Intent) {
        when (intent.action) {
            ACTION_REFRESH -> refresh(context, StateWidgetPlan.Why.Tap)
            // A new last computer: asked once, whatever the setting.
            ACTION_FOLLOW -> refresh(context, StateWidgetPlan.Why.Period, auto = false)
            else -> super.onReceive(context, intent)
        }
    }

    private fun refresh(context: Context, why: StateWidgetPlan.Why, auto: Boolean = Prefs.stateWidgetAuto(context)) {
        val app = context.applicationContext
        val manager = AppWidgetManager.getInstance(app)
        val ids = manager.getAppWidgetIds(ComponentName(app, StateWidget::class.java))
        if (ids.isEmpty()) return
        val host = HostStore.lastHost(app)
        val kept = Kept.read(app, host?.address)
        if (host == null || !StateWidgetPlan.ask(why, StateWidgetPlan.tried(kept.json, kept.failed), auto, metered(app))) {
            redraw(app, manager, ids)
            return
        }
        val pending = goAsync()
        // Only the newest ask writes what it heard (G1 review): an older,
        // slower one never overwrites it.
        val ask = generation.incrementAndGet()
        // Drawn at once, saying it asks, so a tap is seen (G1 review).
        askingFor = host.address
        redraw(app, manager, ids)
        // A broadcast has about 10 s: the answer or the failure is drawn by then.
        thread(name = "state-widget") {
            try {
                val asked = thread(name = "state-widget-ask") {
                    // An Error from the library is a failure, never the app's crash.
                    val answer = try {
                        RustCore.start(app)
                        RustCore.systemState(host.address, host.controlPort)
                    } catch (e: Throwable) {
                        Result.failure(e)
                    }
                    // An answer after the wait is drawn when it comes.
                    finish(app, host.address, ask, answer.getOrNull())
                }
                asked.join(TimeUnit.SECONDS.toMillis(ASK_SECONDS))
                if (asked.isAlive) finish(app, host.address, ask, null)
            } catch (e: Exception) {
                Log.w(TAG, "state widget refresh failed", e)
            } finally {
                pending.finish()
            }
        }
    }

    /** Whether the network now is metered (or there is none known). */
    private fun metered(app: Context): Boolean =
        app.getSystemService(ConnectivityManager::class.java)?.isActiveNetworkMetered ?: true

    /** Keep and draw what ask [ask] heard ([json], or null for nothing), if it is still the newest. */
    private fun finish(app: Context, computer: String, ask: Long, json: String?) {
        synchronized(generation) {
            if (generation.get() != ask) return
            if (json != null) Kept.save(app, computer, json) else Kept.failed(app, computer)
            askingFor = null
        }
        val manager = AppWidgetManager.getInstance(app)
        val ids = manager.getAppWidgetIds(ComponentName(app, StateWidget::class.java))
        if (ids.isNotEmpty()) redraw(app, manager, ids)
    }

    /** The last answer for one computer, kept between draws. */
    private class Kept(val json: String?, val readAt: Long?, val failed: Boolean) {
        companion object {
            private const val FILE = "state_widget"
            private const val KEY_FOR = "for"
            private const val KEY_JSON = "json"
            private const val KEY_AT = "at"
            private const val KEY_FAILED = "failed"

            private fun prefs(ctx: Context) = ctx.getSharedPreferences(FILE, Context.MODE_PRIVATE)

            fun read(ctx: Context, lastComputer: String?): Kept {
                val p = prefs(ctx)
                val readFor = p.getString(KEY_FOR, null)
                if (!StateWidgetPlan.keeps(readFor, lastComputer)) {
                    // Another computer's reading goes, so it never comes back stale (G1 review).
                    if (readFor != null && lastComputer != null) p.edit { clear() }
                    return Kept(null, null, false)
                }
                val at = p.getLong(KEY_AT, 0L)
                // A reading that no longer reads counts as a failure, so it is asked again (G1 review).
                val json = p.getString(KEY_JSON, null)
                val readable = json?.takeIf { runCatching { ComputerState.parse(it) }.isSuccess }
                return Kept(readable, at.takeIf { it > 0 }, p.getBoolean(KEY_FAILED, false) || (json != null && readable == null))
            }

            fun save(ctx: Context, computer: String, json: String) = prefs(ctx).edit {
                putString(KEY_FOR, computer)
                putString(KEY_JSON, json)
                putLong(KEY_AT, System.currentTimeMillis())
                putBoolean(KEY_FAILED, false)
            }

            fun failed(ctx: Context, computer: String) {
                val p = prefs(ctx)
                p.edit {
                    // A failure for another computer drops that one's reading.
                    if (p.getString(KEY_FOR, null) != computer) {
                        clear()
                        putString(KEY_FOR, computer)
                    }
                    putBoolean(KEY_FAILED, true)
                }
            }
        }
    }

    companion object {
        private const val TAG = "StateWidget"
        private const val ACTION_REFRESH = "dev.pocketdesk.android.action.STATE_WIDGET_REFRESH"
        private const val ACTION_FOLLOW = "dev.pocketdesk.android.action.STATE_WIDGET_FOLLOW"
        private const val ASK_SECONDS = 8L

        /** The newest ask in this process. */
        private val generation = AtomicLong()

        /** The computer an ask is running for in this process, if one is. */
        @Volatile
        private var askingFor: String? = null

        /**
         * The last computer may have changed: draw from what is kept, and
         * ask once when nothing is kept for it.
         */
        fun follow(context: Context) {
            context.sendBroadcast(Intent(context, StateWidget::class.java).setAction(ACTION_FOLLOW))
        }

        /** Draw every state widget from what is kept, asking nothing. */
        fun redraw(context: Context, manager: AppWidgetManager, ids: IntArray) {
            val host = HostStore.lastHost(context)
            val kept = Kept.read(context, host?.address)
            val state = kept.json?.let { runCatching { ComputerState.parse(it) }.getOrNull() }
            val views = RemoteViews(context.packageName, R.layout.widget_state).apply {
                setTextViewText(
                    R.id.widget_state_host,
                    host?.let { HostStore.shownName(context, it.address) } ?: context.getString(R.string.tile_no_computer),
                )
                setTextViewText(R.id.widget_state_numbers, numbers(context, state))
                setTextViewText(R.id.widget_state_when, timeLine(context, if (state == null) null else kept.readAt, kept.failed, host != null && askingFor == host.address))
                setOnClickPendingIntent(
                    R.id.widget_state_root,
                    if (host == null) {
                        PendingIntent.getActivity(
                            context,
                            0,
                            Shortcuts.connectLast(context),
                            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                        )
                    } else {
                        PendingIntent.getBroadcast(
                            context,
                            0,
                            Intent(context, StateWidget::class.java).setAction(ACTION_REFRESH),
                            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                        )
                    },
                )
            }
            manager.updateAppWidget(ids, views)
        }

        private fun numbers(context: Context, state: ComputerState.State?): String {
            if (state == null) return ""
            if (state.off) return context.getString(R.string.state_widget_not_shared)
            val n = StateWidgetPlan.numbers(state)
            return listOfNotNull(
                n.cpu?.let { context.getString(R.string.state_widget_cpu, it) },
                n.memory?.let { context.getString(R.string.state_widget_memory, it) },
                n.disk?.let { context.getString(R.string.state_widget_disk, it) },
                n.battery?.let { context.getString(R.string.state_widget_battery, it) },
            ).joinToString(" · ")
        }

        private fun timeLine(context: Context, readAt: Long?, failed: Boolean, asking: Boolean): String {
            // The time today, the date before (G1 review): a day-old reading never reads as now.
            val time = readAt?.let {
                DateUtils.formatSameDayTime(it, System.currentTimeMillis(), java.text.DateFormat.SHORT, java.text.DateFormat.SHORT)
                    .toString()
            }
            return when (StateWidgetPlan.timeLine(readAt, failed, asking)) {
                StateWidgetPlan.When.Updated -> context.getString(R.string.state_widget_updated, time)
                StateWidgetPlan.When.StaleSince -> context.getString(R.string.state_widget_stale, time)
                StateWidgetPlan.When.NotAnswering -> context.getString(R.string.state_widget_not_answering)
                StateWidgetPlan.When.Asking -> context.getString(R.string.state_loading)
            }
        }
    }
}
