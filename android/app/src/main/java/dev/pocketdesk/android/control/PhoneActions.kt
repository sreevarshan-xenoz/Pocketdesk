package dev.pocketdesk.android.control

import android.accessibilityservice.AccessibilityService
import android.app.KeyguardManager
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.PowerManager
import android.os.SystemClock
import android.util.Log
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.Prefs
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.files.ScreenshotPrefs
import dev.pocketdesk.android.notify.NotificationActionRule
import org.json.JSONArray
import org.json.JSONObject

/**
 * O2: the screenshots and locks this phone's computer asks for, done
 * through C8's control service when [PhoneActionRule] says so. A
 * screenshot reaches the computer as T10h's new screenshots do.
 */
object PhoneActions {
    private const val TAG = "PhoneActions"

    /** Phone → computer: the apps that open (P2), `core::protocol::phone_action::PHONE_APPS`. */
    private const val PHONE_APPS = "kdeconnect.pocketdesk.phone_apps"

    /** Phone → computer (Apps part 1): the icons it did not have, `core::protocol::phone_action::PHONE_APP_ICONS`. */
    private const val PHONE_APP_ICONS = "kdeconnect.pocketdesk.phone_app_icons"

    /** Phone → computer (Apps part 1): how an ask for the apps or an open went, `PHONE_APP_OPENED`. */
    private const val PHONE_APP_OPENED = "kdeconnect.pocketdesk.phone_app_opened"

    private val budget = NotificationActionRule.Budget(PhoneActionRule.PER_MINUTE)

    /** The apps last listed for the computer: an open takes one of them only (P2 review). */
    @Volatile
    private var listed: Set<String> = emptySet()

    /** A list being made, off the answers tick (P2 review: hundreds of labels read). */
    private val listing = java.util.concurrent.atomic.AtomicBoolean(false)

    /** A list of contacts being made, off the answers tick (P10). */
    private val listingContacts = java.util.concurrent.atomic.AtomicBoolean(false)

    /** The packet the contacts go in (P10). */
    private const val PHONE_CONTACTS = "kdeconnect.pocketdesk.phone_contacts"

    /** The most contacts sent, as the computer keeps at most. */
    private const val MAX_CONTACTS = 5_000

    /**
     * This phone's contacts with a number (P10): the name each shows by,
     * and its numbers, each once, five at most; nothing else of them.
     */
    private fun contacts(context: Context): JSONObject? {
        val byName = LinkedHashMap<String, MutableList<String>>()
        val phone = android.provider.ContactsContract.CommonDataKinds.Phone.CONTENT_URI
        val projection = arrayOf(
            android.provider.ContactsContract.CommonDataKinds.Phone.DISPLAY_NAME,
            android.provider.ContactsContract.CommonDataKinds.Phone.NUMBER,
        )
        val read = runCatching {
            context.contentResolver.query(
                phone,
                projection,
                null,
                null,
                android.provider.ContactsContract.CommonDataKinds.Phone.DISPLAY_NAME + " COLLATE NOCASE",
            )?.use { cursor ->
                while (cursor.moveToNext()) {
                    val name = cursor.getString(0)?.trim().orEmpty()
                    val number = cursor.getString(1)?.trim().orEmpty()
                    if (name.isEmpty() || number.isEmpty()) continue
                    if (name !in byName && byName.size >= MAX_CONTACTS) continue
                    val numbers = byName.getOrPut(name) { mutableListOf() }
                    if (numbers.size < 5 && numbers.none { it.filter(Char::isDigit) == number.filter(Char::isDigit) }) {
                        numbers.add(number)
                    }
                }
            }
        }.onFailure { Log.w(TAG, "could not read the contacts", it) }
        // None sent when they could not be read (P10 review: an empty list
        // would replace the computer's good one).
        if (read.isFailure) return null
        val list = JSONArray()
        for ((name, numbers) in byName) {
            list.put(JSONObject().put("name", name).put("numbers", JSONArray(numbers)))
        }
        return JSONObject().put("contacts", list)
    }

    /** The channel of "Tap to open", as N3's. */
    private const val OPEN_CHANNEL = "opens"

    /** How long "Tap to open" waits, as N3's. */
    private const val OPEN_OFFER_MS = 10 * 60_000L

    /** Android's opt-in to start an app from the background, as N3 sends its opens. */
    @Suppress("DEPRECATION")
    private fun openOptions(): android.os.Bundle? = when {
        Build.VERSION.SDK_INT >= 36 -> android.app.ActivityOptions.makeBasic()
            .setPendingIntentBackgroundActivityStartMode(android.app.ActivityOptions.MODE_BACKGROUND_ACTIVITY_START_ALLOW_ALWAYS)
            .toBundle()
        Build.VERSION.SDK_INT >= 34 -> android.app.ActivityOptions.makeBasic()
            .setPendingIntentBackgroundActivityStartMode(android.app.ActivityOptions.MODE_BACKGROUND_ACTIVITY_START_ALLOWED)
            .toBundle()
        else -> null
    }

    /**
     * Start [pkg]'s launcher activity from the bound control service (Apps
     * part 1). Android lets an accessibility service the system has bound
     * start an activity from the background (a documented exemption of its
     * background-start limits), so the service's own context starts it; if
     * the service refuses, the same intent goes by N3's opt-in. The words of
     * [AppOpenRule]: opened, not found, or failed.
     */
    internal fun launchFromService(context: Context, pkg: String): String {
        val intent = context.packageManager.getLaunchIntentForPackage(pkg)?.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            ?: return AppOpenRule.NOT_FOUND
        val service = ControlService.instance
        if (service != null && runCatching { service.startActivity(intent) }.isSuccess) return AppOpenRule.OPENED
        val tap = android.app.PendingIntent.getActivity(
            context,
            pkg.hashCode(),
            intent,
            android.app.PendingIntent.FLAG_IMMUTABLE or android.app.PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return if (runCatching { tap.send(context, 0, null, null, null, null, openOptions()) }.isSuccess) {
            AppOpenRule.OPENED
        } else {
            AppOpenRule.FAILED
        }
    }

    /** Tell the computer at [address]:[controlPort] how an ask went, over the link already up to it. */
    private fun report(address: String, controlPort: Int, about: String, app: String?, result: String) {
        val sent = RustCore.sendPhonePacketOnLink(
            address,
            controlPort,
            PHONE_APP_OPENED,
            AppOpenRule.report(about, app, result).toString(),
        ).isSuccess
        Log.i(TAG, if (sent) "told the computer: $about $result" else "could not tell the computer: $about $result")
    }

    /**
     * The app the computer named opens once the phone's screen is shown to
     * it (Apps part 1): after the owner's one consent, [CastService] is up,
     * and with the control service bound the app starts and the computer is
     * told. Waits at most 30 s, off the main thread.
     */
    fun openWhenShown(context: Context, pkg: String, address: String, controlPort: Int) {
        val app = context.applicationContext
        Thread {
            val until = SystemClock.elapsedRealtime() + SHOW_WAIT_MS
            while (SystemClock.elapsedRealtime() < until) {
                if (dev.pocketdesk.android.cast.CastService.screenTo == address && ControlService.instance != null) {
                    // Asked again now, not only before the consent and the
                    // wait: the switch, the lock and the computer (review).
                    val result = stillRefused(app, address) ?: launchFromService(app, pkg)
                    report(address, controlPort, "open", pkg, result)
                    return@Thread
                }
                Thread.sleep(250)
            }
            report(address, controlPort, "open", pkg, if (ControlService.instance == null) AppOpenRule.CONTROL_OFF else AppOpenRule.FAILED)
        }.start()
    }

    /**
     * Why an open asked earlier may not go ahead now, or null when it may: the
     * switch turned off, the phone locked, or [address] no longer the computer
     * the phone works with.
     */
    internal fun stillRefused(context: Context, address: String): String? = when {
        !Prefs.appsFromComputer(context) -> AppOpenRule.OFF
        KeyguardDismiss.kind(context) == LockRule.Kind.SECURE -> AppOpenRule.LOCKED_SECURE
        context.getSystemService(KeyguardManager::class.java)?.isKeyguardLocked ?: true -> AppOpenRule.LOCKED
        dev.pocketdesk.android.notify.PhoneLink.target(context)?.address != address -> AppOpenRule.OFF
        else -> null
    }

    /** How long an open waits for the screen to be shown first. */
    private const val SHOW_WAIT_MS = 30_000L

    /**
     * Open [pkg]: through N3's road while the control service is bound
     * (Android lets the app start one from the background then, with its
     * opt-in), else "Tap to open" for ten minutes. What it did.
     */
    private fun open(context: Context, pkg: String, host: String, service: Boolean): String {
        val intent = context.packageManager.getLaunchIntentForPackage(pkg)?.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            ?: return "not found"
        val tap = android.app.PendingIntent.getActivity(
            context,
            pkg.hashCode(),
            intent,
            android.app.PendingIntent.FLAG_IMMUTABLE or android.app.PendingIntent.FLAG_UPDATE_CURRENT,
        )
        if (service && launchFromService(context, pkg) == AppOpenRule.OPENED) return "opened"
        val manager = context.getSystemService(android.app.NotificationManager::class.java) ?: return "not offered"
        if (!manager.areNotificationsEnabled()) return "not offered: notifications are off"
        if (manager.getNotificationChannel(OPEN_CHANNEL) == null) {
            manager.createNotificationChannel(
                android.app.NotificationChannel(
                    OPEN_CHANNEL,
                    context.getString(dev.pocketdesk.android.R.string.channel_opens),
                    android.app.NotificationManager.IMPORTANCE_HIGH,
                ),
            )
        }
        val label = runCatching {
            context.packageManager.getApplicationLabel(context.packageManager.getApplicationInfo(pkg, 0)).toString()
        }.getOrDefault(pkg)
        val notification = android.app.Notification.Builder(context, OPEN_CHANNEL)
            .setSmallIcon(dev.pocketdesk.android.R.drawable.ic_stat_pocketdesk)
            .setContentTitle(context.getString(dev.pocketdesk.android.R.string.open_tap_title, label))
            .setContentText(context.getString(dev.pocketdesk.android.R.string.open_tap_text, HostStore.shownName(context, host)))
            .setContentIntent(tap)
            .setAutoCancel(true)
            .setTimeoutAfter(OPEN_OFFER_MS)
            .build()
        return if (runCatching { manager.notify("open-app", pkg.hashCode(), notification) }.isSuccess) "offered" else "not offered"
    }

    /** Send the icons of [apps] to [own], [AppsList.MAX_ICONS_PER_PACKET] or fewer a packet. */
    private fun sendIcons(context: Context, own: HostStore.Host, apps: List<AppsList.App>) {
        val icons = apps.mapNotNull { AppsList.icon(context, it) }
        var sent = 0
        for (batch in AppsList.batches(icons, { it.png.length }, AppsList.MAX_ICONS_PER_PACKET, AppsList.MAX_PACKET_CHARS)) {
            val ok = RustCore.sendPhonePacketOnLink(
                own.address,
                own.controlPort,
                PHONE_APP_ICONS,
                AppsList.iconsBody(batch).toString(),
            ).isSuccess
            if (!ok) break
            sent += batch.size
        }
        Log.i(TAG, "$sent of ${icons.size} app icons were sent to the computer")
    }

    /**
     * An open [PhoneActionRule] allowed: what the computer asked for decides
     * how it goes ([AppOpenRule.plan]), and the computer is told the result.
     */
    private fun openAsked(context: Context, ask: RustCore.PhoneActionAsk, own: HostStore.Host?, service: Boolean) {
        if (own == null) return
        // Hands-free: a lock that guards nothing is swiped away, and a screen
        // that is off woken, first (off this tick: it waits for Android). A
        // secure lock never got here; the window looks again before it acts.
        val awake = context.getSystemService(PowerManager::class.java)?.isInteractive ?: false
        if (service && LockRule.needsDismiss(KeyguardDismiss.kind(context), awake)) {
            val app = context.applicationContext
            Thread {
                if (KeyguardDismiss.dismiss(app)) {
                    openUnlocked(app, ask, own, true)
                } else {
                    val why = if (KeyguardDismiss.kind(app) == LockRule.Kind.SECURE) AppOpenRule.LOCKED_SECURE else AppOpenRule.LOCKED
                    Log.i(TAG, "the lock screen was not dismissed for an open: $why")
                    report(own.address, own.controlPort, "open", ask.app, why)
                }
            }.start()
            return
        }
        openUnlocked(context, ask, own, service)
    }

    /** The open itself, once the phone is awake and unlocked. */
    private fun openUnlocked(context: Context, ask: RustCore.PhoneActionAsk, own: HostStore.Host, service: Boolean) {
        val casting = dev.pocketdesk.android.cast.CastService.screenTo == own.address
        val paused = dev.pocketdesk.android.cast.CastService.pausedFor == own.address
        val result = when (AppOpenRule.plan(ask.screen, service, casting, paused)) {
            AppOpenRule.Plan.TAP -> {
                // As P2: at once with the service, else "Tap to open".
                val done = open(context, ask.app, ask.host, service)
                Log.i(TAG, "an app the computer named: $done")
                when (done) {
                    "opened" -> AppOpenRule.OPENED
                    "not found" -> AppOpenRule.NOT_FOUND
                    "offered" -> AppOpenRule.CONTROL_OFF
                    else -> AppOpenRule.FAILED
                }
            }
            AppOpenRule.Plan.CONTROL_OFF -> AppOpenRule.CONTROL_OFF
            // One consent for the connection: the owner is asked once, by a tap, and the app opens after.
            AppOpenRule.Plan.SHOW_SCREEN_FIRST -> showScreenFirst(context, ask, own)
            // The cast already shown is reused: no new consent.
            AppOpenRule.Plan.START -> launchFromService(context, ask.app)
            // Hands-free: the share paused when this computer left resumes with the consent already
            // given; the app opens when it is up (and the computer is told then). Not resumable now
            // (it ran out, or the switch went off): asked for as if none was kept.
            AppOpenRule.Plan.RESUME ->
                if (dev.pocketdesk.android.cast.CastService.resume(context, own.address, own.port)) {
                    openWhenShown(context.applicationContext, ask.app, own.address, own.controlPort)
                    null
                } else {
                    showScreenFirst(context, ask, own)
                }
        }
        if (result != null) Thread { report(own.address, own.controlPort, "open", ask.app, result) }.start()
    }

    /** The owner is asked once, at the phone, for the screen; the computer is told so. */
    private fun showScreenFirst(context: Context, ask: RustCore.PhoneActionAsk, own: HostStore.Host): String {
        val pm = context.packageManager
        val label = runCatching { pm.getApplicationLabel(pm.getApplicationInfo(ask.app, 0)).toString() }
            .getOrDefault(ask.app)
        dev.pocketdesk.android.cast.ScreenForApp.offer(context, own, ask.app, label)
        return AppOpenRule.NEEDS_SCREEN
    }

    /** Take the asks waiting; [own] is the computer the phone works with. */
    fun take(context: Context, own: HostStore.Host?) {
        val asks = RustCore.takePhoneActions()
        if (asks.isEmpty()) return
        val service = ControlService.instance
        for (ask in asks) {
            val action = PhoneActionRule.Action.parse(ask.action) ?: continue
            val ownComputer = own != null && own.address == ask.host && own.controlPort == ask.port
            var decision = PhoneActionRule.decide(
                action = action,
                on = Prefs.phoneActionsFromComputer(context),
                own = ownComputer,
                service = service != null,
                sdk = Build.VERSION.SDK_INT,
                locked = context.getSystemService(KeyguardManager::class.java)?.isKeyguardLocked ?: true,
                secureLock = KeyguardDismiss.kind(context) == LockRule.Kind.SECURE,
                awake = context.getSystemService(PowerManager::class.java)?.isInteractive ?: false,
                // T10h carries it, to the clipboard or Downloads (O2 review: with
                // both off, it would be taken and go nowhere).
                screenshots = ScreenshotPrefs.on(context) && ScreenshotPrefs.granted(context) &&
                    (ScreenshotPrefs.clipboard(context) || ScreenshotPrefs.save(context)),
                withinBudget = true,
                appsOn = Prefs.appsFromComputer(context),
                contactsOn = dev.pocketdesk.android.notify.TextPrefs.contactsToComputer(context),
                contactsPermitted = context.checkSelfPermission(android.Manifest.permission.READ_CONTACTS) ==
                    android.content.pm.PackageManager.PERMISSION_GRANTED,
                listed = ask.app in listed,
            )
            if (decision == PhoneActionRule.Decision.DO && !budget.take(SystemClock.elapsedRealtime())) {
                decision = PhoneActionRule.Decision.TOO_MANY
            }
            if (decision != PhoneActionRule.Decision.DO) {
                Log.i(TAG, "an ask from the computer (${action.word}) was not done: $decision")
                // Apps part 1: the phone's own computer is told why a list or an
                // open was refused, so it can say so; any other is told nothing.
                val word = AppOpenRule.tell(decision, ownComputer)
                if (own != null && word != null &&
                    (action == PhoneActionRule.Action.APPS || action == PhoneActionRule.Action.OPEN)
                ) {
                    val about = if (action == PhoneActionRule.Action.OPEN) "open" else "apps"
                    val named = if (action == PhoneActionRule.Action.OPEN) ask.app else null
                    Thread { report(own.address, own.controlPort, about, named, word) }.start()
                }
                continue
            }
            // P2: the apps listed for the computer, off this tick, or one opened.
            // P10: the contacts, off this tick, names and numbers only.
            if (action == PhoneActionRule.Action.CONTACTS) {
                if (own == null || !listingContacts.compareAndSet(false, true)) continue
                val app = context.applicationContext
                Thread {
                    try {
                        val body = contacts(app) ?: return@Thread
                        val sent = RustCore.sendPhonePacketOnLink(
                            own.address,
                            own.controlPort,
                            PHONE_CONTACTS,
                            body.toString(),
                        ).isSuccess
                        Log.i(TAG, if (sent) "the contacts were listed for the computer" else "the contacts could not be sent")
                    } finally {
                        listingContacts.set(false)
                    }
                }.start()
                continue
            }
            if (action == PhoneActionRule.Action.APPS) {
                if (own == null || !listing.compareAndSet(false, true)) continue
                val app = context.applicationContext
                val have = ask.have
                Thread {
                    try {
                        val apps = AppsList.collect(app, PhoneActionRule.MAX_APPS)
                        listed = apps.map { it.pkg }.toSet()
                        val sent = RustCore.sendPhonePacketOnLink(
                            own.address,
                            own.controlPort,
                            PHONE_APPS,
                            AppsList.listBody(apps).toString(),
                        ).isSuccess
                        Log.i(TAG, if (sent) "the apps were listed for the computer" else "the apps could not be sent")
                        // The icons the computer does not keep at these versions, once each, in batches.
                        if (sent) sendIcons(app, own, AppsList.missing(apps, have))
                    } finally {
                        listing.set(false)
                    }
                }.start()
                continue
            }
            if (action == PhoneActionRule.Action.OPEN) {
                openAsked(context, ask, own, service != null)
                continue
            }
            if (service == null) continue
            val done = Build.VERSION.SDK_INT >= PhoneActionRule.MIN_SDK &&
                service.performGlobalAction(
                    if (action == PhoneActionRule.Action.SCREENSHOT) {
                        AccessibilityService.GLOBAL_ACTION_TAKE_SCREENSHOT
                    } else {
                        AccessibilityService.GLOBAL_ACTION_LOCK_SCREEN
                    },
                )
            // What became of it, for the device checks.
            Log.i(TAG, if (done) "a ${action.word} from the computer was done" else "a ${action.word} from the computer failed")
        }
    }
}
