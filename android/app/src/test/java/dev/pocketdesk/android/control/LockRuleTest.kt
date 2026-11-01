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
    fun `an ask is acted on only while it is fresh`() {
        assertTrue(LockRule.askValid(askedAt = 1_000, startedAt = 1_000))
        assertTrue(LockRule.askValid(askedAt = 1_000, startedAt = 1_000 + LockRule.ASK_VALID_MS))
        assertFalse(LockRule.askValid(askedAt = 1_000, startedAt = 1_001 + LockRule.ASK_VALID_MS))
        // Nothing asked: nothing is done. A clock that went backwards is not an ask either.
        assertFalse(LockRule.askValid(askedAt = null, startedAt = 1_000))
        assertFalse(LockRule.askValid(askedAt = 2_000, startedAt = 1_000))
    }
}
