package dev.pocketdesk.android.ui

/** Home-family destinations; plain strings so rememberSaveable just works. */
object Route {
    const val HOME = "home"
    const val FORM = "form"
    const val SETTINGS = "settings"
    const val SCAN = "scan"
}

/**
 * Where Android's Back goes from each Home-family screen (U2): up one level,
 * to Home. `null` on Home itself, where Back leaves the app as Android's
 * does. Before, only the session handled Back, so it left the app from
 * Settings, Add computer and the scanner.
 */
object Navigation {
    fun backTo(route: String): String? = when (route) {
        Route.HOME -> null
        else -> Route.HOME
    }
}
