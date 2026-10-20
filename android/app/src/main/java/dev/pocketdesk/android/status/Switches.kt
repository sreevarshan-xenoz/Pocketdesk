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
    return mapOf(
        "notifications" to s(PhoneNotificationPrefs.enabled(ctx), access),
        "answers" to s(Prefs.answerNotifications(ctx), access),
        "texts" to s(TextPrefs.show(ctx), granted(Manifest.permission.RECEIVE_SMS)),
        "clipboard" to s(Prefs.clipboardInBackground(ctx)),
        "files" to s(Prefs.filesInBackground(ctx), mayPost),
        "folders" to s(runCatching { PhoneFolders.all(ctx).isNotEmpty() }.getOrDefault(false)),
        "photos" to s(RecentPhotoPrefs.on(ctx), photos),
        "screenshots" to s(ScreenshotPrefs.on(ctx), photos),
        "camera" to s(Prefs.cameraAsks(ctx), granted(Manifest.permission.CAMERA)),
        "links" to s(Prefs.linksFromComputer(ctx), mayPost),
        "ring" to s(Prefs.ringFromComputer(ctx), mayPost),
        "media" to s(Prefs.phoneMediaOnComputer(ctx), access),
        "mute" to s(QuietPrefs.letComputerMute(ctx), access),
        "control" to s(Prefs.controlFromComputer(ctx), ControlRunner.serviceEnabled(ctx)),
        "dial" to s(Prefs.callsFromComputer(ctx)),
        "commands" to s(Prefs.commandsFromComputer(ctx)),
    )
}
