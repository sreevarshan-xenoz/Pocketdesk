package dev.pocketdesk.android.ui

import android.graphics.Bitmap
import android.graphics.Color
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import dev.pocketdesk.android.pair.firstPairingCode
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import zxingcpp.BarcodeReader

/**
 * The scanner's library reads the code the desktop app draws (T3): the
 * fixture is that app's QR for the shared pairing link, held to it by a
 * desktop test, decoded here on a real device as the camera would see it,
 * dark on light and inverted (a dark-themed screen).
 */
@RunWith(AndroidJUnit4::class)
class QrDecodeTest {
    private val link =
        "pocketdesk://pair?v=1&name=Desk%20%26%20Co&id=d-1&a=192.168.1.5,100.80.1.2&cp=1716&sp=4716" +
            "&pin=012345&fp=abababababababababababababababababababababababababababababababab" +
            "&ep=eyJlbmRwb2ludElkIjoieCIsInJlbGF5VXJscyI6W10sImRpcmVjdEFkZHJzIjpbXX0"

    private val reader = BarcodeReader(
        BarcodeReader.Options(
            formats = setOf(BarcodeReader.Format.QR_CODE),
            tryHarder = true,
            tryInvert = true,
            tryDownscale = true,
        ),
    )

    private fun modules(): List<String> =
        InstrumentationRegistry.getInstrumentation().context.assets
            .open("pair_fixture_qr.txt").bufferedReader().readLines().filter { it.isNotBlank() }

    private fun draw(rows: List<String>, inverted: Boolean): Bitmap {
        val scale = 8
        val quiet = 4
        val side = (rows.size + 2 * quiet) * scale
        val light = if (inverted) Color.BLACK else Color.WHITE
        val dark = if (inverted) Color.WHITE else Color.BLACK
        val bitmap = Bitmap.createBitmap(side, side, Bitmap.Config.ARGB_8888)
        bitmap.eraseColor(light)
        rows.forEachIndexed { y, row ->
            row.forEachIndexed { x, c ->
                if (c == '1') {
                    for (dy in 0 until scale) for (dx in 0 until scale) {
                        bitmap.setPixel((x + quiet) * scale + dx, (y + quiet) * scale + dy, dark)
                    }
                }
            }
        }
        return bitmap
    }

    @Test
    fun the_desktop_apps_code_is_read() {
        val texts = reader.read(draw(modules(), inverted = false)).map { it.text }
        assertEquals(link, firstPairingCode(texts))
    }

    @Test
    fun an_inverted_code_is_read_too() {
        val texts = reader.read(draw(modules(), inverted = true)).map { it.text }
        assertEquals(link, firstPairingCode(texts))
    }
}
