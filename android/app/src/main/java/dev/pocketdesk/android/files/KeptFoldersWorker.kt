package dev.pocketdesk.android.files

import android.content.Context
import androidx.work.Worker
import androidx.work.WorkerParameters

/**
 * The job that says a run of the kept folders is due: WorkManager holds it
 * until its constraints do (an unmetered network, and charging by default),
 * and it then marks the run due and starts it where Android lets it. The
 * work itself is the transfer service's, which shows its own notice, so it
 * is not a second path to the computer: a phone that would not let this job
 * start the service has the notification listener's tick, or the app's
 * opening, start it.
 */
class KeptFoldersWorker(context: Context, params: WorkerParameters) : Worker(context, params) {
    override fun doWork(): Result {
        KeptFolders.markDue(applicationContext)
        KeptFolders.startIfDue(applicationContext)
        return Result.success()
    }
}
