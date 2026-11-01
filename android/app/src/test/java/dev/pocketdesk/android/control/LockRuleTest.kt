package dev.pocketdesk.android.control

import dev.pocketdesk.android.control.LockRule.Kind
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class LockRuleTest {
    @Test
    fun `no lock screen is unlocked`() {
        assertEquals(Kind.UNLOCKED, LockRule.kind(keyguardLocked = false, keyguardSecure = false, deviceLocked = false))
        // Whatever else is said, no lock screen up is no lock.
        assertEquals(Kind.UNLOCKED, LockRule.kind(keyguardLocked = false, keyguardSecure = true, deviceLocked = false))
    }

    @Test
    fun `a lock screen with no secret behind it is a swipe`() {
        assertEquals(Kind.SWIPE, LockRule.kind(keyguardLocked = true, keyguardSecure = false, deviceLocked = false))
        assertTrue(LockRule.mayDismiss(Kind.SWIPE))
    }

    @Test
    fun `a secure lock Android has already unlocked is trusted`() {
        assertEquals(Kind.TRUSTED, LockRule.kind(keyguardLocked = true, keyguardSecure = true, deviceLocked = false))
        assertTrue(LockRule.mayDismiss(Kind.TRUSTED))
    }

    @Test
    fun `a secure lock that is on is never bypassed`() {
        assertEquals(Kind.SECURE, LockRule.kind(keyguardLocked = true, keyguardSecure = true, deviceLocked = true))
        assertFalse(LockRule.mayDismiss(Kind.SECURE))
        // The answers that could not be read are passed as the refusing ones.
        assertEquals(Kind.SECURE, LockRule.kind(keyguardLocked = true, keyguardSecure = true, deviceLocked = true))
    }

    @Test
    fun `an unlocked phone is dismissed only to be woken`() {
        assertFalse(LockRule.needsDismiss(Kind.UNLOCKED, awake = true))
        assertTrue(LockRule.needsDismiss(Kind.UNLOCKED, awake = false))
        assertTrue(LockRule.needsDismiss(Kind.SWIPE, awake = true))
        assertTrue(LockRule.needsDismiss(Kind.TRUSTED, awake = false))
        assertTrue(LockRule.mayDismiss(Kind.UNLOCKED))
    }

    @Test
    fun `after a wake or a dismissal the phone is open only if it is awake with no lock screen, read again`() {
        assertTrue(LockRule.openNow(Kind.UNLOCKED, awake = true))
        // Woken but a lock screen is still up (swipe, trusted or secure): not open.
        for (kind in listOf(Kind.SWIPE, Kind.TRUSTED, Kind.SECURE)) {
            assertFalse(kind.name, LockRule.openNow(kind, awake = true))
        }
        // No lock but still asleep: not open.
        assertFalse(LockRule.openNow(Kind.UNLOCKED, awake = false))
        // The lock turned secure between the look and the wake: refused, not opened.
        assertFalse(LockRule.mayDismiss(LockRule.kind(keyguardLocked = true, keyguardSecure = true, deviceLocked = true)))
        assertFalse(LockRule.openNow(LockRule.kind(true, true, true), awake = true))
    }

    @Test
    fun `a bouncer asking for a secret is given up on quickly`() {
        assertTrue(LockRule.BOUNCER_WAIT_MS <= 3_000L)
        assertTrue(LockRule.BOUNCER_WAIT_MS < LockRule.DISMISS_WAIT_MS)
    }

    @Test
    fun `an ask is acted on only while it is fresh`() {
        assertTrue(LockRule.askValid(askedAt = 1_000, startedAt = 1_000))
        assertTrue(LockRule.askValid(askedAt = 1_000, startedAt = 1_000 + LockRule.ASK_VALID_MS))
        assertFalse(LockRule.askValid(askedAt = 1_000, startedAt = 1_001 + LockRule.ASK_VALID_MS))
        // Nothing asked: nothing is done. A clock that went backwards is not an ask either.
        assertFalse(LockRule.askValid(askedAt = null, startedAt = 1_000))
        assertFalse(LockRule.askValid(askedAt = 2_000, startedAt = 1_000))
    }
}
