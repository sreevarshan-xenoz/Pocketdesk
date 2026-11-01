package dev.pocketdesk.android.files

import dev.pocketdesk.android.service.FolderWalk
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class KeptRuleTest {
    private fun file(vararg path: String, size: Long = 10, modified: Long = 100) =
        FolderWalk.File(path.joinToString("/"), path.toList(), size, modified)

    private fun plan(vararg files: FolderWalk.File, listFailed: Int = 0, truncated: Boolean = false) =
        FolderWalk.Plan(files.toList(), 0, truncated, listFailed)

    private fun record(vararg files: FolderWalk.File) =
        files.associate { KeptRule.key(it.path) to KeptRule.Mark(it.size, it.modified) }

    @Test
    fun aNewFileIsSentAndOneThatIsUnchangedIsNot() {
        val a = file("a.txt")
        val b = file("d", "b.txt")
        val diff = KeptRule.diff(plan(a, b), record(a))
        assertEquals(listOf(b), diff.send)
        assertTrue(diff.gone.isEmpty())
        assertNull(diff.held)
    }

    @Test
    fun aFileChangedInSizeOrInTimeIsSentAgain() {
        val old = file("a.txt")
        assertEquals(1, KeptRule.diff(plan(file("a.txt", size = 11)), record(old)).send.size)
        assertEquals(1, KeptRule.diff(plan(file("a.txt", modified = 101)), record(old)).send.size)
        assertEquals(0, KeptRule.diff(plan(file("a.txt")), record(old)).send.size)
    }

    @Test
    fun aFileDeletedOnThePhoneIsNamedGone() {
        val a = file("a.txt")
        val b = file("d", "b.txt")
        val diff = KeptRule.diff(plan(a), record(a, b))
        assertEquals(listOf("d/b.txt"), diff.gone)
        assertNull(diff.held)
        assertEquals(listOf("d", "b.txt"), KeptRule.path(diff.gone.single()))
    }

    @Test
    fun nothingIsNamedGoneFromAPartOfTheFolder() {
        val a = file("a.txt")
        val b = file("d", "b.txt")
        // A folder the provider could not list, or a walk cut short.
        for (part in listOf(plan(a, listFailed = 1), plan(a, truncated = true))) {
            val diff = KeptRule.diff(part, record(a, b))
            assertTrue(diff.gone.isEmpty())
            assertEquals(KeptRule.Held.PART, diff.held)
        }
        // And a plan that found nothing at all is a part when it failed to list.
        val empty = KeptRule.diff(plan(listFailed = 1), record(a, b))
        assertTrue(empty.gone.isEmpty())
    }

    @Test
    fun aFolderEmptiedAtOnceHoldsItsDeletions() {
        val many = (1..40).map { file("f$it") }
        // 30 of 40 gone: a storage taken out, not a person deleting.
        val diff = KeptRule.diff(plan(*many.take(10).toTypedArray()), record(*many.toTypedArray()))
        assertEquals(KeptRule.Held.MANY, diff.held)
        assertTrue(diff.gone.isEmpty())
        // A few gone from a large folder is an ordinary deletion.
        val few = KeptRule.diff(plan(*many.drop(5).toTypedArray()), record(*many.toTypedArray()))
        assertNull(few.held)
        assertEquals(5, few.gone.size)
        // A small folder emptied is not "many".
        val small = KeptRule.diff(plan(), record(file("a"), file("b")))
        assertNull(small.held)
        assertEquals(2, small.gone.size)
    }

    @Test
    fun aRecordSurvivesItsTextAndASpoiledLineIsDropped() {
        val rec = record(file("a b.txt", size = 7, modified = 5), file("d", "é.txt", size = 9, modified = 0))
        assertEquals(rec, KeptRule.read(KeptRule.write(rec)))
        val read = KeptRule.read("7\t5\ta.txt\nnot a line\n7\tx\tb\n\t\t\n3\t4\tok\n")
        assertEquals(setOf("a.txt", "ok"), read.keys)
        assertTrue(KeptRule.read("").isEmpty())
    }

    @Test
    fun theScheduleNeedsWifiAndChargingByDefault() {
        assertEquals(KeptRule.Needs(unmetered = true, charging = true), KeptRule.needs(KeptRule.Mode.WIFI_CHARGING))
        assertEquals(KeptRule.Needs(unmetered = true, charging = false), KeptRule.needs(KeptRule.Mode.EVERY))
        assertTrue(KeptRule.mayStart(KeptRule.Mode.WIFI_CHARGING, unmetered = true, charging = true))
        assertFalse(KeptRule.mayStart(KeptRule.Mode.WIFI_CHARGING, unmetered = true, charging = false))
        assertFalse(KeptRule.mayStart(KeptRule.Mode.WIFI_CHARGING, unmetered = false, charging = true))
        assertTrue(KeptRule.mayStart(KeptRule.Mode.EVERY, unmetered = true, charging = false))
        assertFalse("never on mobile data", KeptRule.mayStart(KeptRule.Mode.EVERY, unmetered = false, charging = true))
    }

    @Test
    fun theJobComesRoundByTheChosenHoursOrTheDefault() {
        assertEquals(KeptRule.DEFAULT_HOURS, KeptRule.periodHours(KeptRule.Mode.WIFI_CHARGING, 1))
        assertEquals(3, KeptRule.periodHours(KeptRule.Mode.EVERY, 3))
        assertEquals(KeptRule.DEFAULT_HOURS, KeptRule.periodHours(KeptRule.Mode.EVERY, 5))
        assertTrue(KeptRule.HOURS.all { it >= 1 })
    }

    @Test
    fun aFoldersLabelIsWhatItsDocumentIdEndsIn() {
        assertEquals("WhatsApp", KeptRule.label("primary:Android/media/WhatsApp", "Storage"))
        assertEquals("Notes", KeptRule.label("primary:Notes", "Storage"))
        assertEquals("Storage", KeptRule.label("primary:", "Storage"))
        assertEquals("Storage", KeptRule.label(null, "Storage"))
        assertEquals(80, KeptRule.label("primary:" + "x".repeat(300), "Storage").length)
    }

    @Test
    fun theComputersFullAndOffRefusalsAreRecognised() {
        assertTrue(KeptRule.isFull("The computer refused the file: the kept folders on this computer are full: 9.0 GB of the 10 GB cap"))
        assertFalse(KeptRule.isFull("The computer refused the file: no free transfer slot"))
        assertFalse(KeptRule.isFull(null))
        assertTrue(KeptRule.isNotKept("The computer refused the file: Kept folders are turned off on this computer"))
        assertTrue(KeptRule.isNotKept("The computer refused the file: Kept folders take only the owner's own phones"))
        assertFalse(KeptRule.isNotKept("The computer refused the file: full"))
    }

    @Test
    fun aFolderIdIsThirtyTwoHexDigits() {
        assertTrue(KeptRule.validId("0123456789abcdef0123456789ABCDEF"))
        assertFalse(KeptRule.validId("0123"))
        assertFalse(KeptRule.validId("g".repeat(32)))
        assertFalse(KeptRule.validId(""))
    }
}
