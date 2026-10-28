package dev.pocketdesk.android.control

import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Path
import android.view.View

/**
 * The pointer the computer's mouse moves on this phone as a screen (BK5):
 * an arrow whose tip is the view's top-left corner, white edged in black so
 * it shows on any screen. It takes no touch: Android passes them under it.
 */
class PointerArrow(context: Context) : View(context) {
    private val density = context.resources.displayMetrics.density
    private val outline = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = 0xFF000000.toInt()
        style = Paint.Style.STROKE
        strokeWidth = 1.5f * density
        strokeJoin = Paint.Join.ROUND
    }
    private val fill = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = 0xFFFFFFFF.toInt()
        style = Paint.Style.FILL
    }

    /** The arrow, in dp from its tip: a pointer's usual shape. */
    private val arrow = Path().apply {
        val d = density
        moveTo(1f * d, 1f * d)
        lineTo(1f * d, 19f * d)
        lineTo(6f * d, 14.5f * d)
        lineTo(9.5f * d, 22f * d)
        lineTo(12.5f * d, 20.5f * d)
        lineTo(9f * d, 13.5f * d)
        lineTo(15f * d, 13.5f * d)
        close()
    }

    override fun onDraw(canvas: Canvas) {
        canvas.drawPath(arrow, fill)
        canvas.drawPath(arrow, outline)
    }

    companion object {
        /** The view's side, in dp: room for the arrow and its edge. */
        const val SIZE_DP = 24
    }
}
