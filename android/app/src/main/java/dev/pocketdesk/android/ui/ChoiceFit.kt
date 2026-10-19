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
    /**
     * The words of [option], each of which must fit on a line: split at
     * spaces only, since a line never breaks at a no-break space (C5 review).
     */
    fun words(option: String): List<String> = option.split(' ').filter { it.isNotEmpty() }.ifEmpty { listOf(option) }

    /** Whether words [wordWidthsPx] wide all fit a segment [segmentWidthPx] wide inside its padding. */
    fun fits(segmentWidthPx: Float, wordWidthsPx: List<Float>): Boolean =
        segmentWidthPx > 0f && wordWidthsPx.all { it <= segmentWidthPx }

    /** How a row of choices is drawn. */
    enum class Layout {
        /** Each choice on one line, the chosen one with a check. */
        Segments,

        /** No check, narrower sides, a smaller face, two lines a choice. */
        Crowded,

        /** A list of radio buttons. */
        List,
    }

    /**
     * Up to three choices are segments when they fit, crowded when only
     * that fits, and a list when neither does (E6: three choices, such as
     * Direct touch, Trackpad and Pen, became a list at font scale 1.0).
     * Four are crowded or a list.
     */
    fun layout(options: Int, segmentsFit: Boolean, crowdedFit: Boolean): Layout = when {
        options <= 3 && segmentsFit -> Layout.Segments
        crowdedFit -> Layout.Crowded
        else -> Layout.List
    }
}
