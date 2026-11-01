package dev.pocketdesk.android.control

import dev.pocketdesk.android.control.PhoneActionRule.Action
import dev.pocketdesk.android.control.PhoneActionRule.Decision
import dev.pocketdesk.android.control.ShareRule.Verdict
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ShareRuleTest {
    private fun ask(at: Long = 0, pkg: String? = "org.example.notes", name: String = "a.png", size: Long = 10) =
        ShareRule.Pending("192.0.2.7", name, size, pkg, at)

    private fun file(at: Long = 0, name: String = "a.png", size: Long = 10, address: String = "192.0.2.7") =
        ShareRule.Arrived(address, name, size, "content://media/external/downloads/1", "image/png", at)

    @Test
    fun `an ask and a file meet by computer, name and size, whichever comes first`() {
        var now = 0L
        val queue = ShareRule.Queue { now }
        assertNull(queue.offer(ask()))
        assertNotNull(queue.arrive(file()))
        // The other way round.
        assertNull(queue.arrive(file()))
        assertNotNull(queue.offer(ask()))
        // Each is used once.
        assertNull(queue.offer(ask()))
        assertNotNull(queue.arrive(file()))
        assertNull(queue.arrive(file()))
    }

    @Test
    fun `a file of another name, size or computer is not the one asked for`() {
        val queue = ShareRule.Queue { 0 }
        assertNull(queue.offer(ask()))
        assertNull(queue.arrive(file(name = "b.png")))
        assertNull(queue.arrive(file(size = 11)))
        assertNull(queue.arrive(file(address = "192.0.2.8")))
        assertNotNull(queue.arrive(file()))
    }

    @Test
    fun `what waits too long is forgotten`() {
        var now = 0L
        val queue = ShareRule.Queue { now }
        assertNull(queue.offer(ask(at = 0)))
        now = ShareRule.WAIT_MS + 1
        // A file that came after the ask went stale.
        assertNull(queue.arrive(file(at = now)))
        now = 2 * ShareRule.WAIT_MS + 5
        // An ask after a stale file.
        assertNull(queue.offer(ask(at = now)))
        assertNotNull(queue.arrive(file(at = now)))
    }

    @Test
    fun `no more than a few are kept waiting`() {
        val queue = ShareRule.Queue { 0 }
        for (i in 1..ShareRule.MAX_WAITING + 3) assertNull(queue.offer(ask(name = "f$i.png")))
        // The oldest asks went.
        assertNull(queue.arrive(file(name = "f1.png")))
        assertNotNull(queue.arrive(file(name = "f${ShareRule.MAX_WAITING + 3}.png")))
    }

    @Test
    fun `only an app with a launcher entry that takes the type is shared into`() {
        val own = "dev.pocketdesk.android"
        fun v(pkg: String?, launchable: Boolean = true, uri: Boolean = true, takes: Boolean = true) =
            ShareRule.verdict(pkg, own, launchable, uri, takes)
        assertEquals(Verdict.SHARE, v("org.example.notes"))
        assertEquals(Verdict.NO_APP, v(null))
        assertEquals(Verdict.NO_APP, v(""))
        // Never Pocketdesk itself.
        assertEquals(Verdict.NO_APP, v(own))
        assertEquals(Verdict.NO_APP, v("com.android.systemui"))
        assertEquals(Verdict.NO_APP, v("org.example.hidden", launchable = false))
        assertEquals(Verdict.CANNOT_TAKE, v("org.example.notes", takes = false))
        assertEquals(Verdict.CANNOT_TAKE, v("org.example.notes", uri = false))
    }

    @Test
    fun `the app to share into is the one in front when the ask came, only if it still is`() {
        assertEquals("org.example.notes", ShareRule.frontStill("org.example.notes", "org.example.notes"))
        assertNull(ShareRule.frontStill("org.example.notes", "org.example.chat"))
        assertNull(ShareRule.frontStill("org.example.notes", null))
        assertNull(ShareRule.frontStill(null, null))
        assertNull(ShareRule.frontStill("", ""))
        // Changed since the ask: no app, so the file stays in Downloads.
        assertEquals(Verdict.NO_APP, ShareRule.verdict(ShareRule.frontStill("org.example.notes", "org.example.chat"), "dev.pocketdesk.android", true, true, true))
    }

    @Test
    fun `the type is the files own when it reads as one`() {
        assertEquals("image/png", ShareRule.mimeOf("image/png"))
        assertEquals("image/png", ShareRule.mimeOf(" IMAGE/PNG "))
        assertEquals("application/vnd.example+json", ShareRule.mimeOf("application/vnd.example+json"))
        for (bad in listOf(null, "", "image/*", "*/*", "image", "image/png; x=1", "a b/c", "../x/y")) {
            assertEquals("$bad", "application/octet-stream", ShareRule.mimeOf(bad))
        }
    }

    @Test
    fun `a file name to share is one line with no path`() {
        assertTrue(ShareRule.validName("photo 1.png"))
        assertFalse(ShareRule.validName(""))
        assertFalse(ShareRule.validName("a/b.png"))
        assertFalse(ShareRule.validName("a\\b.png"))
        assertFalse(ShareRule.validName("a\nb.png"))
        assertFalse(ShareRule.validName("x".repeat(256)))
    }

    @Test
    fun `a share is allowed only with the switch, the own computer, the service and an open phone`() {
        fun d(
            own: Boolean = true,
            service: Boolean = true,
            appsOn: Boolean = true,
            locked: Boolean = false,
            awake: Boolean = true,
            secure: Boolean = true,
        ) = PhoneActionRule.decide(
            Action.SHARE, true, own, service, 34, locked, awake, true, true, appsOn,
            secureLock = secure,
        )
        assertEquals(Decision.DO, d())
        assertEquals(Decision.OFF, d(appsOn = false))
        assertEquals(Decision.NOT_OWN, d(own = false))
        assertEquals(Decision.NO_SERVICE, d(service = false))
        assertEquals(Decision.LOCKED_SECURE, d(locked = true))
        assertEquals(Decision.LOCKED, d(locked = true, secure = false))
        assertEquals(Decision.ASLEEP, d(awake = false))
        // The words the computer is told; the own computer only.
        assertEquals(AppOpenRule.CONTROL_OFF, AppOpenRule.tell(Decision.NO_SERVICE, own = true))
        assertNull(AppOpenRule.tell(Decision.NO_SERVICE, own = false))
        assertEquals(Action.SHARE, Action.parse("share"))
    }

    @Test
    fun `a report of a share names the file and the app, and the others do not`() {
        val share = AppOpenRule.report("share", "org.example.notes", AppOpenRule.SHARED, "a.png")
        assertEquals("share", share.getString("about"))
        assertEquals("org.example.notes", share.getString("app"))
        assertEquals("a.png", share.getString("name"))
        assertEquals("shared", share.getString("result"))
        val open = AppOpenRule.report("open", "org.example.notes", AppOpenRule.OPENED)
        assertFalse(open.has("name"))
        val none = AppOpenRule.report("share", null, AppOpenRule.NO_APP, "a.png")
        assertFalse(none.has("app"))
    }
}
