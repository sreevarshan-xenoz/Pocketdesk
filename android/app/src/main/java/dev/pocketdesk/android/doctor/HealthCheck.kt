package dev.pocketdesk.android.doctor

import android.Manifest
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.provider.Settings
import android.service.notification.NotificationListenerService
import androidx.core.app.NotificationManagerCompat
import androidx.core.net.toUri
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.control.ControlRunner
import dev.pocketdesk.android.files.PhoneFolders
import dev.pocketdesk.android.files.RecentPhotoPrefs
import dev.pocketdesk.android.files.ScreenshotPrefs
import dev.pocketdesk.android.notify.PhoneNotificationListener
import dev.pocketdesk.android.notify.PhoneNotificationPrefs
import dev.pocketdesk.android.notify.TextPrefs
import dev.pocketdesk.android.service.DownloadTarget

/**
 * Whether the notification listener is connected in this process (D1): set
 * by the listener itself. Pocketdesk's activity and the listener share one
 * process, so this is the truth for it; false in a process that never saw it.
 */
object ListenerState {
    @Volatile
    var connected = false
        private set

    fun connected(on: Boolean) {
        connected = on
    }
}

/** Reads [HealthSnapshot] from Android, and opens each problem's fix (D1). */
object HealthCheck {
    /** How long a rebind is given to connect the listener before it counts as stopped. */
    const val REBIND_WAIT_MS = 5_000L

    /**
     * Whether any feature that runs on the notification listener's ticks is
     * on. Answering notifications needs forwarding on as well, so forwarding
     * covers it; texts shown on the computer wait there to be sent again.
     */
    fun listenerFeatures(ctx: Context): Boolean =
        PhoneNotificationPrefs.enabled(ctx) ||
            TextPrefs.show(ctx) ||
            TextPrefs.reply(ctx) ||
            Prefs.clipboardInBackground(ctx) ||
            Prefs.ringFromComputer(ctx) ||
            dev.pocketdesk.android.notify.QuietPrefs.letComputerMute(ctx) ||
            Prefs.filesInBackground(ctx) ||
            Prefs.typingFromComputer(ctx) ||
            // O2: the computer's screenshots and locks wait on the listener (O2 review).
            Prefs.phoneActionsFromComputer(ctx) ||
            // P2: so do its asks for the apps, and P10's for the contacts.
            Prefs.appsFromComputer(ctx) ||
            dev.pocketdesk.android.notify.TextPrefs.contactsToComputer(ctx) ||
            ScreenshotPrefs.on(ctx) ||
            RecentPhotoPrefs.on(ctx) ||
            PhoneFolders.all(ctx).isNotEmpty() ||
            // N2: the phone's music without a session.
            (Prefs.phoneMediaInBackground(ctx) && Prefs.phoneMediaOnComputer(ctx)) ||
            // N5: the computer's music in the media controls.
            Prefs.computerMediaInControls(ctx)

    /**
     * Whether a feature that needs notification access, but not the listener
     * running, is on: the phone's media reads other apps' sessions by it.
     */
    fun accessFeatures(ctx: Context): Boolean = Prefs.phoneMediaOnComputer(ctx)

    /**
     * Ask Android to bind the listener again when it should run and does
     * not: after an update it can stay unbound until access is toggled.
     * True when a rebind was asked, so the caller waits [REBIND_WAIT_MS].
     */
    fun rebindIfStopped(ctx: Context): Boolean {
        if (lowRamListener(ctx) || ListenerState.connected || !listenerFeatures(ctx) || !PhoneNotificationPrefs.accessGranted(ctx)) return false
        return runCatching {
            NotificationListenerService.requestRebind(ComponentName(ctx, PhoneNotificationListener::class.java))
        }.isSuccess
    }

    /**
     * What the check reads. [listenerKnown] is false while a rebind's answer
     * is awaited, so a listener not connected yet is not called stopped.
     */
    fun snapshot(ctx: Context, listenerKnown: Boolean = true): HealthSnapshot {
        val granted = { permission: String ->
            ctx.checkSelfPermission(permission) == PackageManager.PERMISSION_GRANTED
        }
        // M4: typing needs the same Accessibility service.
        val control = Prefs.controlFromComputer(ctx) || Prefs.typingFromComputer(ctx) ||
            Prefs.phoneActionsFromComputer(ctx) || Prefs.appsFromComputer(ctx) ||
            (Prefs.copiesByThemselves(ctx) && dev.pocketdesk.android.clip.CopyWatchRule.supported(Build.VERSION.SDK_INT))
        return HealthSnapshot(
            listenerFeatures = listenerFeatures(ctx),
            accessFeatures = accessFeatures(ctx),
            listenerAccess = PhoneNotificationPrefs.accessGranted(ctx),
            // Android 10 and older bind no listener on low-RAM phones: nothing to judge.
            listenerConnected = when {
                ListenerState.connected -> true
                lowRamListener(ctx) || !listenerKnown -> null
                else -> false
            },
            textsShow = TextPrefs.show(ctx),
            receiveSms = granted(Manifest.permission.RECEIVE_SMS),
            textsReply = TextPrefs.reply(ctx),
            sendSms = granted(Manifest.permission.SEND_SMS),
            // P10: the contacts to the computer need the permission too.
            contactNames = TextPrefs.names(ctx) || TextPrefs.contactsToComputer(ctx),
            readContacts = granted(Manifest.permission.READ_CONTACTS),
            photoFeatures = ScreenshotPrefs.on(ctx) || RecentPhotoPrefs.on(ctx),
            photoAccess = ScreenshotPrefs.granted(ctx),
            screenshotsStopped = ScreenshotPrefs.stopped(ctx),
            // Turned on by the owner, both off by default (D1 review): links
            // from the computer are on by default and post only in a session,
            // so they would flag every phone that said no to notifications.
            postingFeatures = Prefs.ringFromComputer(ctx) || Prefs.filesInBackground(ctx),
            mayPost = NotificationManagerCompat.from(ctx).areNotificationsEnabled(),
            control = control,
            controlService = !control || ControlRunner.serviceEnabled(ctx),
            advancedProtection = control && advancedProtection(ctx),
            downloadFolderLost = runCatching { DownloadTarget.treeLost(ctx) }.getOrDefault(false),
            frozenRecently = FreezeWatch.recent(Prefs.lastFreezeMs(ctx), System.currentTimeMillis()),
            frozenWhileLinked = Prefs.lastFreezeLinked(ctx),
            stayLinked = Prefs.stayLinked(ctx),
        )
    }

    fun problems(ctx: Context, listenerKnown: Boolean = true): List<HealthProblem> =
        Health.problems(snapshot(ctx, listenerKnown))

    /** Android 16's Advanced Protection, which keeps Accessibility services to accessibility tools on 17. */
    fun advancedProtection(ctx: Context): Boolean {
        if (Build.VERSION.SDK_INT < 36) return false
        return runCatching {
            ctx.getSystemService(android.security.advancedprotection.AdvancedProtectionManager::class.java)
                ?.isAdvancedProtectionEnabled == true
        }.onFailure { android.util.Log.w("HealthCheck", "Advanced Protection unreadable", it) }.getOrDefault(false)
    }

    /** A low-RAM phone on Android 10 or older, where no notification listener is bound. */
    private fun lowRamListener(ctx: Context): Boolean =
        Build.VERSION.SDK_INT <= 29 &&
            ctx.getSystemService(android.app.ActivityManager::class.java)?.isLowRamDevice == true

    /**
     * The page that fixes [problem], or null where there is none: Advanced
     * Protection's block is lifted only by turning the setting off, and the
     * download folder is chosen in Pocketdesk's own Settings.
     */
    fun fixIntent(ctx: Context, problem: HealthProblem): Intent? = when (problem) {
        HealthProblem.NOTIFICATION_ACCESS, HealthProblem.LISTENER_STOPPED -> PhoneNotificationPrefs.accessSettingsIntent()
        HealthProblem.TEXTS_RECEIVE, HealthProblem.TEXTS_SEND, HealthProblem.CONTACTS, HealthProblem.PHOTOS ->
            BackgroundRunning.appPage(ctx)
        HealthProblem.APP_NOTIFICATIONS ->
            Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, ctx.packageName)
        HealthProblem.CONTROL_SERVICE -> Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS)
        HealthProblem.CONTROL_BLOCKED, HealthProblem.DOWNLOAD_FOLDER, HealthProblem.SCREENSHOTS_STOPPED,
        HealthProblem.PAUSED_STAY_LINKED_OFF -> null
        // M1: the maker's own settings hold it; its guide says where, else the app's page.
        HealthProblem.PAUSED_ANYWAY -> BackgroundRunning.makerGuide(Build.MANUFACTURER)
            ?.let { Intent(Intent.ACTION_VIEW, it.toUri()) }
            ?: BackgroundRunning.appPage(ctx)
    }?.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

    /** Open [problem]'s fix; the app's page where that one is missing. */
    fun openFix(ctx: Context, problem: HealthProblem) {
        val intent = fixIntent(ctx, problem) ?: return
        val opened = runCatching { ctx.startActivity(intent) }.isSuccess
        if (!opened) runCatching { ctx.startActivity(BackgroundRunning.appPage(ctx).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
    }
}
