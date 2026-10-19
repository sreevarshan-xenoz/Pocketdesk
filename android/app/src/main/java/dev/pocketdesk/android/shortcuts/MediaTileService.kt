package dev.pocketdesk.android.shortcuts

import android.app.PendingIntent
import android.os.Build
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.R

/**
 * Quick Settings tile (G6): plays or pauses the last computer's music,
 * through the media widget's own press (G5), whose answer it shows: on
 * while the computer plays, its subtitle the track or why not. It asks the
 * computer only when tapped; a locked phone is unlocked first.
 */
class MediaTileService : TileService() {
    override fun onStartListening() {
        super.onStartListening()
        val tile = qsTile ?: return
        val host = HostStore.lastHost(this)
        val latest = MediaWidget.latest(this)
        val look = MediaTilePlan.look(host != null, latest.playing, latest.failed, latest.asking)
        tile.label = getString(R.string.media_tile_label)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            tile.subtitle = when (look) {
                MediaTilePlan.Look.NoComputer -> getString(R.string.tile_no_computer)
                MediaTilePlan.Look.Asking -> getString(R.string.deck_loading)
                MediaTilePlan.Look.Failed -> latest.line ?: getString(R.string.media_tile_hint)
                MediaTilePlan.Look.Unknown -> getString(R.string.media_tile_hint)
                MediaTilePlan.Look.Paused, MediaTilePlan.Look.Playing -> latest.playing?.headline()
            }
        }
        tile.state = if (look.on) Tile.STATE_ACTIVE else Tile.STATE_INACTIVE
        tile.updateTile()
    }

    override fun onClick() {
        super.onClick()
        if (HostStore.lastHost(this) == null) {
            openApp()
            return
        }
        // Behind the phone's lock, as the deck's buttons are on the home screen.
        if (isLocked) unlockAndRun { MediaWidget.playPause(this) } else MediaWidget.playPause(this)
    }

    private fun openApp() {
        val intent = Shortcuts.connectLast(this)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            startActivityAndCollapse(
                PendingIntent.getActivity(this, 0, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT),
            )
        } else {
            @Suppress("DEPRECATION")
            @android.annotation.SuppressLint("StartActivityAndCollapseDeprecated")
            startActivityAndCollapse(intent)
        }
    }
}
