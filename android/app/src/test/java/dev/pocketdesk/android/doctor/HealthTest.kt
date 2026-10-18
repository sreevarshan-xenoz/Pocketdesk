package dev.pocketdesk.android.doctor

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class HealthTest {
    /** Everything on and allowed: no problem. */
    private val healthy = HealthSnapshot(
        listenerFeatures = true,
        listenerAccess = true,
        listenerConnected = true,
        textsShow = true,
        receiveSms = true,
        textsReply = true,
        sendSms = true,
        contactNames = true,
        readContacts = true,
        photoFeatures = true,
        photoAccess = true,
        postingFeatures = true,
        mayPost = true,
        control = true,
        controlService = true,
        advancedProtection = false,
        downloadFolderLost = false,
    )

    private val nothingOn = HealthSnapshot(
        listenerFeatures = false,
        listenerAccess = false,
        listenerConnected = false,
        textsShow = false,
        receiveSms = false,
        textsReply = false,
        sendSms = false,
        contactNames = false,
        readContacts = false,
        photoFeatures = false,
        photoAccess = false,
        postingFeatures = false,
        mayPost = false,
        control = false,
        controlService = false,
        advancedProtection = true,
        downloadFolderLost = false,
    )

    @Test
    fun `nothing is wrong when everything on is allowed, or when nothing is on`() {
        assertEquals(emptyList<HealthProblem>(), Health.problems(healthy))
        assertEquals(emptyList<HealthProblem>(), Health.problems(nothingOn))
    }

    @Test
    fun `listener features without notification access`() {
        val s = healthy.copy(listenerAccess = false, listenerConnected = false)
        // Without access the listener cannot run: one problem, not two.
        assertEquals(listOf(HealthProblem.NOTIFICATION_ACCESS), Health.problems(s))
    }

    @Test
    fun `a listener that never connected is stopped, but only once it is known`() {
        assertEquals(
            listOf(HealthProblem.LISTENER_STOPPED),
            Health.problems(healthy.copy(listenerConnected = false)),
        )
        // Not known yet (a rebind asked for, its answer awaited): nothing said.
        assertEquals(emptyList<HealthProblem>(), Health.problems(healthy.copy(listenerConnected = null)))
        // A stopped listener matters only while a feature needs it.
        assertEquals(
            emptyList<HealthProblem>(),
            Health.problems(healthy.copy(listenerFeatures = false, listenerConnected = false)),
        )
    }

    @Test
    fun `each runtime permission a switch needs`() {
        assertEquals(listOf(HealthProblem.TEXTS_RECEIVE), Health.problems(healthy.copy(receiveSms = false)))
        assertEquals(listOf(HealthProblem.TEXTS_SEND), Health.problems(healthy.copy(sendSms = false)))
        assertEquals(listOf(HealthProblem.CONTACTS), Health.problems(healthy.copy(readContacts = false)))
        assertEquals(listOf(HealthProblem.PHOTOS), Health.problems(healthy.copy(photoAccess = false)))
        assertEquals(listOf(HealthProblem.APP_NOTIFICATIONS), Health.problems(healthy.copy(mayPost = false)))
        // Off switches need nothing.
        assertEquals(
            emptyList<HealthProblem>(),
            Health.problems(
                healthy.copy(
                    textsShow = false, receiveSms = false, textsReply = false, sendSms = false,
                    contactNames = false, readContacts = false, photoFeatures = false, photoAccess = false,
                    postingFeatures = false, mayPost = false,
                ),
            ),
        )
    }

    @Test
    fun `the control service off, or blocked by Advanced Protection`() {
        assertEquals(listOf(HealthProblem.CONTROL_SERVICE), Health.problems(healthy.copy(controlService = false)))
        assertEquals(
            listOf(HealthProblem.CONTROL_BLOCKED),
            Health.problems(healthy.copy(controlService = false, advancedProtection = true)),
        )
        // A service that still runs under Advanced Protection (an accessibility tool) is fine.
        assertEquals(emptyList<HealthProblem>(), Health.problems(healthy.copy(advancedProtection = true)))
        assertEquals(
            emptyList<HealthProblem>(),
            Health.problems(healthy.copy(control = false, controlService = false)),
        )
    }

    @Test
    fun `a chosen download folder whose grant is gone`() {
        assertEquals(listOf(HealthProblem.DOWNLOAD_FOLDER), Health.problems(healthy.copy(downloadFolderLost = true)))
    }

    @Test
    fun `problems come in the table's order`() {
        val all = Health.problems(
            healthy.copy(
                listenerAccess = false, receiveSms = false, sendSms = false, readContacts = false,
                photoAccess = false, mayPost = false, controlService = false, downloadFolderLost = true,
            ),
        )
        assertEquals(
            listOf(
                HealthProblem.NOTIFICATION_ACCESS, HealthProblem.TEXTS_RECEIVE, HealthProblem.TEXTS_SEND,
                HealthProblem.CONTACTS, HealthProblem.PHOTOS, HealthProblem.APP_NOTIFICATIONS,
                HealthProblem.CONTROL_SERVICE, HealthProblem.DOWNLOAD_FOLDER,
            ),
            all,
        )
    }

    @Test
    fun `Not now hides a set until it changes`() {
        val one = listOf(HealthProblem.PHOTOS)
        val two = listOf(HealthProblem.PHOTOS, HealthProblem.CONTACTS)
        assertTrue(Health.shows(one, dismissed = null))
        assertFalse(Health.shows(one, dismissed = Health.key(one)))
        // A new problem shows the card again.
        assertTrue(Health.shows(two, dismissed = Health.key(one)))
        // No problem, no card; and the key does not depend on the order.
        assertFalse(Health.shows(emptyList(), dismissed = null))
        assertEquals(Health.key(two), Health.key(two.reversed()))
    }

    @Test
    fun `a system update is a build other than the last one seen`() {
        assertTrue(Health.systemUpdated("a/1", "a/2"))
        assertFalse(Health.systemUpdated("a/1", "a/1"))
        assertFalse(Health.systemUpdated(null, "a/1"))
        assertFalse(Health.systemUpdated("a/1", null))
    }

    @Test
    fun `an update notifies only of what is new`() {
        val photos = listOf(HealthProblem.PHOTOS)
        val both = listOf(HealthProblem.PHOTOS, HealthProblem.NOTIFICATION_ACCESS)
        assertTrue(Health.notifyAfterUpdate(photos, seen = null, dismissed = null))
        // Seen on Home before the update, or put off: not news.
        assertFalse(Health.notifyAfterUpdate(photos, seen = Health.key(photos), dismissed = null))
        assertFalse(Health.notifyAfterUpdate(photos, seen = null, dismissed = Health.key(photos)))
        // Something the update took away besides.
        assertTrue(Health.notifyAfterUpdate(both, seen = Health.key(photos), dismissed = null))
        assertFalse(Health.notifyAfterUpdate(emptyList(), seen = null, dismissed = null))
    }

    @Test
    fun `a feature that needs access but not the listener running`() {
        val media = nothingOn.copy(accessFeatures = true, advancedProtection = false)
        assertEquals(listOf(HealthProblem.NOTIFICATION_ACCESS), Health.problems(media))
        // With access, a listener not running does not stop it.
        assertEquals(
            emptyList<HealthProblem>(),
            Health.problems(media.copy(listenerAccess = true, listenerConnected = false)),
        )
    }

    @Test
    fun `screenshots that turned themselves off are said`() {
        assertEquals(
            listOf(HealthProblem.SCREENSHOTS_STOPPED),
            Health.problems(nothingOn.copy(screenshotsStopped = true, advancedProtection = false)),
        )
    }
}
