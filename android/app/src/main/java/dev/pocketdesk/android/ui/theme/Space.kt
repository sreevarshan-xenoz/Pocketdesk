package dev.pocketdesk.android.ui.theme

import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/** Shared spacing/shape scale; screens should not hardcode one-off dp values. */
object Space {
    val xs: Dp = 4.dp
    val sm: Dp = 8.dp
    val md: Dp = 16.dp
    val lg: Dp = 24.dp
    val xl: Dp = 32.dp

    val chip = RoundedCornerShape(12.dp)
    val card = RoundedCornerShape(16.dp)
    val sheet = RoundedCornerShape(28.dp)
    val pill = RoundedCornerShape(50)
}
