package dev.pocketdesk.fixture

import android.app.Activity
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Person
import android.app.RemoteInput
import android.content.BroadcastReceiver
import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.os.PersistableBundle
import android.util.Log
import android.view.ViewGroup
import android.widget.EditText

/**
 * What scripts/device_checks.py drives on the phone (N1). Nothing here is
 * part of Pocketdesk: it stands in for the apps a person would use.
 */
private const val TAG = "Fixture"

/**
 * A text field that fills the screen, focused, for typing checks.
 * `am start -n dev.pocketdesk.fixture/.FieldActivity [--es text …]`.
 */
class FieldActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val field = EditText(this).apply {
            id = R_ID_FIELD
            contentDescription = "fixture-field"
            setText(intent.getStringExtra("text").orEmpty())
            layoutParams = ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT)
        }
        setContentView(field)
        field.requestFocus()
        field.setSelection(field.text.length)
        // Its notifications need the owner's yes once (the shell may not
        // grant it on every phone): asked here, the first time it opens.
        if (android.os.Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) != android.content.pm.PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissions(arrayOf(android.Manifest.permission.POST_NOTIFICATIONS), 1)
        }
    }

    private companion object {
        const val R_ID_FIELD = 0x7f0a0001
    }
}

/**
 * Puts `text` on the clipboard once it has focus (Android lets only the app
 * in front write it), marked sensitive with `--ez sensitive true`, then
 * closes. `am start -n dev.pocketdesk.fixture/.CopyActivity --es text …`.
 */
class CopyActivity : Activity() {
    private var done = false

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (!hasFocus || done) return
        done = true
        val text = intent.getStringExtra("text").orEmpty()
        val clip = ClipData.newPlainText("fixture", text)
        if (intent.getBooleanExtra("sensitive", false)) {
            clip.description.extras = PersistableBundle().apply {
                // ClipDescription.EXTRA_IS_SENSITIVE, by value: the constant is API 33.
                putBoolean("android.content.extra.IS_SENSITIVE", true)
            }
        }
        getSystemService(ClipboardManager::class.java).setPrimaryClip(clip)
        Log.i(TAG, "copied ${text.length} characters")
        finish()
    }
}

/**
 * Posts or cancels a notification on an adb broadcast:
 * `am broadcast -n dev.pocketdesk.fixture/.NotifyReceiver -a dev.pocketdesk.fixture.NOTIFY
 *  --ei id N --es title … --es text … [--es style chat|progress] [--ei progress P]`,
 * and `-a dev.pocketdesk.fixture.CANCEL --ei id N`.
 */
class NotifyReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val manager = context.getSystemService(NotificationManager::class.java)
        val id = intent.getIntExtra("id", 1)
        if (intent.action == "dev.pocketdesk.fixture.CANCEL") {
            manager.cancel(id)
            return
        }
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, "Fixture", NotificationManager.IMPORTANCE_DEFAULT),
        )
        val title = intent.getStringExtra("title") ?: "Fixture"
        val text = intent.getStringExtra("text") ?: "A notification"
        // A tap opens the field with "opened <id>" in it (N3's check).
        val tap = PendingIntent.getActivity(
            context,
            id,
            Intent(context, FieldActivity::class.java)
                .putExtra("text", "opened $id")
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val builder = Notification.Builder(context, CHANNEL)
            .setSmallIcon(android.R.drawable.ic_dialog_info)
            .setContentTitle(title)
            .setContentText(text)
            .setContentIntent(tap)
        when (intent.getStringExtra("style")) {
            "chat" -> {
                val me = Person.Builder().setName("Me").build()
                val them = Person.Builder().setName(title).build()
                builder.setStyle(
                    Notification.MessagingStyle(me).addMessage(text, System.currentTimeMillis(), them),
                )
                val reply = RemoteInput.Builder(KEY_REPLY).setLabel("Reply").build()
                builder.addAction(
                    Notification.Action.Builder(null, "Reply", answer(context, id, "reply"))
                        .addRemoteInput(reply)
                        .build(),
                )
                builder.addAction(Notification.Action.Builder(null, "Mark as read", answer(context, id, "read")).build())
            }
            "progress" -> builder.setOngoing(true).setProgress(100, intent.getIntExtra("progress", 0), false)
        }
        manager.notify(id, builder.build())
        Log.i(TAG, "posted $id")
    }

    private fun answer(context: Context, id: Int, what: String): PendingIntent =
        PendingIntent.getBroadcast(
            context,
            id * 10 + what.length,
            Intent(context, AnswerReceiver::class.java).putExtra("id", id).putExtra("what", what),
            PendingIntent.FLAG_MUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )

    companion object {
        const val CHANNEL = "fixture"
        const val KEY_REPLY = "reply"
    }
}

/** A notification's answer, written to logcat as `FixtureReply`. */
class AnswerReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val reply = RemoteInput.getResultsFromIntent(intent)?.getCharSequence(NotifyReceiver.KEY_REPLY)
        Log.i(
            "FixtureReply",
            "id=${intent.getIntExtra("id", 0)} what=${intent.getStringExtra("what")} reply=${reply ?: ""}",
        )
    }
}
