package dev.pocketdesk.android.shortcuts

import android.content.BroadcastReceiver
import android.content.Context
import android.util.Log
import dev.pocketdesk.android.bridge.RustCore
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread

/**
 * What the home-screen widgets that ask the computer share (G3, G5): the
 * bridge started before a request, and a request run under a broadcast's
 * goAsync with a bounded wait.
 */
object WidgetAsk {
    private const val TAG = "WidgetAsk"

    /** How long a broadcast waits for an answer: under its 10 s. */
    const val ASK_SECONDS = 8L

    /**
     * The bridge's [request], with the bridge started first: a widget's tap
     * may start the process, which then has no identity or pins until it is
     * (G3 review). An Error from the library is a failure.
     */
    fun ask(app: Context, request: () -> Result<String>): Result<String> = try {
        RustCore.start(app)
        request()
    } catch (e: Throwable) {
        Result.failure(e)
    }

    /**
     * Run [work] on a thread under [receiver]'s goAsync, waiting at most
     * [ASK_SECONDS], then [onTimeout] if it still runs; an Error is caught,
     * never the app's crash. [work] draws what it heard itself, so a late
     * answer is drawn too.
     */
    fun bounded(
        receiver: BroadcastReceiver,
        name: String,
        onEnd: () -> Unit = {},
        onTimeout: () -> Unit,
        work: () -> Unit,
    ) {
        val pending = receiver.goAsync()
        thread(name = name) {
            try {
                val worker = thread(name = "$name-ask") {
                    try {
                        work()
                    } catch (e: Throwable) {
                        Log.w(TAG, "$name failed", e)
                    } finally {
                        onEnd()
                    }
                }
                worker.join(TimeUnit.SECONDS.toMillis(ASK_SECONDS))
                // Said, not kept as "Asking…" for good (G3 review).
                if (worker.isAlive) onTimeout()
            } catch (e: Exception) {
                Log.w(TAG, "$name failed", e)
            } finally {
                pending.finish()
            }
        }
    }
}
