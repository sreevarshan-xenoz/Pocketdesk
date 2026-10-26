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
        val hosts = listOf(host("192.0.2.1"), host("192.0.2.2"), host("2001:db8::7"))
        val token = "0123456789abcdef"
        fun share(id: String) = ShortcutPlan.shortcutHost(id, hosts, token)
        val id = ShortcutPlan.Entry(ShortcutPlan.Kind.CONNECT, "192.0.2.2", token).id
        assertEquals(ShortcutPlan.Share.To(host("192.0.2.2")), share(id))
        assertEquals(ShortcutPlan.Share.To(host("2001:db8::7")), share("connect:$token:2001:db8::7"))
        assertEquals(ShortcutPlan.Share.NotSaved, share("connect:$token:192.0.2.9"))
        assertEquals(ShortcutPlan.Share.NotSaved, share("wake:192.0.2.1"))
        // Another app knows the format, not the token (N2 review).
        assertEquals(ShortcutPlan.Share.NotSaved, share("connect:192.0.2.2"))
        assertEquals(ShortcutPlan.Share.NotSaved, share("connect:fedcba9876543210:192.0.2.2"))
        assertEquals(ShortcutPlan.Share.NotSaved, ShortcutPlan.shortcutHost("connect::192.0.2.2", hosts, ""))
    }

    /**
     * AZ1: a paired computer's shortcut is named by its paired id, so it
     * stays its own across a move and no other computer takes it over.
     */
    @Test
    fun aPairedComputersShortcutIsNamedByItsPairedId() {
        val token = "0123456789abcdef"
        assertEquals(
            "connect:$token:paired:pc-a@192.0.2.1",
            ShortcutPlan.Entry(ShortcutPlan.Kind.CONNECT, "192.0.2.1", token, "pc-a").id,
        )
        assertEquals(
            "wake:paired:pc-a@2001:db8::7",
            ShortcutPlan.Entry(ShortcutPlan.Kind.WAKE, "2001:db8::7", pairedId = "pc-a").id,
        )
        assertEquals(
            ShortcutPlan.Target.Paired("pc-a", "192.0.2.1"),
            ShortcutPlan.targetOf("connect:$token:paired:pc-a@192.0.2.1"),
        )
        assertEquals(
            ShortcutPlan.Target.Paired("pc-a", "2001:db8::7"),
            ShortcutPlan.targetOf("wake:paired:pc-a@2001:db8::7"),
        )
        assertEquals(ShortcutPlan.Target.Address("2001:db8::7"), ShortcutPlan.targetOf("connect:$token:2001:db8::7"))
        assertEquals(ShortcutPlan.Target.Address("192.0.2.1"), ShortcutPlan.targetOf("wake:192.0.2.1"))
        assertEquals(ShortcutPlan.Kind.WAKE, ShortcutPlan.kindOf("wake:paired:pc-a@192.0.2.1"))
        assertEquals(ShortcutPlan.Kind.CONNECT, ShortcutPlan.kindOf("connect:$token:paired:pc-a@192.0.2.1"))
        val plan = ShortcutPlan.plan(
            listOf(host("192.0.2.1"), host("192.0.2.2")),
            hasWake = { it == "192.0.2.1" },
            token = token,
            pairedId = { if (it == "192.0.2.1") "pc-a" else null },
        )
        assertEquals(
            listOf("connect:$token:paired:pc-a@192.0.2.1", "connect:$token:192.0.2.2", "wake:paired:pc-a@192.0.2.1"),
            plan.map { it.id },
        )
    }

    /**
     * AZ1: a shortcut made for a paired computer reaches it at its new
     * address, and never another computer saved later at its old one.
     */
    @Test
    fun aShortcutFollowsItsComputerAndNeverAnotherAtItsAddress() {
        val token = "0123456789abcdef"
        // A moved from .1 to .9; B was paired later at .1.
        val paired = mapOf("192.0.2.9" to "pc-a", "192.0.2.1" to "pc-b")
        val hosts = listOf(host("192.0.2.1"), host("192.0.2.9"))
        assertEquals(host("192.0.2.9"), ShortcutPlan.savedHost("192.0.2.1", hosts, "pc-a", paired::get))
        assertNull(ShortcutPlan.savedHost("192.0.2.1", hosts, "pc-gone", paired::get))
        assertEquals(
            "one made before AZ1 names an address",
            host("192.0.2.1"),
            ShortcutPlan.savedHost("192.0.2.1", hosts, null, paired::get),
        )
        fun share(id: String) = ShortcutPlan.shortcutHost(id, hosts, token, paired::get)
        assertEquals(ShortcutPlan.Share.To(host("192.0.2.9")), share("connect:$token:paired:pc-a@192.0.2.1"))
        assertEquals(ShortcutPlan.Share.NotSaved, share("connect:$token:paired:pc-gone@192.0.2.1"))
        assertEquals(ShortcutPlan.Share.NotSaved, share("connect:fedcba9876543210:paired:pc-a@192.0.2.9"))
        assertEquals(ShortcutPlan.Share.NotSaved, share("wake:paired:pc-a@192.0.2.9"))
    }

    /**
     * AZ1 review: a computer with two cards (its LAN and tailnet addresses)
     * has a shortcut for each, and each goes to its own card.
     */
    @Test
    fun aComputersTwoCardsKeepTheirOwnShortcuts() {
        val token = "0123456789abcdef"
        val paired = mapOf("192.0.2.1" to "pc-a", "198.51.100.7" to "pc-a")
        val hosts = listOf(host("198.51.100.7"), host("192.0.2.1"))
        val plan = ShortcutPlan.plan(hosts, hasWake = { it == "192.0.2.1" }, token = token, pairedId = paired::get)
        assertEquals(3, plan.map { it.id }.distinct().size)
        val wake = plan.single { it.kind == ShortcutPlan.Kind.WAKE }
        assertEquals("the LAN card's wake wakes by it", host("192.0.2.1"), ShortcutPlan.savedHost(wake.address, hosts, "pc-a", paired::get))
        fun fate(id: String, address: String) = ShortcutPlan.pinnedFate(id, address, hosts, paired::get)
        assertEquals(ShortcutPlan.Pinned.Keep, fate(wake.id, "192.0.2.1"))
        assertEquals(ShortcutPlan.Pinned.Keep, fate("connect:$token:paired:pc-a@198.51.100.7", "198.51.100.7"))
        // Two cards of one computer never collide on one id.
        assertEquals(
            listOf("connect:$token:paired:pc-a@198.51.100.7", "connect:$token:paired:pc-a@192.0.2.1"),
            plan.filter { it.kind == ShortcutPlan.Kind.CONNECT }.map { it.id },
        )
    }

    /** AZ1: at a refresh, a pinned shortcut is kept, pointed at its computer's new address, or turned off. */
    @Test
    fun aPinnedShortcutIsKeptMovedOrTurnedOff() {
        val token = "0123456789abcdef"
        val paired = mapOf("192.0.2.9" to "pc-a", "192.0.2.1" to "pc-b")
        val hosts = listOf(host("192.0.2.1"), host("192.0.2.9"))
        fun fate(id: String, address: String?) = ShortcutPlan.pinnedFate(id, address, hosts, paired::get)
        assertEquals(ShortcutPlan.Pinned.Keep, fate("connect:$token:paired:pc-b@192.0.2.1", "192.0.2.1"))
        // A at .1 moved to .9; B is paired at .1 now.
        assertEquals(ShortcutPlan.Pinned.Move(host("192.0.2.9")), fate("connect:$token:paired:pc-a@192.0.2.1", "192.0.2.1"))
        assertEquals(ShortcutPlan.Pinned.Move(host("192.0.2.9")), fate("wake:paired:pc-a@192.0.2.1", null))
        assertEquals(ShortcutPlan.Pinned.Off, fate("wake:paired:pc-gone@192.0.2.5", "192.0.2.5"))
        // Made before AZ1: by its address, as before.
        assertEquals(ShortcutPlan.Pinned.Keep, fate("connect:$token:192.0.2.9", "192.0.2.9"))
        assertEquals(ShortcutPlan.Pinned.Off, fate("connect:$token:192.0.2.5", "192.0.2.5"))
    }

    @Test
    fun aShortcutIdNamesItsAddressWithOrWithoutAToken() {
        assertEquals("192.0.2.2", ShortcutPlan.addressOf("connect:0123456789abcdef:192.0.2.2"))
        assertEquals("192.0.2.2", ShortcutPlan.addressOf("connect:192.0.2.2"))
        assertEquals("2001:db8::7", ShortcutPlan.addressOf("connect:0123456789abcdef:2001:db8::7"))
        assertEquals("2001:db8::7", ShortcutPlan.addressOf("wake:2001:db8::7"))
    }

    /** The category in shortcuts.xml is the one the code publishes (N2 review). */
    @Test
    fun theShareTargetNamesTheCategoryTheShortcutsCarry() {
        val xml = java.io.File("src/main/res/xml/shortcuts.xml").readText()
        assertTrue(xml.contains("\"${ShortcutPlan.SHARE_CATEGORY}\""))
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
