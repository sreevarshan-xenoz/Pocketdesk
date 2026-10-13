package dev.pocketdesk.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.pocketdesk.android.R

/** Which of the computer's screens the session shows, kept apart from Compose so it is tested. */
object ScreenChoices {
    /**
     * The screen on show for the chosen [selected] index: `-1` (Auto) is the
     * primary screen, and so is a screen that is no longer there.
     */
    fun current(screens: List<DesktopMonitor>, selected: Int): Int? {
        screens.firstOrNull { it.index == selected }?.let { return it.index }
        return (screens.firstOrNull { it.isPrimary } ?: screens.firstOrNull())?.index
    }

    /** A switcher is worth its space only with more than one screen. */
    fun worthShowing(screens: List<DesktopMonitor>): Boolean = screens.size > 1

    /** Whether tapping [tapped] changes what is on show. */
    fun isSwitch(screens: List<DesktopMonitor>, selected: Int, tapped: Int): Boolean =
        current(screens, selected) != tapped
}

/**
 * One tap to another of the computer's screens, in the session's chrome:
 * a numbered chip per screen, the one on show highlighted. Shown only when
 * the computer has more than one screen.
 */
@Composable
fun ScreenSwitcher(
    screens: List<DesktopMonitor>,
    selected: Int,
    onPick: (Int) -> Unit,
    modifier: Modifier = Modifier,
) {
    if (!ScreenChoices.worthShowing(screens)) return
    val current = ScreenChoices.current(screens, selected)
    Row(
        modifier = modifier
            .clip(RoundedCornerShape(18.dp))
            .background(Color.White.copy(alpha = 0.16f))
            .padding(horizontal = 4.dp, vertical = 3.dp),
        horizontalArrangement = Arrangement.spacedBy(2.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        screens.forEachIndexed { position, screen ->
            val shown = screen.index == current
            val description = stringResource(
                R.string.screen_chip,
                position + 1,
                screen.width,
                screen.height,
            )
            Box(
                modifier = Modifier
                    .size(30.dp)
                    .clip(CircleShape)
                    .background(if (shown) MaterialTheme.colorScheme.primary else Color.Transparent)
                    .selectable(selected = shown, role = Role.Tab) { onPick(screen.index) }
                    .semantics { contentDescription = description },
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    "${position + 1}",
                    color = if (shown) MaterialTheme.colorScheme.onPrimary else Color.White,
                    fontSize = 13.sp,
                )
            }
        }
    }
}
