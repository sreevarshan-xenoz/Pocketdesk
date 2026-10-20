package dev.pocketdesk.android.shortcuts

import android.appwidget.AppWidgetManager
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.ShortcutInfo
import android.content.pm.ShortcutManager
import android.graphics.drawable.Icon
import android.service.quicksettings.TileService
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.MainActivity
import dev.pocketdesk.android.R

/**
 * Shortcuts outside the app (F11): launcher shortcuts, the Quick Settings
 * tile and the home-screen widget all open [MainActivity] with one of these
 * actions, naming a saved computer by its address.
 */
object Shortcuts {
    const val ACTION_CONNECT = "dev.pocketdesk.android.action.CONNECT"
    const val ACTION_WAKE = "dev.pocketdesk.android.action.WAKE"
    const val EXTRA_ADDRESS = "dev.pocketdesk.android.extra.ADDRESS"

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
                ?.reportShortcutUsed(
                    ShortcutPlan.Entry(ShortcutPlan.Kind.CONNECT, address, dev.pocketdesk.android.Prefs.shareToken(ctx)).id,
                )
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
            hasWake = { dev.pocketdesk.android.wake.Waker.canWake(dev.pocketdesk.android.wake.Waker.planFor(app, it)) },
            token = dev.pocketdesk.android.Prefs.shareToken(app),
        )
        val wake = ShortcutPlan.Kind.WAKE
        // Its name tells computers apart, else its address (U3); two of one
        // name get their addresses too (U3 review). The icon tells connect
        // from wake.
        val names = plan.associate { it.address to HostStore.shownName(app, it.address) }
        val shared = names.values.groupingBy { it }.eachCount().filterValues { it > 1 }.keys
        val shortcuts = plan.map { entry ->
            val name = names.getValue(entry.address)
            val shown = if (name in shared) "$name (${entry.address})" else name
            ShortcutInfo.Builder(app, entry.id)
                .setShortLabel(shown)
                .setLongLabel(
                    app.getString(
                        if (entry.kind == wake) R.string.shortcut_wake else R.string.shortcut_connect,
                        shown,
                    ),
                )
                .setIcon(
                    Icon.createWithResource(
                        app,
                        if (entry.kind == wake) R.drawable.ic_shortcut_wake else R.drawable.ic_shortcut_connect,
                    ),
                )
                .setIntent(intent(app, entry.kind, entry.address))
                .apply {
                    // N2: a computer to share to from the share sheet.
                    if (entry.kind != wake) {
                        setCategories(setOf(ShortcutPlan.SHARE_CATEGORY))
                        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.R) setLongLived(true)
                    }
                }
                .build()
        }
        val manager = app.getSystemService(ShortcutManager::class.java)
        // A launcher may refuse (rate limits for an app in the background);
        // the next refresh catches up.
        runCatching { manager?.dynamicShortcuts = shortcuts }
        // A shortcut pinned to the home screen outlives the list: one for a
        // computer that was removed is disabled (a computer saved later at
        // the same address must not inherit it), and enabled again if the
        // computer is saved again.
        runCatching {
            val saved = HostStore.hosts(app).map { it.address }.toSet()
            val (keep, gone) = manager?.pinnedShortcuts.orEmpty().map { it.id }
                .partition { id -> ShortcutPlan.addressOf(id) in saved }
            if (gone.isNotEmpty()) manager?.disableShortcuts(gone, app.getString(R.string.shortcut_not_saved))
            if (keep.isNotEmpty()) manager?.enableShortcuts(keep)
        }
        // Not every device has app widgets.
        AppWidgetManager.getInstance(app)?.let { widgets ->
            val ids = widgets.getAppWidgetIds(ComponentName(app, ConnectWidget::class.java))
            if (ids.isNotEmpty()) ConnectWidget.update(app, widgets, ids)
            // G1: the state widget follows the last computer too.
            val stateIds = widgets.getAppWidgetIds(ComponentName(app, StateWidget::class.java))
            if (stateIds.isNotEmpty()) StateWidget.follow(app)
            // G3: the deck widget follows the last computer too.
            if (widgets.getAppWidgetIds(ComponentName(app, DeckWidget::class.java)).isNotEmpty()) DeckWidget.follow(app)
            // G5: and the media widget.
            if (widgets.getAppWidgetIds(ComponentName(app, MediaWidget::class.java)).isNotEmpty()) MediaWidget.follow(app)
        }
        // The tile reads the last computer each time the panel opens; this
        // only asks for a refresh where the system honours it.
        runCatching {
            TileService.requestListeningState(app, ComponentName(app, ConnectTileService::class.java))
        }
    }
}
