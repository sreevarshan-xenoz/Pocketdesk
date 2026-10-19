package dev.pocketdesk.android.shortcuts

import dev.pocketdesk.android.wake.WakePlan
import org.junit.Assert.assertEquals
import org.junit.Test

class WakeTilePlanTest {
    @Test
    fun theTileSaysWhetherAWakeCanGoFromHere() {
        val say = { plan: WakePlan.Plan? -> WakeTilePlan.said(plan, sent = null) }
        assertEquals(WakeTilePlan.Said.NoComputer, say(null))
        assertEquals(WakeTilePlan.Said.Ready, say(WakePlan.Plan.Direct("00:00:5e:00:53:01", listOf("192.0.2.255"))))
        assertEquals(WakeTilePlan.Said.Ready, say(WakePlan.Plan.Relay("192.0.2.2", "00:00:5e:00:53:01", "192.0.2.255")))
        assertEquals(WakeTilePlan.Said.Ready, say(WakePlan.Plan.Away))
        assertEquals(WakeTilePlan.Said.NotYet, say(WakePlan.Plan.NoMac))
        assertEquals(WakeTilePlan.Said.Off, say(WakePlan.Plan.Off))
        assertEquals(WakeTilePlan.Said.NotYet, say(WakePlan.Plan.NoRelay))
    }

    @Test
    fun aSendSaysHowItWentForAWhile() {
        val plan = WakePlan.Plan.Direct("00:00:5e:00:53:01", listOf("192.0.2.255"))
        assertEquals(WakeTilePlan.Said.Sent, WakeTilePlan.said(plan, sent = WakeTilePlan.Send(ok = true, atMs = 1_000), nowMs = 2_000))
        assertEquals(WakeTilePlan.Said.Failed, WakeTilePlan.said(plan, sent = WakeTilePlan.Send(ok = false, atMs = 1_000), nowMs = 2_000))
        assertEquals(WakeTilePlan.Said.Ready, WakeTilePlan.said(plan, sent = WakeTilePlan.Send(ok = true, atMs = 1_000), nowMs = 1_000 + WakeTilePlan.SAID_MS + 1))
    }
}
