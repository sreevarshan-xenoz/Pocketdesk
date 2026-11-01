package dev.pocketdesk.wear

import dev.pocketdesk.watchlink.MusicCommand
import dev.pocketdesk.watchlink.WatchBodies
import dev.pocketdesk.watchlink.WatchButton
import dev.pocketdesk.watchlink.WatchOp
import dev.pocketdesk.watchlink.WatchStatus
import dev.pocketdesk.watchlink.WatchWire
import java.io.IOException

/**
 * What the tile shows and what a press on it means, kept apart from Android
 * and from the layout classes so it is tested: which chips the phone's
 * switches allow, how a clickable's id maps to a request, and what an
 * answer, or no answer, is called on the tile's line.
 */
object TilePlan {
    /** A clickable's id; a deny carries the question's own id, never its place in the list. */
    const val WAKE = "wake"
    const val RING = "ring"
    const val PLAY = "play"
    const val NEXT = "next"
    const val DENY = "deny:"

    /** What a tap on a clickable asks for. */
    sealed interface Act {
        data object Wake : Act
        data object Ring : Act
        data class Music(val command: MusicCommand) : Act
        data class Deny(val id: String) : Act

        /** A deck button, from the short list only (a tile's layout holds no list that changes). */
        data class Deck(val index: Int, val label: String) : Act

        val op: WatchOp
            get() = when (this) {
                Wake -> WatchOp.Wake
                Ring -> WatchOp.Ring
                is Music -> WatchOp.Music
                is Deny -> WatchOp.AskDeny
                is Deck -> WatchOp.DeckPress
            }
    }

    private val ASK_ID = Regex("^[A-Za-z0-9-]{1,64}$")

    /** The request a clickable's [id] stands for, or null for none (a stale or foreign id). */
    fun act(id: String?): Act? = when {
        id == null -> null
        id == WAKE -> Act.Wake
        id == RING -> Act.Ring
        id == PLAY -> Act.Music(MusicCommand.PlayPause)
        id == NEXT -> Act.Music(MusicCommand.Next)
        id.startsWith(DENY) -> id.removePrefix(DENY).takeIf { ASK_ID.matches(it) }?.let(Act::Deny)
        else -> null
    }

    enum class Label { Wake, Ring, PlayPause, Next, Deck, Deny }

    /** A chip: what it says, the clickable id it carries, and whether it opens the list instead of asking. */
    data class Chip(val label: Label, val id: String, val opensList: Boolean = false, val askText: String? = null)

    /**
     * The chips for the buttons the phone has on, [asks] being the questions
     * waiting. A Deck chip opens the short list, where the deck's buttons
     * are listed: a tile's layout holds no list that changes.
     */
    fun chips(buttons: Set<WatchButton>, asks: List<WatchBodies.Ask>): List<Chip> = buildList {
        if (WatchButton.Wake in buttons) add(Chip(Label.Wake, WAKE))
        if (WatchButton.Ring in buttons) add(Chip(Label.Ring, RING))
        if (WatchButton.Music in buttons) {
            add(Chip(Label.PlayPause, PLAY))
            add(Chip(Label.Next, NEXT))
        }
        if (WatchButton.Deck in buttons) add(Chip(Label.Deck, "deck", opensList = true))
        if (WatchButton.Asks in buttons) {
            // The first one: the rest are in the list.
            asks.firstOrNull()?.let { add(Chip(Label.Deny, DENY + it.id, askText = it.text)) }
        }
    }

    /** What an outcome is called on the tile's line. */
    sealed interface Line {
        data object Nothing : Line
        data object Sent : Line
        data object Off : Line
        data object Busy : Line
        data object Unreachable : Line
        data object NotPaired : Line
        data object NoBluetooth : Line
        data object NeedsLockSet : Line
        data object NeedsUnlock : Line
        data object Failed : Line
        data object Answered : Line

        /** The phone's own few words for why. */
        data class Why(val text: String) : Line
    }

    /** The line for the phone's [answer] to [act], or for no answer at all. */
    fun line(act: Act?, answer: Result<WatchWire.Response>): Line {
        if (act == null) return Line.Nothing
        val response = answer.getOrElse { return if (it is IOException) Line.Unreachable else Line.Failed }
        return when (response.status) {
            WatchStatus.Ok -> if (act is Act.Deny) Line.Answered else Line.Sent
            WatchStatus.Refused -> Line.Off
            WatchStatus.Busy -> Line.Busy
            WatchStatus.Failed -> WatchBodies.readWhy(response.body)?.takeIf { it.isNotBlank() }?.let(Line::Why) ?: Line.Failed
        }
    }
}
