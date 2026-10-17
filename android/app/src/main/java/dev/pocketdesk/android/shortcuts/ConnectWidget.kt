package dev.pocketdesk.android.shortcuts

import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.Context
import android.widget.RemoteViews
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.R

/**
 * Home-screen widget (F11): the last computer, one tap to connect to it
 * (or open the app when none is saved).
 */
class ConnectWidget : AppWidgetProvider() {
    override fun onUpdate(context: Context, manager: AppWidgetManager, ids: IntArray) {
        update(context, manager, ids)
    }

    companion object {
        fun update(context: Context, manager: AppWidgetManager, ids: IntArray) {
            val host = HostStore.lastHost(context)
            val views = RemoteViews(context.packageName, R.layout.widget_connect).apply {
                setTextViewText(
                    R.id.widget_host,
                    // U3: by its name, else its address.
                    host?.let { HostStore.shownName(context, it.address) }
                        ?: context.getString(R.string.tile_no_computer),
                )
                setTextViewText(
                    R.id.widget_action,
                    context.getString(if (host != null) R.string.widget_connect else R.string.widget_open),
                )
                setOnClickPendingIntent(
                    R.id.widget_root,
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
