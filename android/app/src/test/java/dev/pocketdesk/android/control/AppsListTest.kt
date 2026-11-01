package dev.pocketdesk.android.control

import dev.pocketdesk.android.control.AppsList.App
import dev.pocketdesk.android.control.AppsList.Icon
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class AppsListTest {
    private fun app(pkg: String, version: Long = 1) = App(pkg, pkg.substringAfterLast('.'), version)

    @Test
    fun `an icon is sent once, until the app is updated`() {
        val apps = listOf(app("org.example.a", 3), app("org.example.b", 9), app("org.example.c", 1))
        // Nothing kept: all go.
        assertEquals(apps, AppsList.missing(apps, emptyMap()))
        // The computer keeps a at its version and b at an older one: b and c go.
        val have = mapOf("org.example.a" to 3L, "org.example.b" to 8L)
        assertEquals(listOf(apps[1], apps[2]), AppsList.missing(apps, have))
        // All kept: none go.
        val all = apps.associate { it.pkg to it.version }
        assertTrue(AppsList.missing(apps, all).isEmpty())
        // An icon kept for an app no longer listed changes nothing.
        assertTrue(AppsList.missing(apps, all + ("org.example.gone" to 1L)).isEmpty())
    }

    @Test
    fun `icons go in batches held to a count and a size`() {
        val icons = (1..10).map { Icon("org.example.a$it", 1, "x".repeat(100)) }
        val byCount = AppsList.batches(icons, { it.png.length }, 4, 1_000_000)
        assertEquals(listOf(4, 4, 2), byCount.map { it.size })
        // By size: 100 each, 250 a batch holds two.
        val bySize = AppsList.batches(icons, { it.png.length }, 40, 250)
        assertEquals(List(5) { 2 }, bySize.map { it.size })
        // Order kept, none lost.
        assertEquals(icons, byCount.flatten())
        // One too big for a batch still goes, alone.
        val big = listOf(Icon("org.example.big", 1, "x".repeat(500)), icons[0])
        assertEquals(listOf(1, 1), AppsList.batches(big, { it.png.length }, 40, 250).map { it.size })
        assertTrue(AppsList.batches(emptyList<Icon>(), { 1 }, 4, 10).isEmpty())
    }

    @Test
    fun `the list and the icons are the computer's bodies`() {
        val list = AppsList.listBody(listOf(app("org.example.maps", 7)))
        val first = list.getJSONArray("apps").getJSONObject(0)
        assertEquals("org.example.maps", first.getString("id"))
        assertEquals("maps", first.getString("name"))
        assertEquals(7L, first.getLong("v"))
        val icons = AppsList.iconsBody(listOf(Icon("org.example.maps", 7, "AAAA")))
        val icon = icons.getJSONArray("icons").getJSONObject(0)
        assertEquals("org.example.maps", icon.getString("id"))
        assertEquals(7L, icon.getLong("v"))
        assertEquals("AAAA", icon.getString("png"))
    }

    @Test
    fun `a batch is never more icons than the computer takes`() {
        // core::protocol::phone_action::MAX_ICONS_PER_PACKET.
        assertEquals(40, AppsList.MAX_ICONS_PER_PACKET)
    }
}
