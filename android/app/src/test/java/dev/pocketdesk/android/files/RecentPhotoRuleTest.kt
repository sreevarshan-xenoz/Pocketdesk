package dev.pocketdesk.android.files

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class RecentPhotoRuleTest {
    private fun row(id: Long, folder: String?, taken: Long = 0, added: Long = 0, pending: Boolean = false, name: String = "IMG_$id.jpg") =
        RecentPhotoRule.Row(id, name, folder, taken, added, 1000, pending)

    @Test
    fun onlyCameraPhotosAreListedNewestFirst() {
        val rows =
            listOf(
                row(1, "DCIM/Camera/", taken = 1_000),
                row(2, "Pictures/Screenshots/", taken = 5_000),
                row(3, "DCIM/Camera/", taken = 3_000),
                row(4, "Download/", taken = 9_000),
                row(5, "DCIM/", added = 2),
                row(6, "DCIM/Camera/", taken = 8_000, pending = true),
                row(7, null, taken = 9_500),
                row(8, "dcim/OpenCamera/", taken = 4_000),
            )
        // T4: newest by id, so pages follow on from each other.
        assertEquals(listOf(8L, 5L, 3L, 1L), RecentPhotoRule.listed(rows).map { it.id })
    }

    /** T4: a page says when more wait, and a photo of any page shown may be fetched. */
    @Test
    fun pagesSayWhenMoreWaitAndRememberWhatWasShown() {
        val rows = (1L..30L).map { row(it, "DCIM/Camera/") }
        val first = RecentPhotoRule.page(rows, lookedAll = true)
        assertEquals(30L, first.listed.first().id)
        assertEquals(7L, first.listed.last().id)
        assertTrue("six left over", first.more)
        val last = RecentPhotoRule.page(rows.filter { it.id < 7 }, lookedAll = true)
        assertEquals(6, last.listed.size)
        assertFalse(last.more)
        assertTrue("the query stopped short", RecentPhotoRule.page(rows.take(3), lookedAll = false).more)
        // A window of none it lists: the next page starts past them.
        val shots = (50L..60L).map { row(it, "DCIM/Screenshots/") }
        val none = RecentPhotoRule.page(shots, lookedAll = false)
        assertTrue(none.listed.isEmpty() && none.more)
        assertEquals(50L, none.next)
        val shown = RecentPhotoRule.remember(emptySet(), first.listed.map { it.id }, olderPage = false)
        val both = RecentPhotoRule.remember(shown, last.listed.map { it.id }, olderPage = true)
        assertTrue(RecentPhotoRule.mayFetch(30, both) && RecentPhotoRule.mayFetch(1, both))
        assertFalse("a newest page starts over", RecentPhotoRule.mayFetch(30, RecentPhotoRule.remember(both, listOf(31), olderPage = false)))
        val many = RecentPhotoRule.remember(emptySet(), (1L..1000L).toList(), olderPage = false)
        assertEquals(RecentPhotoRule.MAX_REMEMBERED, many.size)
    }

    @Test
    fun screenshotsAndHiddenFoldersInDcimAreNotCameraPhotos() {
        assertFalse(RecentPhotoRule.camera(row(1, "DCIM/Screenshots/")))
        assertFalse(RecentPhotoRule.camera(row(2, "DCIM/.thumbnails/")))
        assertFalse(RecentPhotoRule.camera(row(3, "DCIM/Camera/", name = ".hidden.jpg")))
        assertFalse(RecentPhotoRule.camera(row(4, "Private/DCIM/")))
        assertTrue(RecentPhotoRule.camera(row(5, "DCIM/Camera/")))
        assertTrue(RecentPhotoRule.camera(row(6, "DCIM/")))
    }

    @Test
    fun atMostTwentyFourAreListed() {
        val rows = (1L..40L).map { row(it, "DCIM/Camera/", taken = it) }
        val listed = RecentPhotoRule.listed(rows)
        assertEquals(RecentPhotoRule.MAX_PHOTOS, listed.size)
        assertEquals(40L, listed.first().id)
    }

    @Test
    fun onlyAListedPhotoIsFetched() {
        assertTrue(RecentPhotoRule.mayFetch(3, setOf(1, 3)))
        assertFalse("not in the last answer", RecentPhotoRule.mayFetch(2, setOf(1, 3)))
        assertFalse("nothing listed yet", RecentPhotoRule.mayFetch(1, emptySet()))
    }

    @Test
    fun aPhotoIsSampledDownNearTheThumbnailsSize() {
        assertEquals(1, RecentPhotoRule.sampleSize(300, 200))
        assertEquals(8, RecentPhotoRule.sampleSize(4000, 3000))
        assertEquals(1, RecentPhotoRule.sampleSize(0, 0))
    }
}
