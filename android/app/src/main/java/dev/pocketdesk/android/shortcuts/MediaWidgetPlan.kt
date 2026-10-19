package dev.pocketdesk.android.shortcuts

import dev.pocketdesk.android.ui.NowPlaying

/**
 * The media widget's choices (G5), kept apart from Android so they are
 * tested: which buttons show, what play/pause says, when to ask the
 * computer and whose answer is kept.
 */
object MediaWidgetPlan {
    /** The widget's three buttons. */
    enum class Button { Previous, PlayPause, Next }

    /** Which buttons the player offers now. */
    data class Buttons(val previous: Boolean, val playPause: Boolean, val next: Boolean)

    fun buttons(playing: NowPlaying?): Buttons = Buttons(
        previous = playing?.canPrevious == true,
        playPause = playing?.canPlayPause == true,
        next = playing?.canNext == true,
    )

    /** What a tap on play/pause does now. */
    enum class Toggle { Play, Pause }

    fun toggle(playing: NowPlaying?): Toggle = if (playing?.isPlaying == true) Toggle.Pause else Toggle.Play

    /** The computer's command for [button] (T8c's `mediaControl`). */
    fun command(button: Button): String = when (button) {
        Button.Previous -> "previous"
        Button.PlayPause -> "playPause"
        Button.Next -> "next"
    }

    /** The button a command names, or null for one the widget never sends. */
    fun button(command: String?): Button? = Button.entries.firstOrNull { command(it) == command }

    /** Why the widget is being drawn. */
    enum class Why { Refresh, Follow, Period }

    /**
     * Whether to ask what plays: for ↻, or when the last computer was never
     * [tried]. Never by the system's period: music changes by the minute.
     */
    fun ask(why: Why, tried: Boolean): Boolean = why == Why.Refresh || !tried

    /** A kept answer is shown only while its computer is still the last one. */
    fun keeps(readFor: String?, lastComputer: String?): Boolean =
        readFor != null && readFor == lastComputer
}
