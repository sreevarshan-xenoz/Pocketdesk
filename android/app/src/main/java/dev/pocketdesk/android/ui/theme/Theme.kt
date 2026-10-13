package dev.pocketdesk.android.ui.theme

import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.platform.LocalContext
import dev.pocketdesk.android.Prefs

private val LightScheme = lightColorScheme(
    primary = Pine40,
    onPrimary = OnPine,
    primaryContainer = PineContainer,
    onPrimaryContainer = OnPineContainer,
    secondary = SlateGreen30,
    onSecondary = OnSlateGreen,
    secondaryContainer = SlateGreenContainer,
    onSecondaryContainer = OnSlateGreenContainer,
    background = SurfaceLight,
    onBackground = OnSurfaceLight,
    surface = SurfaceLight,
    onSurface = OnSurfaceLight,
    surfaceVariant = SurfaceVariantLight,
    onSurfaceVariant = OnSurfaceVariantLight,
    outline = OutlineLight,
    error = ErrorLight,
    onError = OnErrorLight,
    errorContainer = ErrorContainerLight,
    onErrorContainer = OnErrorContainerLight,
)

private val DarkScheme = darkColorScheme(
    primary = Pine80,
    onPrimary = OnPineDark,
    primaryContainer = PineContainerDark,
    onPrimaryContainer = OnPineContainerDark,
    secondary = Pine80,
    onSecondary = OnPineDark,
    secondaryContainer = SlateGreenContainerDark,
    onSecondaryContainer = OnSlateGreenContainerDark,
    background = SurfaceDark,
    onBackground = OnSurfaceDark,
    surface = SurfaceDark,
    onSurface = OnSurfaceDark,
    surfaceVariant = SurfaceVariantDark,
    onSurfaceVariant = OnSurfaceVariantDark,
    outline = OutlineDark,
    error = ErrorDark,
    onError = OnErrorDark,
    errorContainer = ErrorContainerDark,
    onErrorContainer = OnErrorContainerDark,
)

/**
 * App theme: follows the system light/dark setting unless the user pins a
 * mode in Settings, and uses Material You wallpaper colors on Android 12+
 * when enabled. Reading [Prefs.observedVersion] re-composes on pref changes.
 */
@Composable
fun PocketdeskTheme(content: @Composable () -> Unit) {
    val context = LocalContext.current
    Prefs.observedVersion()

    val systemDark = isSystemInDarkTheme()
    val dark = when (Prefs.themeMode(context)) {
        Prefs.THEME_LIGHT -> false
        Prefs.THEME_DARK -> true
        else -> systemDark
    }
    val colorScheme: ColorScheme = if (Prefs.dynamicColor(context) &&
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.S
    ) {
        if (dark) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
    } else {
        if (dark) DarkScheme else LightScheme
    }

    MaterialTheme(
        colorScheme = colorScheme,
        typography = Typography,
        content = content,
    )
}
