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

    /** The paired id of the computer an intent was made for (AZ1). */
    const val EXTRA_PAIRED_ID = "dev.pocketdesk.android.extra.PAIRED_ID"

    /**
     * Open the app on [kind] for the computer at [address], and, once paired,
     * only for that computer, wherever it is then (AZ1).
     */
    fun intent(ctx: Context, kind: ShortcutPlan.Kind, address: String): Intent =
        Intent(ctx, MainActivity::class.java)
            .setAction(if (kind == ShortcutPlan.Kind.WAKE) ACTION_WAKE else ACTION_CONNECT)
            .putExtra(EXTRA_ADDRESS, address)
            .apply { HostStore.pairedDesktopId(ctx, address)?.let { putExtra(EXTRA_PAIRED_ID, it) } }
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
                    ShortcutPlan.Entry(
                        ShortcutPlan.Kind.CONNECT,
                        address,
                        dev.pocketdesk.android.Prefs.shareToken(ctx),
                        HostStore.pairedDesktopId(ctx, address),
                    ).id,
                )
        }
    }

    /** One shortcut: [kind] for the computer at [address], labelled [shown]. */
    private fun info(app: Context, id: String, kind: ShortcutPlan.Kind, address: String, shown: String): ShortcutInfo {
        val wake = kind == ShortcutPlan.Kind.WAKE
        // The icon tells connect from wake.
        return ShortcutInfo.Builder(app, id)
            .setShortLabel(shown)
            .setLongLabel(app.getString(if (wake) R.string.shortcut_wake else R.string.shortcut_connect, shown))
            .setIcon(
                Icon.createWithResource(app, if (wake) R.drawable.ic_shortcut_wake else R.drawable.ic_shortcut_connect),
            )
            .setIntent(intent(app, kind, address))
            .apply {
                // N2: a computer to share to from the share sheet.
                if (!wake) {
                    setCategories(setOf(ShortcutPlan.SHARE_CATEGORY))
                    if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.R) setLongLived(true)
                }
            }
            .build()
    }

    /**
     * Bring every shortcut up to date with the saved computers: the
     * launcher's list, the widgets, and the tile's label.
     */
    fun refresh(ctx: Context) {
        val app = ctx.applicationContext
        val hosts = HostStore.hosts(app)
        val pairedId = { address: String -> HostStore.pairedDesktopId(app, address) }
        val plan = ShortcutPlan.plan(
            hosts,
            hasWake = { dev.pocketdesk.android.wake.Waker.canWake(dev.pocketdesk.android.wake.Waker.planFor(app, it)) },
            token = dev.pocketdesk.android.Prefs.shareToken(app),
            pairedId = pairedId,
        )
        // Its name tells computers apart, else its address (U3); two of one
        // name get their addresses too (U3 review).
        val names = hosts.associate { it.address to HostStore.shownName(app, it.address) }
        val shared = names.values.groupingBy { it }.eachCount().filterValues { it > 1 }.keys

        fun shown(address: String): String {
            val name = names[address] ?: address
            return if (name in shared) "$name ($address)" else name
        }
        val shortcuts = plan.map { info(app, it.id, it.kind, it.address, shown(it.address)) }
        val manager = app.getSystemService(ShortcutManager::class.java)
        // A launcher may refuse (rate limits for an app in the background);
        // the next refresh catches up.
        runCatching { manager?.dynamicShortcuts = shortcuts }
        // A shortcut pinned to the home screen outlives the list: one for a
        // computer that was removed is disabled, and enabled again if the
        // computer is saved again; one for a computer that moved follows it
        // (AZ1: by address, a move turned it off and another computer saved
        // at a removed one's address took it over).
        runCatching {
            val pinned = manager?.pinnedShortcuts.orEmpty()
            val fates = pinned.associate {
                it.id to ShortcutPlan.pinnedFate(it.id, it.intent?.getStringExtra(EXTRA_ADDRESS), hosts, pairedId)
            }
            val (off, on) = fates.keys.partition { fates[it] == ShortcutPlan.Pinned.Off }
            if (off.isNotEmpty()) manager?.disableShortcuts(off, app.getString(R.string.shortcut_not_saved))
            if (on.isNotEmpty()) manager?.enableShortcuts(on)
            val moved = fates.mapNotNull { (id, fate) ->
                (fate as? ShortcutPlan.Pinned.Move)?.let {
                    info(app, id, ShortcutPlan.kindOf(id), it.host.address, shown(it.host.address))
                }
            }
            if (moved.isNotEmpty()) manager?.updateShortcuts(moved)
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
