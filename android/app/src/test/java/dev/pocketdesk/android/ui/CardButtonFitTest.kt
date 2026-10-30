package dev.pocketdesk.android.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class CardButtonFitTest {
    @Test
    fun `english fits with roomy sides`() {
        assertEquals(12, CardButtonFit.sideDp(listOf(52f, 49f, 81f), rowWidthDp = 288f))
    }

    @Test
    fun `spanish fits only with tight sides`() {
        // As measured on the OPPO: Conectar 58, Mando 45, Mostrar pantalla 106.
        assertEquals(8, CardButtonFit.sideDp(listOf(58f, 45f, 106f), rowWidthDp = 288f))
    }

    @Test
    fun `a row that cannot fit keeps roomy sides and wraps`() {
        assertEquals(12, CardButtonFit.sideDp(listOf(40f, 70f, 160f), rowWidthDp = 288f))
    }

    @Test
    fun `a short label still counts as material's narrowest button`() {
        // 30 + 24 = 54 is under 58: three such buttons and two gaps are 190.
        assertEquals(12, CardButtonFit.sideDp(listOf(30f, 30f, 30f), rowWidthDp = 190f))
        // Tight sides cannot make them narrower than 58 either: it wraps.
        assertEquals(12, CardButtonFit.sideDp(listOf(30f, 30f, 30f), rowWidthDp = 189f))
    }
}
