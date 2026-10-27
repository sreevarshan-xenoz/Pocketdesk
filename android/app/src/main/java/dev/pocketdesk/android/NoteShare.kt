package dev.pocketdesk.android

import android.content.ComponentName
import android.content.Context
import android.content.pm.PackageManager
import dev.pocketdesk.android.ui.plainErrorText

/**
 * "Note to computer" in the share sheet (BF4): its entry, turned on or off
 * from Settings, and the words for how a note went.
 */
object NoteShare {
    /** The manifest's "Note to computer" alias of the share screen. */
    const val ALIAS = "dev.pocketdesk.android.NoteToComputer"

    private fun component(ctx: Context) = ComponentName(ctx.packageName, ALIAS)

    /** Whether the share sheet offers it; on until turned off. */
    fun shown(ctx: Context): Boolean =
        ctx.packageManager.getComponentEnabledSetting(component(ctx)) != PackageManager.COMPONENT_ENABLED_STATE_DISABLED

    fun setShown(ctx: Context, shown: Boolean) {
        ctx.packageManager.setComponentEnabledSetting(
            component(ctx),
            if (shown) PackageManager.COMPONENT_ENABLED_STATE_DEFAULT else PackageManager.COMPONENT_ENABLED_STATE_DISABLED,
            PackageManager.DONT_KILL_APP,
        )
    }

    /** How a note to [computer] went, from the bridge's answer ([error] null: kept). */
    fun words(ctx: Context, computer: String, error: String?): String = when (NoteRule.outcome(error)) {
        NoteRule.Outcome.Saved -> ctx.getString(R.string.note_saved, computer)
        NoteRule.Outcome.NoFile -> ctx.getString(R.string.note_no_file, computer)
        NoteRule.Outcome.Empty -> ctx.getString(R.string.share_nothing)
        NoteRule.Outcome.NotWritten -> ctx.getString(R.string.note_not_written, computer)
        NoteRule.Outcome.Failed -> ctx.getString(R.string.share_failed, ctx.plainErrorText(error))
    }
}

/** How a note went, from the bridge's answer; kept apart from Android so it is tested. */
object NoteRule {
    enum class Outcome { Saved, NoFile, Empty, NotWritten, Failed }

    fun outcome(error: String?): Outcome = when (error?.trim()) {
        null -> Outcome.Saved
        "no_file" -> Outcome.NoFile
        "empty" -> Outcome.Empty
        "not_written" -> Outcome.NotWritten
        else -> Outcome.Failed
    }
}
