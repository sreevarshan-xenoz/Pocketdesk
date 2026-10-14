package dev.pocketdesk.android.doctor

import android.app.ActivityManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.PowerManager
import android.provider.Settings
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
 * Staying connected in the background (T2): phone notifications and texts
 * reach the computer only while Android lets Pocketdesk run. The decisions
 * are kept apart from Android so they are tested.
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

    /** Whether a feature that works in the background is on. */
    fun featuresOn(ctx: Context): Boolean =
        PhoneNotificationPrefs.enabled(ctx) || TextPrefs.show(ctx) || TextPrefs.reply(ctx)

    /** Whether Home shows the card: a background feature on, Android limiting it, and the reminder on. */
    fun shouldRemind(featuresOn: Boolean, limit: BackgroundLimit, remindOn: Boolean): Boolean =
        featuresOn && limit != BackgroundLimit.NONE && remindOn

    fun shouldRemind(ctx: Context): Boolean =
        shouldRemind(featuresOn(ctx), limit(ctx), Prefs.backgroundReminder(ctx))

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

    /** The settings page that lifts [limit]. */
    fun settingsIntent(ctx: Context, limit: BackgroundLimit): Intent = when (limit) {
        BackgroundLimit.RESTRICTED ->
            Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", ctx.packageName, null))
        else -> Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)
    }
}
