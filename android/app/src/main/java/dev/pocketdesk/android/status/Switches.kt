package dev.pocketdesk.android.status

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import androidx.core.app.NotificationManagerCompat
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.control.ControlRunner
import dev.pocketdesk.android.files.PhoneFolders
import dev.pocketdesk.android.files.RecentPhotoPrefs
import dev.pocketdesk.android.files.ScreenshotPrefs
import dev.pocketdesk.android.notify.PhoneNotificationPrefs
import dev.pocketdesk.android.notify.QuietPrefs
import dev.pocketdesk.android.notify.TextPrefs

/**
 * The phone's features now, as [SwitchReport] says them (L7): each one's
 * own switch, and whether Android lets it work where Android must.
 */
internal fun readSwitches(ctx: Context): Map<String, SwitchReport.Switch> {
    fun granted(permission: String) = ctx.checkSelfPermission(permission) == PackageManager.PERMISSION_GRANTED
    val access = runCatching { PhoneNotificationPrefs.accessGranted(ctx) }.getOrDefault(false)
    val photos = runCatching { ScreenshotPrefs.granted(ctx) }.getOrDefault(false)
    val mayPost = NotificationManagerCompat.from(ctx).areNotificationsEnabled()
    fun s(on: Boolean, android: Boolean? = null) = SwitchReport.Switch(on, android)
    // What runs outside a session runs on the notification listener, which
    // Android binds only with notification access (L7 review, as
    // HealthCheck.listenerFeatures counts them); what arrives as a
    // notification needs the app's own notifications too.
    val notifications = PhoneNotificationPrefs.enabled(ctx)
    return mapOf(
        "notifications" to s(notifications, access),
        // Answers need the notifications they answer (L7 review).
        "answers" to s(Prefs.answerNotifications(ctx) && notifications, access),
        // P5: an alarm goes as a notification does (Q3: told, so the
        // computer can show it).
        "alarms" to s(Prefs.alarmsOnComputer(ctx) && notifications, access),
        "texts" to s(TextPrefs.show(ctx), granted(Manifest.permission.RECEIVE_SMS)),
        // P10: read on the listener's link, with Android's contacts permission.
        "contacts" to s(TextPrefs.contactsToComputer(ctx), access && granted(Manifest.permission.READ_CONTACTS)),
        "clipboard" to s(Prefs.clipboardInBackground(ctx), access),
        "files" to s(Prefs.filesInBackground(ctx), access && mayPost),
        // O7: a received image put on the clipboard as it lands.
        "images" to s(Prefs.receivedImagesToClipboard(ctx)),
        "folders" to s(runCatching { PhoneFolders.all(ctx).isNotEmpty() }.getOrDefault(false), access),
        "photos" to s(RecentPhotoPrefs.on(ctx), access && photos),
        "screenshots" to s(ScreenshotPrefs.on(ctx), access && photos),
        // The camera asks for itself when it opens; the ask comes as a notification.
        "camera" to s(Prefs.cameraAsks(ctx), access && mayPost),
        "links" to s(Prefs.linksFromComputer(ctx), mayPost),
        "ring" to s(Prefs.ringFromComputer(ctx), access && mayPost),
        "media" to s(Prefs.phoneMediaOnComputer(ctx), access),
        "media_background" to s(Prefs.phoneMediaInBackground(ctx) && Prefs.phoneMediaOnComputer(ctx), access),
        // N5: shown through a notification of its own.
        "computer_media" to s(Prefs.computerMediaInControls(ctx), access && mayPost),
        "mute" to s(QuietPrefs.letComputerMute(ctx), access),
        "control" to s(Prefs.controlFromComputer(ctx), ControlRunner.serviceEnabled(ctx)),
        "dial" to s(Prefs.callsFromComputer(ctx), mayPost),
        "commands" to s(Prefs.commandsFromComputer(ctx), access),
        // O1: another app's broadcast, taken whatever runs.
        "tasks" to s(Prefs.othersRunTasks(ctx)),
        // O2: the control service does them; the listener waits for the asks.
        "phone_actions" to s(Prefs.phoneActionsFromComputer(ctx), ControlRunner.serviceEnabled(ctx) && access),
        // P2: listed on the listener's link; an app opens at once with the
        // control service, else as "Tap to open".
        "apps" to s(Prefs.appsFromComputer(ctx), access),
    )
}
