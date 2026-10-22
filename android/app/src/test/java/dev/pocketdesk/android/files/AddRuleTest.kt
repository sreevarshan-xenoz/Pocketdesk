package dev.pocketdesk.android.files

import dev.pocketdesk.android.files.AddRule.Decision
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class AddRuleTest {
    @Test
    fun onlyTheOwnComputerAddsAndOnlyToAMarkedFolder() {
        assertEquals(Decision.ADD, AddRule.decide(ownComputer = true, marked = true, path = listOf("Docs"), name = "a.txt"))
        assertEquals(Decision.NOT_OWN_COMPUTER, AddRule.decide(false, true, emptyList(), "a.txt"))
        assertEquals(Decision.NOT_MARKED, AddRule.decide(true, false, emptyList(), "a.txt"))
    }

    @Test
    fun onlyNamesAListingShowsAreAdded() {
        assertEquals(Decision.BAD_NAME, AddRule.decide(true, true, emptyList(), ".hidden"))
        assertEquals(Decision.BAD_NAME, AddRule.decide(true, true, listOf(".git"), "a"))
        assertEquals(Decision.BAD_NAME, AddRule.decide(true, true, listOf(".."), "a"))
        assertEquals(Decision.BAD_NAME, AddRule.decide(true, true, emptyList(), "a/b"))
        // A folder is named by the end of its path; the root itself is never made.
        assertEquals(Decision.ADD, AddRule.decide(true, true, listOf("New"), null))
        assertEquals(Decision.BAD_NAME, AddRule.decide(true, true, emptyList(), null))
        assertEquals(Decision.BAD_NAME, AddRule.decide(true, true, List(PhoneFilesRule.MAX_DEPTH + 1) { "a" }, "b"))
    }

    @Test
    fun theComputerHearsWhyButNeverAnotherComputer() {
        assertEquals(AddRule.NOT_MARKED, AddRule.reason(Decision.NOT_MARKED))
        assertEquals(AddRule.BAD_NAME, AddRule.reason(Decision.BAD_NAME))
        assertNull(AddRule.reason(Decision.NOT_OWN_COMPUTER))
        assertNull(AddRule.reason(Decision.ADD))
    }
}
