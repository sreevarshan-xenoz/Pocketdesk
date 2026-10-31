package dev.pocketdesk.android

import android.content.Context
import android.content.ContextWrapper
import android.content.SharedPreferences
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * UX1 (PR #575 review I2): a computer's last-used time is kept with its
 * other per-address records: recorded when a session comes up, moved with
 * the card, forgotten with it.
 */
class LastUsedStoreTest {
    private val ctx = FakeContext()
    private val a = HostStore.Host("192.0.2.1", HostStore.DEFAULT_STREAMING_PORT)
    private val b = HostStore.Host("192.0.2.2", HostStore.DEFAULT_STREAMING_PORT)

    @Test
    fun `saving a computer does not count as using it`() {
        HostStore.save(ctx, a)
        assertNull(HostStore.lastUsed(ctx, a.address, now = 10_000L))
    }

    @Test
    fun `a session coming up records the time`() {
        HostStore.save(ctx, a)
        HostStore.markUsed(ctx, a.address, now = 5_000L)
        assertEquals(5_000L, HostStore.lastUsed(ctx, a.address, now = 10_000L))
    }

    @Test
    fun `an unsaved address records nothing`() {
        HostStore.markUsed(ctx, a.address, now = 5_000L)
        assertNull(HostStore.lastUsed(ctx, a.address, now = 10_000L))
    }

    @Test
    fun `a time far ahead of now is not shown`() {
        HostStore.save(ctx, a)
        HostStore.markUsed(ctx, a.address, now = 60 * 60_000L)
        assertNull(HostStore.lastUsed(ctx, a.address, now = 0L))
        assertEquals(60 * 60_000L, HostStore.lastUsed(ctx, a.address, now = 59 * 60_000L))
    }

    @Test
    fun `it moves with the card and goes with it`() {
        HostStore.save(ctx, a)
        HostStore.markUsed(ctx, a.address, now = 5_000L)
        HostStore.move(ctx, a.address, b)
        assertNull(HostStore.lastUsed(ctx, a.address, now = 10_000L))
        assertEquals(5_000L, HostStore.lastUsed(ctx, b.address, now = 10_000L))
        HostStore.remove(ctx, b.address)
        assertNull(HostStore.lastUsed(ctx, b.address, now = 10_000L))
        // Saved again, it starts with no time.
        HostStore.save(ctx, b)
        assertNull(HostStore.lastUsed(ctx, b.address, now = 10_000L))
    }

    @Test
    fun `a recorded time is announced for Home to read again`() {
        HostStore.save(ctx, a)
        val before = HostStore.usedStamps
        HostStore.markUsed(ctx, a.address, now = 5_000L)
        assertTrue("a stamp bumps the count", HostStore.usedStamps > before)
        val after = HostStore.usedStamps
        HostStore.markUsed(ctx, b.address, now = 6_000L)
        assertEquals("an unsaved address announces nothing", after, HostStore.usedStamps)
    }
}

/** Only what HostStore asks of a Context: its preferences, kept in memory. */
private class FakeContext : ContextWrapper(null) {
    private val files = mutableMapOf<String, FakePrefs>()

    override fun getSharedPreferences(name: String, mode: Int): SharedPreferences = files.getOrPut(name) { FakePrefs() }

    override fun getApplicationContext(): Context = this
}

private class FakePrefs : SharedPreferences {
    private val values = mutableMapOf<String, Any?>()

    override fun getAll(): Map<String, *> = values.toMap()
    override fun getString(key: String, defValue: String?): String? = values[key] as String? ?: defValue

    @Suppress("UNCHECKED_CAST")
    override fun getStringSet(key: String, defValues: Set<String>?): Set<String>? = values[key] as Set<String>? ?: defValues
    override fun getInt(key: String, defValue: Int): Int = values[key] as Int? ?: defValue
    override fun getLong(key: String, defValue: Long): Long = values[key] as Long? ?: defValue
    override fun getFloat(key: String, defValue: Float): Float = values[key] as Float? ?: defValue
    override fun getBoolean(key: String, defValue: Boolean): Boolean = values[key] as Boolean? ?: defValue
    override fun contains(key: String): Boolean = key in values
    override fun edit(): SharedPreferences.Editor = Editor()
    override fun registerOnSharedPreferenceChangeListener(l: SharedPreferences.OnSharedPreferenceChangeListener) {}
    override fun unregisterOnSharedPreferenceChangeListener(l: SharedPreferences.OnSharedPreferenceChangeListener) {}

    private inner class Editor : SharedPreferences.Editor {
        private val puts = mutableMapOf<String, Any?>()
        private val removes = mutableSetOf<String>()
        private var clear = false

        override fun putString(key: String, value: String?) = apply { puts[key] = value }
        override fun putStringSet(key: String, values: Set<String>?) = apply { puts[key] = values }
        override fun putInt(key: String, value: Int) = apply { puts[key] = value }
        override fun putLong(key: String, value: Long) = apply { puts[key] = value }
        override fun putFloat(key: String, value: Float) = apply { puts[key] = value }
        override fun putBoolean(key: String, value: Boolean) = apply { puts[key] = value }
        override fun remove(key: String) = apply { removes += key }
        override fun clear() = apply { clear = true }
        override fun commit(): Boolean {
            apply()
            return true
        }

        override fun apply() {
            if (clear) values.clear()
            removes.forEach { values.remove(it) }
            puts.forEach { (k, v) -> if (v == null) values.remove(k) else values[k] = v }
        }
    }
}
