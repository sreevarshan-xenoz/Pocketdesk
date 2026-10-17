package dev.pocketdesk.android.shortcuts

import dev.pocketdesk.android.HostStore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.assertNull
import org.junit.Test

class ShortcutPlanTest {
    private fun host(address: String) = HostStore.Host(address, 4716)

    @Test
    fun connectingComesFirstInRecentOrderThenWaking() {
        val hosts = listOf(host("10.0.0.1"), host("10.0.0.2"), host("10.0.0.3"))
        val plan = ShortcutPlan.plan(hosts, hasWake = { it != "10.0.0.2" })
        assertEquals(
            listOf(
                ShortcutPlan.Entry(ShortcutPlan.Kind.CONNECT, "10.0.0.1"),
                ShortcutPlan.Entry(ShortcutPlan.Kind.CONNECT, "10.0.0.2"),
                ShortcutPlan.Entry(ShortcutPlan.Kind.CONNECT, "10.0.0.3"),
                ShortcutPlan.Entry(ShortcutPlan.Kind.WAKE, "10.0.0.1"),
            ),
            plan,
        )
    }

    @Test
    fun aLauncherShowsAtMostFour() {
        val hosts = (1..6).map { host("10.0.0.$it") }
        val plan = ShortcutPlan.plan(hosts, hasWake = { true })
        assertEquals(4, plan.size)
        assertEquals(ShortcutPlan.Kind.CONNECT, plan.last().kind)
    }

    /** N2: a Direct Share target goes to its own saved computer, never another. */
    @Test
    fun aDirectShareTargetGoesToItsSavedComputerOnly() {
        val hosts = listOf(host("192.0.2.1"), host("192.0.2.2"))
        assertEquals(ShortcutPlan.Share.To(host("192.0.2.2")), ShortcutPlan.shortcutHost("connect:192.0.2.2", hosts))
        assertEquals(ShortcutPlan.Share.NotSaved, ShortcutPlan.shortcutHost("connect:192.0.2.9", hosts))
        assertEquals(ShortcutPlan.Share.NotSaved, ShortcutPlan.shortcutHost("wake:192.0.2.1", hosts))
        assertEquals(ShortcutPlan.Share.NotSaved, ShortcutPlan.shortcutHost("192.0.2.1", hosts))
    }

    @Test
    fun noComputersNoShortcuts() {
        assertEquals(emptyList<ShortcutPlan.Entry>(), ShortcutPlan.plan(emptyList(), hasWake = { true }))
    }

    @Test
    fun eachShortcutHasAStableIdForItsComputerAndAction() {
        assertEquals("connect:10.0.0.1", ShortcutPlan.Entry(ShortcutPlan.Kind.CONNECT, "10.0.0.1").id)
        assertEquals("wake:10.0.0.1", ShortcutPlan.Entry(ShortcutPlan.Kind.WAKE, "10.0.0.1").id)
    }

    @Test
    fun aRequestOpensOnlyAComputerAlreadySaved() {
        val hosts = listOf(host("10.0.0.1"), host("mypc.local"))
        assertEquals(hosts[1], ShortcutPlan.savedHost("mypc.local", hosts))
        assertNull("another app cannot add a computer this way", ShortcutPlan.savedHost("10.9.9.9", hosts))
        assertNull(ShortcutPlan.savedHost(null, hosts))
    }

    @Test
    fun aShareGoesToTheComputerAskedForOrTheLastOneUsed() {
        val hosts = listOf(host("10.0.0.1"), host("mypc.local"))
        val share = ShortcutPlan::shareHost
        assertEquals(ShortcutPlan.Share.To(hosts[1]), share("mypc.local", hosts, true))
        assertEquals("none asked for: the last one used", ShortcutPlan.Share.To(hosts[0]), share(null, hosts, true))
        assertEquals(
            "an address that is not saved never falls back to another computer",
            ShortcutPlan.Share.NotSaved,
            share("10.9.9.9", hosts, true),
        )
        assertEquals(ShortcutPlan.Share.NotSaved, share("", hosts, true))
        assertEquals(ShortcutPlan.Share.NoneSaved, share(null, emptyList(), true))
    }

    @Test
    fun onlyFilesAnotherAppSharesAreSentNeverAPathItNames() {
        assertTrue(ShortcutPlan.shareable(listOf("content", "content")))
        // A file:// path is opened with this app's own rights: its keys.
        assertFalse(ShortcutPlan.shareable(listOf("content", "file")))
        assertFalse(ShortcutPlan.shareable(listOf("android.resource")))
        assertFalse(ShortcutPlan.shareable(listOf(null)))
    }

    @Test
    fun otherAppsChooseAComputerOnlyWhenThePersonLetsThem() {
        val hosts = listOf(host("10.0.0.1"), host("mypc.local"))
        assertEquals(
            "off: a share naming a computer is refused, never sent elsewhere",
            ShortcutPlan.Share.NotAllowed,
            ShortcutPlan.shareHost("mypc.local", hosts, false),
        )
        assertEquals(
            "a share that names none is not choosing",
            ShortcutPlan.Share.To(hosts[0]),
            ShortcutPlan.shareHost(null, hosts, false),
        )
    }
}
