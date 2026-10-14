package dev.pocketdesk.android.doctor

import android.app.ActivityManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.PowerManager
import android.provider.Settings
import androidx.core.net.toUri
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.notify.PhoneNotificationPrefs
import dev.pocketdesk.android.notify.TextPrefs

/** What Android lets Pocketdesk do in the background. */
enum class BackgroundLimit {
    /** Allowed: battery optimization is off for it. */
    NONE,

    /** Battery optimization may stop it after a while. */
    OPTIMIZED,

    /** The person set its battery use to Restricted. */
    RESTRICTED,
}

/**
 * Staying connected in the background (T2). Battery optimization does not
 * stop Pocketdesk's notification listener, but while the phone sleeps it
 * cuts the network: a notification sent then is lost, and a text arrives
 * late. The decisions are kept apart from Android so they are tested.
 */
object BackgroundRunning {
    fun limit(ctx: Context): BackgroundLimit {
        val activity = ctx.getSystemService(ActivityManager::class.java)
        if (Build.VERSION.SDK_INT >= 28 && activity.isBackgroundRestricted) return BackgroundLimit.RESTRICTED
        val power = ctx.getSystemService(PowerManager::class.java)
        return if (power.isIgnoringBatteryOptimizations(ctx.packageName)) {
            BackgroundLimit.NONE
        } else {
            BackgroundLimit.OPTIMIZED
        }
    }

    /**
     * Whether a feature that works in the background is on and able to
     * work: without notification access or the texts permission, the
     * battery is not what stops it.
     */
    fun featuresOn(ctx: Context): Boolean {
        val granted = { permission: String ->
            ctx.checkSelfPermission(permission) == android.content.pm.PackageManager.PERMISSION_GRANTED
        }
        val notifications = PhoneNotificationPrefs.enabled(ctx) && PhoneNotificationPrefs.accessGranted(ctx)
        val texts = (TextPrefs.show(ctx) && granted(android.Manifest.permission.RECEIVE_SMS)) ||
            (TextPrefs.reply(ctx) && granted(android.Manifest.permission.SEND_SMS) &&
                PhoneNotificationPrefs.accessGranted(ctx))
        return notifications || texts
    }

    /** Whether Home shows the card: a background feature on, Android limiting it, and the reminder on. */
    fun shouldRemind(featuresOn: Boolean, limit: BackgroundLimit, remindOn: Boolean): Boolean =
        featuresOn && limit != BackgroundLimit.NONE && remindOn


    /**
     * dontkillmyapp.com's page for this phone's maker, where makers add
     * their own battery managers on top of Android's; null for makers the
     * site does not cover.
     */
    fun makerGuide(manufacturer: String): String? {
        val maker = when (val m = manufacturer.lowercase().trim()) {
            "redmi", "poco" -> "xiaomi"
            "honor" -> "huawei"
            else -> m
        }
        return if (maker in KNOWN_MAKERS) "https://dontkillmyapp.com/$maker" else null
    }

    private val KNOWN_MAKERS = setOf(
        "samsung", "xiaomi", "oppo", "realme", "oneplus", "huawei", "vivo", "meizu",
        "asus", "nokia", "sony", "lenovo", "motorola", "google", "tecno", "infinix", "wiko",
    )

    /**
     * The page that lifts [limit]. Since Android 12 the app's own page holds
     * Unrestricted / Optimized / Restricted one tap away; before it, the
     * battery optimization list does (where the person picks the app).
     */
    fun settingsIntent(ctx: Context, limit: BackgroundLimit): Intent =
        if (limit == BackgroundLimit.RESTRICTED || Build.VERSION.SDK_INT >= 31) {
            appPage(ctx)
        } else {
            Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)
        }

    fun appPage(ctx: Context): Intent =
        Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", ctx.packageName, null))

    /** Open the page that lifts [limit], or the app's page where that one is missing. */
    fun openSettings(ctx: Context, limit: BackgroundLimit) {
        val opened = runCatching { ctx.startActivity(settingsIntent(ctx, limit)) }.isSuccess
        if (!opened) runCatching { ctx.startActivity(appPage(ctx)) }
    }

    /** Open the maker's guide; false when there is no browser to open it. */
    fun openGuide(ctx: Context, url: String): Boolean =
        runCatching { ctx.startActivity(Intent(Intent.ACTION_VIEW, url.toUri())) }.isSuccess
}
