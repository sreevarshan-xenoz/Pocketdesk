package dev.linuxlink.android.shortcuts

import android.appwidget.AppWidgetManager
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.ShortcutInfo
import android.content.pm.ShortcutManager
import android.graphics.drawable.Icon
import android.service.quicksettings.TileService
import dev.linuxlink.android.HostStore
import dev.linuxlink.android.MainActivity
import dev.linuxlink.android.R

/**
 * Shortcuts outside the app (F11): launcher shortcuts, the Quick Settings
 * tile and the home-screen widget all open [MainActivity] with one of these
 * actions, naming a saved computer by its address.
 */
object Shortcuts {
    const val ACTION_CONNECT = "dev.linuxlink.android.action.CONNECT"
    const val ACTION_WAKE = "dev.linuxlink.android.action.WAKE"
    const val EXTRA_ADDRESS = "dev.linuxlink.android.extra.ADDRESS"

    /** Open the app on [kind] for the computer at [address]. */
    fun intent(ctx: Context, kind: ShortcutPlan.Kind, address: String): Intent =
        Intent(ctx, MainActivity::class.java)
            .setAction(if (kind == ShortcutPlan.Kind.WAKE) ACTION_WAKE else ACTION_CONNECT)
            .putExtra(EXTRA_ADDRESS, address)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)

    /**
     * Connect to the last computer, or just open the app when none is
     * saved: what the tile and the widget do.
     */
    fun connectLast(ctx: Context): Intent =
        HostStore.lastHost(ctx)?.let { intent(ctx, ShortcutPlan.Kind.CONNECT, it.address) }
            ?: Intent(ctx, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

    /**
     * Tell the launcher the computer at [address] was connected to, however
     * it was opened, so its shortcut ranks with use.
     */
    fun reportConnected(ctx: Context, address: String) {
        runCatching {
            ctx.getSystemService(ShortcutManager::class.java)
                ?.reportShortcutUsed(ShortcutPlan.Entry(ShortcutPlan.Kind.CONNECT, address).id)
        }
    }

    /**
     * Bring every shortcut up to date with the saved computers: the
     * launcher's list, the widgets, and the tile's label.
     */
    fun refresh(ctx: Context) {
        val app = ctx.applicationContext
        val plan = ShortcutPlan.plan(
            HostStore.hosts(app),
            hasWake = { HostStore.wolMac(app, it).isNotBlank() },
        )
        val shortcuts = plan.map { entry ->
            val label = app.getString(
                if (entry.kind == ShortcutPlan.Kind.WAKE) R.string.shortcut_wake else R.string.shortcut_connect,
                entry.address,
            )
            ShortcutInfo.Builder(app, entry.id)
                .setShortLabel(label)
                .setLongLabel(label)
                .setIcon(
                    Icon.createWithResource(
                        app,
                        if (entry.kind == ShortcutPlan.Kind.WAKE) R.drawable.ic_ll_power else R.drawable.ic_stat_linux_link,
                    ),
                )
                .setIntent(intent(app, entry.kind, entry.address))
                .build()
        }
        // A launcher may refuse (rate limits for an app in the background);
        // the next refresh catches up.
        runCatching { app.getSystemService(ShortcutManager::class.java)?.dynamicShortcuts = shortcuts }
        val widgets = AppWidgetManager.getInstance(app)
        val ids = widgets.getAppWidgetIds(ComponentName(app, ConnectWidget::class.java))
        if (ids.isNotEmpty()) ConnectWidget.update(app, widgets, ids)
        runCatching {
            TileService.requestListeningState(app, ComponentName(app, ConnectTileService::class.java))
        }
    }
}
