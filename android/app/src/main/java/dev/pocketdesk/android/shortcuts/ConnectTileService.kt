package dev.pocketdesk.android.shortcuts

import android.app.PendingIntent
import android.os.Build
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.R

/**
 * Quick Settings tile (F11): one tap connects to the last computer, or
 * opens the app when none is saved (the subtitle says so).
 */
class ConnectTileService : TileService() {
    override fun onStartListening() {
        super.onStartListening()
        val tile = qsTile ?: return
        val host = HostStore.lastHost(this)
        tile.label = getString(R.string.tile_label)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            tile.subtitle = host?.let { HostStore.shownName(this, it.address) } ?: getString(R.string.tile_no_computer)
        }
        // Inactive, not unavailable, with no computer saved: an unavailable
        // tile gets no clicks, and this one then opens the app.
        tile.state = Tile.STATE_INACTIVE
        tile.updateTile()
    }

    override fun onClick() {
        super.onClick()
        val intent = Shortcuts.connectLast(this)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            startActivityAndCollapse(
                PendingIntent.getActivity(
                    this,
                    0,
                    intent,
                    PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                ),
            )
        } else {
            // Android 13 and older have only the Intent form.
            @Suppress("DEPRECATION")
            @android.annotation.SuppressLint("StartActivityAndCollapseDeprecated")
            startActivityAndCollapse(intent)
        }
    }
}
