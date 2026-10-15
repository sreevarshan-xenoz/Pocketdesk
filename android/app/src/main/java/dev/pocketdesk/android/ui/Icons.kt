package dev.pocketdesk.android.ui

import androidx.annotation.DrawableRes
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.size
import androidx.compose.material3.LocalContentColor
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import dev.pocketdesk.android.R

/**
 * The app's icon set. Material icons-extended is not in this build's offline
 * dependency cache, so every glyph is a hand-authored VectorDrawable under
 * res/drawable/ic_ll_*.xml (Material path data on a 24x24 viewport, black
 * fill) referenced by resource id. The framework renders these natively —
 * the same proven path as the notification icons — and [LlIcon] tints them
 * at runtime via a color filter. If material-icons-extended becomes
 * available, delete this file and swap the call sites 1:1 to Icons.X.
 */
@JvmInline
value class LlGlyph internal constructor(@DrawableRes internal val resId: Int)

object LlIcons {
    val MoreVert = LlGlyph(R.drawable.ic_ll_more_vert)
    val Close = LlGlyph(R.drawable.ic_ll_close)
    val Check = LlGlyph(R.drawable.ic_ll_check)
    val ChevronRight = LlGlyph(R.drawable.ic_ll_chevron_right)
    val ArrowBack = LlGlyph(R.drawable.ic_ll_arrow_back)
    val Settings = LlGlyph(R.drawable.ic_ll_settings)
    val Add = LlGlyph(R.drawable.ic_ll_add)
    val Delete = LlGlyph(R.drawable.ic_ll_delete)
    val Desktop = LlGlyph(R.drawable.ic_ll_desktop)
    val Monitor = LlGlyph(R.drawable.ic_ll_desktop)
    val Keyboard = LlGlyph(R.drawable.ic_ll_keyboard)
    val Mouse = LlGlyph(R.drawable.ic_ll_mouse)
    val Touch = LlGlyph(R.drawable.ic_ll_touch)
    val Wifi = LlGlyph(R.drawable.ic_ll_wifi)
    val Lan = LlGlyph(R.drawable.ic_ll_lan)
    val Lock = LlGlyph(R.drawable.ic_ll_lock)
    val Power = LlGlyph(R.drawable.ic_ll_power)
    val Volume = LlGlyph(R.drawable.ic_ll_volume)
    val Mic = LlGlyph(R.drawable.ic_ll_mic)
    val Pin = LlGlyph(R.drawable.ic_ll_pin)
    val Notifications = LlGlyph(R.drawable.ic_ll_notifications)
    val Clipboard = LlGlyph(R.drawable.ic_ll_clipboard)
    val Eye = LlGlyph(R.drawable.ic_ll_eye)
    val Language = LlGlyph(R.drawable.ic_ll_language)
    val Sun = LlGlyph(R.drawable.ic_ll_sun)
    val Moon = LlGlyph(R.drawable.ic_ll_moon)
    val History = LlGlyph(R.drawable.ic_ll_history)
    val Pip = LlGlyph(R.drawable.ic_ll_pip)
    val Info = LlGlyph(R.drawable.ic_ll_info)
    val Warning = LlGlyph(R.drawable.ic_ll_warning)
    val Folder = LlGlyph(R.drawable.ic_ll_folder)
    val File = LlGlyph(R.drawable.ic_ll_file)
    val Play = LlGlyph(R.drawable.ic_ll_play)
    val Pause = LlGlyph(R.drawable.ic_ll_pause)
    val SkipNext = LlGlyph(R.drawable.ic_ll_skip_next)
    val SkipPrevious = LlGlyph(R.drawable.ic_ll_skip_previous)
}

/**
 * Draws one of [LlIcons] at [size], tinted with [tint] (or the ambient
 * content color when unset), like material3's Icon does for the library set.
 */
@Composable
fun LlIcon(
    glyph: LlGlyph,
    contentDescription: String?,
    modifier: Modifier = Modifier,
    tint: Color = Color.Unspecified,
    size: Dp = 24.dp,
) {
    val color = if (tint != Color.Unspecified) tint else LocalContentColor.current
    var mod = modifier.size(size)
    if (contentDescription != null) {
        mod = mod.semantics { this.contentDescription = contentDescription }
    }
    Image(
        painter = painterResource(glyph.resId),
        contentDescription = contentDescription,
        colorFilter = ColorFilter.tint(color),
        modifier = mod,
    )
}
