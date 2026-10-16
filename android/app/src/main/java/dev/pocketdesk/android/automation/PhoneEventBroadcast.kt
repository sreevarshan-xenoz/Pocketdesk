package dev.pocketdesk.android.automation

import android.content.Context
import android.content.Intent
import android.util.Log
import dev.pocketdesk.android.Prefs

/** Sends [PhoneEvents] as broadcasts, while Settings says to (off by default). */
object PhoneEventBroadcast {
    private const val TAG = "PhoneEvents"

    fun send(
        context: Context,
        kind: PhoneEvents.Kind,
        address: String,
        fields: Map<String, Any?> = emptyMap(),
    ) {
        if (!Prefs.tellOtherApps(context)) return
        val event = PhoneEvents.build(kind, address, fields) ?: return
        val intent = Intent(kind.action)
        for ((key, value) in event.extras) {
            when (value) {
                is String -> intent.putExtra(key, value)
                is Int -> intent.putExtra(key, value)
                is Long -> intent.putExtra(key, value)
                is Boolean -> intent.putExtra(key, value)
            }
        }
        // No package: automation apps receive it with a receiver registered
        // at run time, which takes implicit broadcasts on every Android.
        runCatching { context.applicationContext.sendBroadcast(intent) }
            .onFailure { Log.w(TAG, "Could not tell other apps: ${it.message}") }
        Log.d(TAG, "Told other apps: ${kind.event}")
    }
}
