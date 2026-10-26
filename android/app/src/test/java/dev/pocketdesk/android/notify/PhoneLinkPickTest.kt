package dev.pocketdesk.android.notify

import dev.pocketdesk.android.HostStore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class PhoneLinkPickTest {
    private fun host(address: String) = HostStore.Host(address, 4716)

    /**
     * BB2: the phone's own computer is the first paired one that has not
     * said the phone is not paired with it.
     */
    @Test
    fun `a computer that unpaired the phone is not its own`() {
        val hosts = listOf(host("192.0.2.1"), host("192.0.2.2"), host("192.0.2.3"))
        val paired = mapOf("192.0.2.1" to "pc-a", "192.0.2.3" to "pc-c")
        assertEquals(host("192.0.2.1"), PhoneLink.pick(hosts, paired::get) { false })
        assertEquals(
            "the next paired one",
            host("192.0.2.3"),
            PhoneLink.pick(hosts, paired::get) { it == "192.0.2.1" },
        )
        assertNull(PhoneLink.pick(hosts, paired::get) { true })
    }
}
