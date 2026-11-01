package dev.pocketdesk.wear

import dev.pocketdesk.watchlink.WatchOp
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class WatchLockRuleTest {
    @Test
    fun aDeckPressAndANoNeedTheWatchsLock() {
        assertTrue(WatchLockRule.needsLock(WatchOp.DeckPress))
        assertTrue(WatchLockRule.needsLock(WatchOp.AskDeny))
        for (op in listOf(WatchOp.Wake, WatchOp.Ring, WatchOp.Music, WatchOp.Status, WatchOp.DeckList, WatchOp.AskList)) {
            assertFalse("$op", WatchLockRule.needsLock(op))
        }
    }

    @Test
    fun aWatchWithNoLockOrLockedHandsOverNothingThatNeedsOne() {
        assertEquals(WatchLockRule.Verdict.NoLockSet, WatchLockRule.check(WatchOp.DeckPress, deviceSecure = false, deviceLocked = false))
        assertEquals(WatchLockRule.Verdict.Locked, WatchLockRule.check(WatchOp.AskDeny, deviceSecure = true, deviceLocked = true))
        assertEquals(WatchLockRule.Verdict.Allowed, WatchLockRule.check(WatchOp.AskDeny, deviceSecure = true, deviceLocked = false))
    }

    @Test
    fun theOthersNeverWaitForTheLock() {
        assertEquals(WatchLockRule.Verdict.Allowed, WatchLockRule.check(WatchOp.Wake, deviceSecure = false, deviceLocked = true))
        assertEquals(WatchLockRule.Verdict.Allowed, WatchLockRule.check(WatchOp.Music, deviceSecure = false, deviceLocked = false))
    }
}
