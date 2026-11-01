package dev.pocketdesk.wear

import android.app.PendingIntent
import android.content.Intent
import androidx.wear.watchface.complications.data.ComplicationData
import androidx.wear.watchface.complications.data.ComplicationType
import androidx.wear.watchface.complications.data.PlainComplicationText
import androidx.wear.watchface.complications.data.ShortTextComplicationData
import androidx.wear.watchface.complications.datasource.ComplicationDataSourceService
import androidx.wear.watchface.complications.datasource.ComplicationRequest

/**
 * The complication: "PD", which opens the short list. It shows no live data
 * (a complication that did would ask the phone over Bluetooth on every
 * refresh, for battery and for nothing), so it is never refreshed.
 */
class PocketdeskComplication : ComplicationDataSourceService() {
    override fun onComplicationRequest(request: ComplicationRequest, listener: ComplicationRequestListener) {
        listener.onComplicationData(data(request.complicationType))
    }

    override fun getPreviewData(type: ComplicationType): ComplicationData? = data(type)

    private fun data(type: ComplicationType): ComplicationData? {
        if (type != ComplicationType.SHORT_TEXT) return null
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return ShortTextComplicationData.Builder(
            PlainComplicationText.Builder(getString(R.string.complication_text)).build(),
            PlainComplicationText.Builder(getString(R.string.app_name)).build(),
        )
            .setTapAction(open)
            .build()
    }
}
