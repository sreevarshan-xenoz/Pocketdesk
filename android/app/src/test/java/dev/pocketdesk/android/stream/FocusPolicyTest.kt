package dev.pocketdesk.android.stream

import android.media.AudioManager
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class FocusPolicyTest {
    @Test
    fun focusIsNotTakenUntilTheDesktopMakesASound() {
        val focus = FocusPolicy()
        assertFalse("a silent desktop must not pause the phone's music", focus.audible())
        assertTrue(focus.wantsRequest())
        focus.onRequestResult(AudioManager.AUDIOFOCUS_REQUEST_GRANTED)
        assertTrue(focus.audible())
        assertFalse(focus.wantsRequest())
        assertEquals(1f, focus.volume())
    }

    @Test
    fun aDelayedGrantWaitsForTheGain() {
        val focus = FocusPolicy()
        focus.onRequestResult(AudioManager.AUDIOFOCUS_REQUEST_DELAYED) // e.g. during a call
        assertFalse(focus.audible())
        assertFalse("asked once; the gain comes by callback", focus.wantsRequest())
        focus.onChange(AudioManager.AUDIOFOCUS_GAIN)
        assertTrue(focus.audible())
    }

    @Test
    fun aTransientLossResumesAndADuckLowersTheVolume() {
        val focus = FocusPolicy()
        focus.onRequestResult(AudioManager.AUDIOFOCUS_REQUEST_GRANTED)
        focus.onChange(AudioManager.AUDIOFOCUS_LOSS_TRANSIENT)
        assertFalse(focus.audible())
        focus.onChange(AudioManager.AUDIOFOCUS_GAIN)
        assertTrue(focus.audible())
        focus.onChange(AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK)
        assertTrue(focus.audible())
        assertEquals(0.3f, focus.volume())
    }

    @Test
    fun aPermanentLossIsNotTakenBack() {
        val focus = FocusPolicy()
        focus.onRequestResult(AudioManager.AUDIOFOCUS_REQUEST_GRANTED)
        focus.onChange(AudioManager.AUDIOFOCUS_LOSS) // the user started another app's music
        assertFalse(focus.audible())
        assertFalse("the user chose the other app", focus.wantsRequest())
    }

    @Test
    fun aFailedRequestIsNotRetriedEveryFrame() {
        val focus = FocusPolicy()
        focus.onRequestResult(AudioManager.AUDIOFOCUS_REQUEST_FAILED)
        assertFalse(focus.audible())
        assertFalse(focus.wantsRequest())
    }
}
