package dev.pocketdesk.android.control

import android.app.Activity
import android.app.KeyguardManager
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.os.SystemClock
import android.util.Log
import android.view.WindowManager
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Hands-free: wake the phone and swipe a lock screen that guards nothing
 * away, so an app the computer asked for can open ([LockRule]). The lock
 * screen is dismissed by Android's own `requestDismissKeyguard`, from a
 * window that shows over it. A secure lock is never touched: the window
 * looks again, at the moment it acts, and does nothing under one.
 */
object KeyguardDismiss {
    private const val TAG = "KeyguardDismiss"

    /** A dismissal waited for, and when it was asked (a monotonic clock). */
    @Volatile
    private var pending: Pending? = null

    private class Pending(val askedAt: Long, val done: CountDownLatch, val ok: AtomicBoolean)

    /** What kind of lock the phone is under now. */
    fun kind(context: Context): LockRule.Kind {
        val keyguard = context.getSystemService(KeyguardManager::class.java)
            ?: return LockRule.Kind.SECURE
        return LockRule.kind(
            keyguardLocked = keyguard.isKeyguardLocked,
            keyguardSecure = keyguard.isKeyguardSecure,
            deviceLocked = keyguard.isDeviceLocked,
        )
    }

    /**
     * Wake the phone and swipe its lock screen away, from the control service's
     * own context (Android lets a bound accessibility service start a window
     * from the background), and wait for Android's answer. Never called on the
     * main thread. True when the phone is awake with no lock screen over it.
     */
    fun dismiss(context: Context): Boolean {
        val service = ControlService.instance ?: return false
        if (!LockRule.mayDismiss(kind(context))) return false
        val ask = Pending(SystemClock.elapsedRealtime(), CountDownLatch(1), AtomicBoolean(false))
        pending = ask
        try {
            val started = runCatching {
                service.startActivity(
                    Intent(context, DismissKeyguardActivity::class.java)
                        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_NO_ANIMATION),
                )
            }
            if (started.isFailure) {
                Log.w(TAG, "the dismissal window did not start", started.exceptionOrNull())
                return false
            }
            val answered = ask.done.await(LockRule.DISMISS_WAIT_MS, TimeUnit.MILLISECONDS)
            return answered && ask.ok.get()
        } finally {
            if (pending === ask) pending = null
        }
    }

    /** The window's side: whether an ask is waiting and still fresh, taking it. */
    internal fun take(): Triple<CountDownLatch, AtomicBoolean, Boolean>? {
        val ask = pending ?: return null
        pending = null
        val valid = LockRule.askValid(ask.askedAt, SystemClock.elapsedRealtime())
        return Triple(ask.done, ask.ok, valid)
    }
}

/**
 * The transparent window [KeyguardDismiss] starts: shows over the lock
 * screen, turns the screen on, and asks Android to dismiss the lock screen
 * when nothing secret guards it. Without an ask waiting (it was started by
 * nothing of ours), or when the lock is secure, it closes at once.
 */
class DismissKeyguardActivity : Activity() {
    private val main = Handler(Looper.getMainLooper())
    private var latch: CountDownLatch? = null
    private var ok: AtomicBoolean? = null
    private var done = false

    @Suppress("DEPRECATION")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val ask = KeyguardDismiss.take()
        if (ask == null || !ask.third) {
            Log.i(TAG, "no dismissal was asked for: nothing is done")
            ask?.first?.countDown()
            finish()
            return
        }
        latch = ask.first
        ok = ask.second
        // Only a lock that guards nothing: looked at again, now, as the window acts.
        val kind = KeyguardDismiss.kind(this)
        if (!LockRule.mayDismiss(kind)) {
            Log.i(TAG, "a secure lock is on: it is not touched")
            end(false)
            return
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
            setShowWhenLocked(true)
            setTurnScreenOn(true)
        } else {
            window.addFlags(
                WindowManager.LayoutParams.FLAG_SHOW_WHEN_LOCKED or WindowManager.LayoutParams.FLAG_TURN_SCREEN_ON,
            )
        }
        window.addFlags(WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE)
        val keyguard = getSystemService(KeyguardManager::class.java)
        if (kind == LockRule.Kind.UNLOCKED || keyguard == null) {
            // Only asleep: the screen is turned on by this window; give it a moment.
            // Read again after the wake: the lock must still be none, never assumed.
            main.postDelayed({ end(settled()) }, WAKE_MS)
            return
        }
        keyguard.requestDismissKeyguard(
            this,
            object : KeyguardManager.KeyguardDismissCallback() {
                // Android's word is read again, not taken: the phone must now be
                // awake with no lock screen over it (a bouncer asking for a secret is not).
                override fun onDismissSucceeded() {
                    main.postDelayed({ end(settled()) }, SETTLE_MS)
                }

                override fun onDismissCancelled() = end(false)

                override fun onDismissError() = end(false)
            },
        )
        // A swipe is answered at once; waiting longer means Android wants a secret: not given here.
        main.postDelayed({ end(false) }, LockRule.BOUNCER_WAIT_MS)
    }

    /** Whether the phone is open now, from Android's own answers read just now. */
    private fun settled(): Boolean =
        LockRule.openNow(
            KeyguardDismiss.kind(this),
            getSystemService(PowerManager::class.java)?.isInteractive == true,
        )

    private fun end(success: Boolean) {
        if (done) return
        done = true
        ok?.set(success)
        latch?.countDown()
        Log.i(TAG, if (success) "the lock screen was dismissed" else "the lock screen was not dismissed")
        finish()
    }

    override fun onDestroy() {
        main.removeCallbacksAndMessages(null)
        if (!done) {
            done = true
            latch?.countDown()
        }
        super.onDestroy()
    }

    private companion object {
        const val TAG = "KeyguardDismiss"
        const val WAKE_MS = 700L
        const val SETTLE_MS = 400L
    }
}
