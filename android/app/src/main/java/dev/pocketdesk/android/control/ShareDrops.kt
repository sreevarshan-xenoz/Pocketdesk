package dev.pocketdesk.android.control

import android.content.ClipData
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.SystemClock
import android.util.Log
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.notify.PhoneLink

/**
 * A file dropped on the phone's screen window shared into the app in front
 * (Apps part 2). The computer sends the file by the ordinary transfer and
 * asks for the share with a `share` action; when both are here and the
 * phone still allows it, Android's own share starts for the app that was in
 * front when the ask came, if that app takes the file's type, and the
 * computer is told how it went. No new permission: the service reads the
 * package of the window in front once, at the ask (never its contents), and
 * starts the share from its own context, which Android lets a bound
 * accessibility service do from the background. If the app takes no such
 * file, nothing is started and the file stays in Downloads, as any file
 * from the computer does.
 */
object ShareDrops {
    private const val TAG = "ShareDrops"

    private val queue = ShareRule.Queue { SystemClock.elapsedRealtime() }

    /** The computer asked (and [PhoneActionRule] allowed) to share [name] ([size] bytes), the file it is sending. */
    fun ask(context: Context, own: HostStore.Host, name: String, size: Long) {
        if (!ShareRule.validName(name) || size < 0) return
        val app = context.applicationContext
        // Off this thread: reading the window in front asks Android.
        Thread {
            val front = ControlService.instance?.foregroundPackage()
            val pending = ShareRule.Pending(own.address, name, size, front, SystemClock.elapsedRealtime())
            val file = queue.offer(pending)
            Log.i(TAG, "a share was asked for; the file ${if (file != null) "is here" else "is awaited"}")
            if (file != null) run(app, own, pending, file)
        }.start()
    }

    /** A file from [address] came whole and was kept at [uri]; shared if a share was asked for it. */
    fun arrived(context: Context, address: String, name: String, size: Long, uri: Uri?, mime: String?) {
        // Nothing is remembered of any file unless the switch is on.
        if (!dev.pocketdesk.android.Prefs.appsFromComputer(context)) return
        val app = context.applicationContext
        val file = ShareRule.Arrived(address, name, size, uri?.toString(), mime, SystemClock.elapsedRealtime())
        val ask = queue.arrive(file) ?: return
        val own = PhoneLink.target(app)?.takeIf { it.address == address } ?: return
        Thread { run(app, own, ask, file) }.start()
    }

    private fun sendIntent(mime: String, uri: Uri?): Intent =
        Intent(Intent.ACTION_SEND).setType(mime).apply {
            if (uri != null) {
                putExtra(Intent.EXTRA_STREAM, uri)
                clipData = ClipData.newRawUri(null, uri)
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }
        }

    /** The share itself, once the ask and the file are both here. */
    private fun run(app: Context, own: HostStore.Host, ask: ShareRule.Pending, file: ShareRule.Arrived) {
        fun tell(app_: String?, result: String) {
            PhoneActions.report(own.address, own.controlPort, "share", app_, result, ask.name)
        }
        // Asked again now: the switch, the lock, the computer, the screen and the service.
        val service = ControlService.instance
        val refused = PhoneActions.stillRefused(app, own.address)
            ?: when {
                service == null -> AppOpenRule.CONTROL_OFF
                dev.pocketdesk.android.cast.CastService.screenTo != own.address -> AppOpenRule.NEEDS_SCREEN
                else -> null
            }
        if (refused != null || service == null) {
            Log.i(TAG, "the share was not done: $refused")
            tell(null, refused ?: AppOpenRule.CONTROL_OFF)
            return
        }
        val pm = app.packageManager
        val front = ask.pkg
        val uri = file.uri?.let { runCatching { Uri.parse(it) }.getOrNull() }
        val mime = ShareRule.mimeOf(file.mime)
        val launchable = front != null && runCatching { pm.getLaunchIntentForPackage(front) != null }.getOrDefault(false)
        @Suppress("DEPRECATION")
        val takes = front != null && uri != null && runCatching {
            pm.queryIntentActivities(sendIntent(mime, null).setPackage(front), PackageManager.MATCH_DEFAULT_ONLY).isNotEmpty()
        }.getOrDefault(false)
        when (ShareRule.verdict(front, app.packageName, launchable, uri != null, takes)) {
            ShareRule.Verdict.NO_APP -> tell(null, AppOpenRule.NO_APP)
            ShareRule.Verdict.CANNOT_TAKE -> tell(front, AppOpenRule.CANNOT_TAKE)
            ShareRule.Verdict.SHARE -> {
                val direct = sendIntent(mime, uri).setPackage(front).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                val started = runCatching { service.startActivity(direct) }.isSuccess ||
                    // The app turned it down: Android's own chooser is the way left.
                    runCatching {
                        service.startActivity(
                            Intent.createChooser(sendIntent(mime, uri), null).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                        )
                    }.isSuccess
                Log.i(TAG, "the share ${if (started) "started" else "could not start"}")
                tell(front, if (started) AppOpenRule.SHARED else AppOpenRule.FAILED)
            }
        }
    }
}
