package dev.pocketdesk.wear

import android.content.Context
import androidx.concurrent.futures.CallbackToFutureAdapter
import androidx.wear.protolayout.ActionBuilders
import androidx.wear.protolayout.ColorBuilders.argb
import androidx.wear.protolayout.DimensionBuilders.dp
import androidx.wear.protolayout.DimensionBuilders.expand
import androidx.wear.protolayout.DimensionBuilders.sp
import androidx.wear.protolayout.DimensionBuilders.wrap
import androidx.wear.protolayout.LayoutElementBuilders
import androidx.wear.protolayout.ModifiersBuilders
import androidx.wear.protolayout.ResourceBuilders
import androidx.wear.protolayout.TimelineBuilders
import androidx.wear.tiles.RequestBuilders
import androidx.wear.tiles.TileBuilders
import androidx.wear.tiles.TileService
import com.google.common.util.concurrent.ListenableFuture
import java.util.concurrent.Executors

/**
 * The tile: the buttons the phone has on (wake, ring the phone, play or
 * pause, next, a question's No, and Deck, which opens the short list) and
 * one line saying how the last tap went. A tap is a load action: the watch
 * asks for the tile again with the clickable's id, and the request makes one
 * Bluetooth connection to the phone, does the tap there (behind the watch's
 * own lock where [WatchLockRule] says so) and draws what the phone has on.
 * With the phone out of reach it says so within seconds and keeps nothing to
 * send later.
 */
class PocketdeskTile : TileService() {
    override fun onTileRequest(requestParams: RequestBuilders.TileRequest): ListenableFuture<TileBuilders.Tile> {
        val clicked = requestParams.currentState.lastClickableId.takeIf { it.isNotEmpty() }
        val app = applicationContext
        // The last tap comes back each time the tile is shown: acted on once.
        val prefs = app.getSharedPreferences("pocketdesk-tile", Context.MODE_PRIVATE)
        val guard = TilePlan.ClickGuard({ prefs.getString("last", null) }, { prefs.edit().putString("last", it).apply() })
        val act = if (guard.fresh(clicked)) TilePlan.act(clicked) else null
        // One drawing, one nonce: its taps are new ids.
        val nonce = TilePlan.nonce()
        return CallbackToFutureAdapter.getFuture { completer ->
            worker.execute {
                val tile = try {
                    val run = Phone.run(app, act)
                    tile(app, TileView.layout(app, run, nonce))
                } catch (e: Throwable) {
                    // Never a tile that stays loading for good.
                    tile(app, TileView.layout(app, Run(TilePlan.Line.Failed, Result.failure(e)), nonce))
                }
                completer.set(tile)
            }
            "pocketdesk tile"
        }
    }

    override fun onTileResourcesRequest(requestParams: RequestBuilders.ResourcesRequest): ListenableFuture<ResourceBuilders.Resources> =
        CallbackToFutureAdapter.getFuture { completer ->
            completer.set(ResourceBuilders.Resources.Builder().setVersion(RESOURCES).build())
            "pocketdesk tile resources"
        }

    private fun tile(ctx: Context, root: LayoutElementBuilders.LayoutElement): TileBuilders.Tile =
        TileBuilders.Tile.Builder()
            .setResourcesVersion(RESOURCES)
            // Asked again each time it is shown: it holds no data that would go stale unseen.
            .setFreshnessIntervalMillis(0)
            .setTileTimeline(TimelineBuilders.Timeline.fromLayoutElement(root))
            .build()

    private companion object {
        const val RESOURCES = "1"
        val worker = Executors.newSingleThreadExecutor { r -> Thread(r, "pocketdesk-tile").apply { isDaemon = true } }
    }
}

/** The tile's layout from what a run came to; no Android view classes, only protolayout. */
internal object TileView {
    private const val CHIP = 0xFF2C2C2E.toInt()
    private const val CHIP_DENY = 0xFF5A2A2A.toInt()
    private const val TEXT = 0xFFFFFFFF.toInt()
    private const val DIM = 0xFFB0B0B8.toInt()

    fun layout(ctx: Context, run: Run, nonce: String): LayoutElementBuilders.LayoutElement {
        val look = run.look.getOrNull()
        // The tap's own line, else why there is nothing to show.
        val said = LineText.of(ctx, run.line)
            ?: if (look == null) LineText.of(ctx, Phone.failed(run.look.exceptionOrNull())) else null
        val chips = look?.let { TilePlan.chips(it.buttons, it.asks) }.orEmpty()
        val column = LayoutElementBuilders.Column.Builder()
            .setWidth(expand())
            .setHeight(expand())
            .setHorizontalAlignment(LayoutElementBuilders.HORIZONTAL_ALIGN_CENTER)
            .addContent(text(look?.computer?.ifBlank { null } ?: ctx.getString(R.string.tile_label), 12f, DIM))
        if (look != null && chips.isEmpty()) {
            column.addContent(text(ctx.getString(R.string.tile_nothing_on), 12f, TEXT))
        }
        for (pair in chips.chunked(2)) {
            val row = LayoutElementBuilders.Row.Builder().setWidth(expand()).setHeight(wrap())
            for (chip in pair) row.addContent(chipBox(ctx, chip, nonce))
            column.addContent(row.build())
        }
        if (said != null) column.addContent(text(said, 11f, DIM))
        return column.build()
    }

    // The overflow constant is deprecated in Java and has no other form in this version.
    @Suppress("DEPRECATION")
    private fun text(text: String, size: Float, color: Int): LayoutElementBuilders.LayoutElement =
        LayoutElementBuilders.Text.Builder()
            .setText(text)
            .setMaxLines(2)
            .setOverflow(LayoutElementBuilders.TEXT_OVERFLOW_ELLIPSIZE_END)
            .setFontStyle(
                LayoutElementBuilders.FontStyle.Builder()
                    .setSize(sp(size))
                    .setColor(argb(color))
                    .build(),
            )
            .build()

    private fun chipBox(ctx: Context, chip: TilePlan.Chip, nonce: String): LayoutElementBuilders.LayoutElement {
        val action: ActionBuilders.Action = if (chip.opensList) {
            ActionBuilders.LaunchAction.Builder()
                .setAndroidActivity(
                    ActionBuilders.AndroidActivity.Builder()
                        .setPackageName(ctx.packageName)
                        .setClassName(MainActivity::class.java.name)
                        .build(),
                )
                .build()
        } else {
            ActionBuilders.LoadAction.Builder().build()
        }
        val inner = LayoutElementBuilders.Box.Builder()
            .setWidth(expand())
            .setHeight(dp(36f))
            .setHorizontalAlignment(LayoutElementBuilders.HORIZONTAL_ALIGN_CENTER)
            .setVerticalAlignment(LayoutElementBuilders.VERTICAL_ALIGN_CENTER)
            .setModifiers(
                ModifiersBuilders.Modifiers.Builder()
                    .setClickable(ModifiersBuilders.Clickable.Builder().setId(TilePlan.clickId(chip.id, nonce)).setOnClick(action).build())
                    .setBackground(
                        ModifiersBuilders.Background.Builder()
                            .setColor(argb(if (chip.label == TilePlan.Label.Deny) CHIP_DENY else CHIP))
                            .setCorner(ModifiersBuilders.Corner.Builder().setRadius(dp(18f)).build())
                            .build(),
                    )
                    .build(),
            )
            .addContent(text(LineText.label(ctx, chip), 12f, TEXT))
            .build()
        // A margin of its own: the chips are not shoulder to shoulder.
        return LayoutElementBuilders.Box.Builder()
            .setWidth(expand())
            .setHeight(wrap())
            .setModifiers(
                ModifiersBuilders.Modifiers.Builder()
                    .setPadding(ModifiersBuilders.Padding.Builder().setAll(dp(2f)).build())
                    .build(),
            )
            .addContent(inner)
            .build()
    }
}
