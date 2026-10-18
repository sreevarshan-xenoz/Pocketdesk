package dev.pocketdesk.android.ui

/**
 * Whether a row of choices fits as segments (C5): at font scale 2.0 the
 * session menu's Quality row broke its words ("Balanc / ed"), a segment
 * being narrower than one word. A choice may wrap between words, never
 * inside one, so the row fits when every word of every option fits its
 * segment; otherwise it is drawn as a list. Kept apart from Android so it
 * is tested.
 */
object ChoiceFit {
    /** The words of [option], each of which must fit on a line. */
    fun words(option: String): List<String> = option.split(' ', '\u00A0').filter { it.isNotEmpty() }.ifEmpty { listOf(option) }

    /** Whether words [wordWidthsPx] wide all fit a segment [segmentWidthPx] wide inside its padding. */
    fun fits(segmentWidthPx: Float, wordWidthsPx: List<Float>): Boolean =
        segmentWidthPx > 0f && wordWidthsPx.all { it <= segmentWidthPx }
}
