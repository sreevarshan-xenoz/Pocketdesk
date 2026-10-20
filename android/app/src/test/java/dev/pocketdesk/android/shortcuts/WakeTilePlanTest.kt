package dev.pocketdesk.android.shortcuts

import dev.pocketdesk.android.shortcuts.WakeTilePlan.Send
import dev.pocketdesk.android.shortcuts.WakeTilePlan.Step
import dev.pocketdesk.android.wake.WakePlan
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class WakeTilePlanTest {
    private val plan = WakePlan.Plan.Direct("00:00:5e:00:53:01", listOf("192.0.2.255"))

    @Test
    fun theTileSaysWhetherAWakeCanGoFromHere() {
        val say = { plan: WakePlan.Plan? -> WakeTilePlan.said(plan, sent = null) }
        assertEquals(WakeTilePlan.Said.NoComputer, say(null))
        assertEquals(WakeTilePlan.Said.Ready, say(plan))
        assertEquals(WakeTilePlan.Said.Ready, say(WakePlan.Plan.Relay("192.0.2.2", "00:00:5e:00:53:01", "192.0.2.255")))
        assertEquals(WakeTilePlan.Said.Ready, say(WakePlan.Plan.Away))
        assertEquals(WakeTilePlan.Said.NotYet, say(WakePlan.Plan.NoMac))
        assertEquals(WakeTilePlan.Said.Off, say(WakePlan.Plan.Off))
        assertEquals(WakeTilePlan.Said.NotYet, say(WakePlan.Plan.NoRelay))
    }

    /** G7 review: a tap says whether the computer answered, not only that packets went. */
    @Test
    fun aTapSaysHowItWentUpToTheComputerAnswering() {
        assertEquals(WakeTilePlan.Said.Sending, WakeTilePlan.said(plan, Send(Step.Sending, 0), nowMs = 999_999))
        assertEquals(WakeTilePlan.Said.Waiting, WakeTilePlan.said(plan, Send(Step.Waiting, 0), nowMs = 999_999))
        assertEquals(WakeTilePlan.Said.Awake, WakeTilePlan.said(plan, Send(Step.Awake, 1_000), nowMs = 2_000))
        assertEquals(WakeTilePlan.Said.NoAnswer, WakeTilePlan.said(plan, Send(Step.NoAnswer, 1_000), nowMs = 2_000))
        assertEquals(WakeTilePlan.Said.Failed, WakeTilePlan.said(plan, Send(Step.Failed, 1_000), nowMs = 2_000))
        assertEquals(WakeTilePlan.Said.Ready, WakeTilePlan.said(plan, Send(Step.Awake, 1_000), nowMs = 1_000 + WakeTilePlan.SAID_MS + 1))
    }

    @Test
    fun aTapWaitsForTheOneUnderWayAndALongErrorIsShortened() {
        assertTrue(WakeTilePlan.mayStart(null))
        assertFalse(WakeTilePlan.mayStart(Send(Step.Waiting, 0)))
        assertTrue(WakeTilePlan.mayStart(Send(Step.Failed, 0)))
        assertEquals("no route", WakeTilePlan.shortWhy("no route"))
        assertNull(WakeTilePlan.shortWhy("x".repeat(49)))
    }
}
