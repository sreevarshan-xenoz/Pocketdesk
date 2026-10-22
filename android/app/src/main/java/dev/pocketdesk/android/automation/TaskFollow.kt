package dev.pocketdesk.android.automation

import android.content.Context
import android.os.SystemClock
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.ui.TaskRun
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.json.JSONObject

/**
 * A run of one of the computer's tasks followed to its end (ecosystem 1c):
 * by the Tasks sheet, by its followers once the sheet closes, and by an
 * automation app's start (O1). Its end is told to other apps (1f).
 */
object TaskFollow {
    /** Runs followed outside a screen, to their end. */
    private val followers = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    /**
     * Run [id]'s status from the computer's latest result, and that result;
     * [asks] is whether the task asks its owner first, [sinceMs] when the
     * start went (elapsed realtime). Blocking: call it off the main thread.
     */
    fun latest(id: String, asks: Boolean, sinceMs: Long): Pair<TaskRun.Status, JSONObject?> {
        val json = RustCore.taskResult(id)
        val body = json.takeIf { it.isNotBlank() }?.let { runCatching { JSONObject(it) }.getOrNull() }
        val answered =
            TaskRun.status(
                state = body?.optString("state")?.takeIf { it.isNotEmpty() },
                code = body?.takeIf { it.has("code") && !it.isNull("code") }?.optInt("code"),
                timedOut = body?.optBoolean("timed_out") == true,
                reason = body?.optString("reason")?.takeIf { it.isNotEmpty() },
                asks = asks,
            )
        return TaskRun.givenUp(answered, SystemClock.elapsedRealtime() - sinceMs) to body
    }

    /** Other apps hear that [task] on the computer at [address] ended so (1f). */
    fun tellFinished(context: Context, address: String, task: String, status: TaskRun.Status) {
        PhoneEventBroadcast.send(context, PhoneEvents.Kind.TASK_FINISHED, address, TaskRun.told(status) + ("task" to task))
    }

    /** Follow run [id] of [task] from outside any screen, then tell other apps how it ended. */
    fun toTheEnd(context: Context, address: String, task: String, id: String, asks: Boolean, sinceMs: Long) {
        val app = context.applicationContext
        followers.launch {
            while (true) {
                val (status, _) = latest(id, asks, sinceMs)
                if (TaskRun.finished(status)) {
                    tellFinished(app, address, task, status)
                    break
                }
                delay(1_000)
            }
        }
    }
}
