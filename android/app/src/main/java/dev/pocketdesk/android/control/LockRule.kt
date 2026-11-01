package dev.pocketdesk.android.control

/**
 * Hands-free: what kind of lock a phone is under, and so whether an open
 * asked by the computer may carry on past it. Pure, for JVM tests;
 * [KeyguardDismiss] does the dismissing.
 *
 * Only a lock that guards nothing is swiped away: one with no secret at
 * all, or one Android itself has already unlocked (a trusted place or
 * device, a face or fingerprint) and only shows still. A PIN, pattern,
 * password or biometric lock that is on is never bypassed, whatever asks.
 */
object LockRule {
    enum class Kind {
        /** No lock screen is up. */
        UNLOCKED,

        /** A lock screen with no secret behind it (swipe only, or none set). */
        SWIPE,

        /**
         * A lock screen over a secure lock that Android has already
         * unlocked (Extend Unlock, Smart Lock, a biometric): it needs only a
         * swipe, and Android's own state says so.
         */
        TRUSTED,

        /** A secure lock (PIN, pattern, password, biometric) that is on now. */
        SECURE,
    }

    /**
     * The kind of lock from Android's three answers: [keyguardLocked] (a lock
     * screen is showing), [keyguardSecure] (a secret is set) and
     * [deviceLocked] (Android needs the secret to get in). An answer that
     * could not be read is passed as the one that refuses ([keyguardSecure]
     * and [deviceLocked] true).
     */
    fun kind(keyguardLocked: Boolean, keyguardSecure: Boolean, deviceLocked: Boolean): Kind = when {
        !keyguardLocked -> Kind.UNLOCKED
        !keyguardSecure -> Kind.SWIPE
        !deviceLocked -> Kind.TRUSTED
        else -> Kind.SECURE
    }

    /** Whether the phone may be woken and its lock screen swiped away for an open. */
    fun mayDismiss(kind: Kind): Boolean = kind != Kind.SECURE

    /**
     * Whether the dismissing is needed at all: a lock screen is up, or the
     * screen is off ([awake] false) and has to be turned on.
     */
    fun needsDismiss(kind: Kind, awake: Boolean): Boolean = kind != Kind.UNLOCKED || !awake

    /** How long a dismissal waits for Android's answer. */
    const val DISMISS_WAIT_MS = 8_000L

    /** How long a dismissal ask stays valid for the transparent window to act on. */
    const val ASK_VALID_MS = 5_000L

    /** Whether the window started at [startedAt] may still act on an ask made at [askedAt] (both one monotonic clock). */
    fun askValid(askedAt: Long?, startedAt: Long): Boolean =
        askedAt != null && startedAt - askedAt in 0..ASK_VALID_MS
}
