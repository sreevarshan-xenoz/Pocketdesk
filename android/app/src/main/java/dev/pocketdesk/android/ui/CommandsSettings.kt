package dev.pocketdesk.android.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.R
import dev.pocketdesk.android.automation.CommandRule
import dev.pocketdesk.android.automation.Commands

/**
 * Settings "Commands from the computer to my automation apps" (L6), off by
 * default, and which app hears them: one of those found, or any app.
 */
@Composable
fun CommandsSetting() {
    val context = LocalContext.current
    var version by remember { mutableIntStateOf(0) }
    val resumes = rememberResumes()
    val on = (version + resumes).let { Prefs.commandsFromComputer(context) }
    val installed = remember(resumes) { Commands.installed(context) }
    val to = (version + resumes).let { CommandRule.onlyTo(Prefs.commandsOnlyTo(context), installed) }
    SettingSwitch(
        stringResource(R.string.settings_commands_from_computer),
        stringResource(
            when {
                !on -> R.string.settings_commands_from_computer_summary
                to == null -> R.string.settings_commands_any_app
                else -> R.string.settings_commands_from_computer_summary
            },
        ),
        on,
        {
            Prefs.setCommandsFromComputer(context, it)
            version++
        },
        warning = on && to == null,
    )
    if (!on || installed.size < 2) return
    // A choice only where there is one to make.
    for (pkg in CommandRule.KNOWN.filter { it in installed }) {
        val label =
            runCatching {
                context.packageManager.getApplicationLabel(context.packageManager.getApplicationInfo(pkg, 0)).toString()
            }.getOrDefault(pkg)
        SettingSwitch(
            stringResource(R.string.settings_commands_only_to, label),
            null,
            to == pkg,
            { want ->
                if (want) Prefs.setCommandsOnlyTo(context, pkg)
                version++
            },
        )
    }
}
