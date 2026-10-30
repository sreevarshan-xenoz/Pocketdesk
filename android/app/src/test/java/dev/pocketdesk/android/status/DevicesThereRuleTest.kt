package dev.pocketdesk.android.status

import dev.pocketdesk.android.R
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class DevicesThereRuleTest {
    private fun device(
        id: String,
        name: String = "",
        computer: Boolean = false,
        owner: Boolean = false,
        guest: Boolean = false,
        connected: Boolean = false,
        you: Boolean = false,
    ) = DevicesThereRule.Device(id, name, computer, owner, guest, connected, you)

    @Test
    fun thisPhoneIsListedButNeverOfferedForRemoval() {
        assertFalse(DevicesThereRule.removable(device("me", you = true)))
        assertTrue(DevicesThereRule.removable(device("lost", owner = true)))
        assertTrue(DevicesThereRule.removable(device("pc", computer = true)))
    }

    @Test
    fun aDeviceWithoutANameShowsTheStartOfItsId() {
        assertEquals("Pixel", DevicesThereRule.shownName(device("abc", "  Pixel ")))
        assertEquals("01234567", DevicesThereRule.shownName(device("0123456789", " ")))
    }

    @Test
    fun thisPhoneComesFirstThenTheOwnersThenTheRest() {
        val sorted = DevicesThereRule.sorted(
            listOf(
                device("g", "Guest tablet", guest = true),
                device("b", "b laptop", computer = true, owner = true),
                device("me", "Mine", you = true, owner = true),
                device("a", "A phone", owner = true),
            ),
        )
        assertEquals(listOf("me", "a", "b", "g"), sorted.map { it.id })
    }

    @Test
    fun eachDeviceSaysWhatItIs() {
        assertEquals(
            listOf(R.string.there_kind_phone, R.string.there_kind_you, R.string.there_kind_connected),
            DevicesThereRule.kinds(device("me", you = true, owner = true, connected = true)),
        )
        assertEquals(
            listOf(R.string.there_kind_computer, R.string.there_kind_yours),
            DevicesThereRule.kinds(device("pc", computer = true, owner = true)),
        )
        assertEquals(
            listOf(R.string.there_kind_phone, R.string.there_kind_guest),
            DevicesThereRule.kinds(device("g", guest = true)),
        )
    }

    @Test
    fun aRemovalSaysWhetherItWentAndWhyNot() {
        assertEquals(DevicesThereRule.Outcome(true, null), DevicesThereRule.outcome(true, null))
        assertEquals(
            DevicesThereRule.Outcome(false, R.string.there_refused_last_owner),
            DevicesThereRule.outcome(false, "last_owner"),
        )
        assertEquals(R.string.there_refused_not_owner, DevicesThereRule.refusal("not_owner"))
        assertEquals(R.string.there_refused_too_many, DevicesThereRule.refusal("too_many"))
        assertEquals(R.string.there_refused_yourself, DevicesThereRule.refusal("yourself"))
        assertEquals(R.string.there_refused_not_paired, DevicesThereRule.refusal("not_paired"))
        // A code from a newer computer, or none, reads as a failure.
        assertEquals(R.string.there_refused_failed, DevicesThereRule.refusal("teleported"))
        assertEquals(R.string.there_refused_failed, DevicesThereRule.refusal(null))
    }

    /** PR #574 review I2: only the answer naming this ask's id is taken. */
    @Test
    fun anAnswerIsTakenOnlyForTheAskItNames() {
        val first = DevicesThereRule.newRequest()
        val second = DevicesThereRule.newRequest()
        assertTrue(first != second)
        assertTrue(DevicesThereRule.isAnswerTo(second, second))
        assertFalse("a late answer to the first ask", DevicesThereRule.isAnswerTo(second, first))
        assertFalse(DevicesThereRule.isAnswerTo(second, null))
        assertFalse(DevicesThereRule.isAnswerTo("", ""))
    }

    /** Review N8: a device already gone there leaves the list too. */
    @Test
    fun aDeviceGoneThereLeavesTheList() {
        assertTrue(DevicesThereRule.dropsRow(true, null))
        assertTrue(DevicesThereRule.dropsRow(false, "not_paired"))
        assertFalse(DevicesThereRule.dropsRow(false, "too_many"))
        assertEquals(R.string.there_refused_off, DevicesThereRule.refusal("off"))
        assertEquals(R.string.there_refused_busy, DevicesThereRule.refusal("busy"))
    }

    @Test
    fun aRemovedDeviceLeavesTheList() {
        val list = listOf(device("a"), device("b"))
        assertEquals(listOf("b"), DevicesThereRule.without(list, "a").map { it.id })
        assertEquals(list, DevicesThereRule.without(list, "zzz"))
    }
}
