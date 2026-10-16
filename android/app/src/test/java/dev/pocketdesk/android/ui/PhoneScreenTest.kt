package dev.pocketdesk.android.ui

import dev.pocketdesk.android.ui.PhoneScreen.Answer
import org.junit.Assert.assertEquals
import org.junit.Test

class PhoneScreenTest {
    @Test
    fun `an added screen gives its index`() {
        assertEquals(Answer.Added(1), PhoneScreen.parse("""{"name":"HEADLESS-2","index":1,"width":1080,"height":2400}"""))
    }

    @Test
    fun `a refusal gives its reason`() {
        assertEquals(
            Answer.Refused("This computer cannot add a screen: only Sway can for now"),
            PhoneScreen.parse("""{"error":"This computer cannot add a screen: only Sway can for now"}"""),
        )
        assertEquals(Answer.Refused(""), PhoneScreen.parse("{}"))
        assertEquals(
            Answer.Refused("Settings, \"Phones may add a screen\""),
            PhoneScreen.parse("""{"error":"Settings, \"Phones may add a screen\""}"""),
        )
    }
}
