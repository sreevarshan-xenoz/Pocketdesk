package dev.pocketdesk.wear

import android.content.Context

/** [TilePlan.Line] in words, in the watch's language. */
object LineText {
    fun of(ctx: Context, line: TilePlan.Line): String? = when (line) {
        TilePlan.Line.Nothing -> null
        TilePlan.Line.Sent -> ctx.getString(R.string.said_sent)
        TilePlan.Line.Off -> ctx.getString(R.string.said_off)
        TilePlan.Line.Busy -> ctx.getString(R.string.said_busy)
        TilePlan.Line.Unreachable -> ctx.getString(R.string.said_unreachable)
        TilePlan.Line.NotPaired -> ctx.getString(R.string.said_not_paired)
        TilePlan.Line.NoBluetooth -> ctx.getString(R.string.said_no_bluetooth)
        TilePlan.Line.NeedsLockSet -> ctx.getString(R.string.said_set_lock)
        TilePlan.Line.NeedsUnlock -> ctx.getString(R.string.said_locked)
        TilePlan.Line.Failed -> ctx.getString(R.string.said_failed)
        TilePlan.Line.Answered -> ctx.getString(R.string.said_answered)
        // The phone's own few words (English: they come from the phone, fixed).
        is TilePlan.Line.Why -> line.text
    }

    fun label(ctx: Context, chip: TilePlan.Chip): String = when (chip.label) {
        TilePlan.Label.Wake -> ctx.getString(R.string.tile_wake)
        TilePlan.Label.Ring -> ctx.getString(R.string.tile_ring)
        TilePlan.Label.PlayPause -> ctx.getString(R.string.tile_play)
        TilePlan.Label.Next -> ctx.getString(R.string.tile_next)
        TilePlan.Label.Deck -> ctx.getString(R.string.tile_deck)
        TilePlan.Label.Deny -> ctx.getString(R.string.tile_deny, chip.askText.orEmpty().take(14))
    }
}
