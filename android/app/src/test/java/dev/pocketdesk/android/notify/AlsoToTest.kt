package dev.pocketdesk.android.notify

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AlsoToTest {
    private fun s(address: String, id: String?) = AlsoTo.Saved(address, id)

    @Test
    fun onlyPickedPairedOtherComputersGetIt() {
        val saved = listOf(s("own", "A"), s("b", "B"), s("c", "C"), s("unpaired", null), s("d", "D"), s("e", "E"), s("b", "B"))
        val picked = setOf("A", "B", "C", "D", "E")
        assertEquals(listOf("b", "c", "d"), AlsoTo.targets(saved, picked, own = "own"))
        assertTrue("not picked", AlsoTo.targets(saved, emptySet(), "own").isEmpty())
    }

    @Test
    fun aPickFollowsTheComputerNotItsAddress() {
        // B was picked; another computer now answers at B's old address.
        val saved = listOf(s("own", "A"), s("10.0.0.2", "Z"))
        assertTrue(AlsoTo.targets(saved, setOf("B"), "own").isEmpty())
    }

    @Test
    fun theOwnComputerKeepsItsPickForWhenAnotherIsOwn() {
        val saved = listOf(s("a", "A"), s("b", "B"))
        val picked = setOf("A", "B")
        assertEquals("at home, A is own", listOf("b"), AlsoTo.targets(saved, picked, own = "a"))
        assertEquals("at work, B is own", listOf("a"), AlsoTo.targets(saved, picked, own = "b"))
    }

    @Test
    fun oneComputerAtTwoAddressesIsOneAndNeverItselfAgain() {
        // A is the own computer, saved at its LAN and its tailnet address;
        // B is saved twice too.
        val saved = listOf(s("a-lan", "A"), s("b-lan", "B"), s("a-tail", "A"), s("b-tail", "B"))
        assertEquals(listOf("b-lan"), AlsoTo.targets(saved, setOf("A", "B"), own = "a-lan"))
    }

    @Test
    fun theOwnAndThreeMoreArePicked() {
        assertTrue(AlsoTo.mayPickMore(3))
        assertFalse(AlsoTo.mayPickMore(4))
    }

    @Test
    fun aCopyLeavesOutWhatOffersAnAnswer() {
        // org.json is Android's, not in JVM tests: the names it drops are held here.
        assertEquals(listOf("actions", "requestReplyId", "messages", "canOpen"), AlsoTo.ANSWERS)
    }
}
