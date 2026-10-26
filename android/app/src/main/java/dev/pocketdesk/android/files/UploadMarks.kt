package dev.pocketdesk.android.files

import android.net.Uri
import java.util.concurrent.ConcurrentHashMap

/** How a file an uploader handed to the transfer service went (BB4). */
enum class SendOutcome { PENDING, SENT, FAILED, CANCELLED }

/**
 * The camera's and the screenshots' files handed to the transfer service,
 * and how each went (BB4): a new photo was handed over and taken for sent,
 * so one that failed (the computer went away mid-file, or unpaired the
 * phone) was never sent again. Only a file an uploader expects is kept,
 * until it reads it; in memory, so a process that dies forgets.
 */
object SendOutcomes {
    private val outcomes = ConcurrentHashMap<String, SendOutcome>()

    /** An uploader hands [uri] over: its outcome is kept once known. */
    fun expect(uri: Uri) {
        outcomes[uri.toString()] = SendOutcome.PENDING
    }

    /** How [uri] went, kept only when an uploader expects it. */
    fun record(uri: Uri, outcome: SendOutcome) {
        outcomes.computeIfPresent(uri.toString()) { _, _ -> outcome }
    }

    /** How [uri] went; null when it is not expected. */
    fun peek(uri: Uri): SendOutcome? = outcomes[uri.toString()]

    fun forget(uri: Uri) {
        outcomes.remove(uri.toString())
    }
}

/** Which of the files an uploader handed over go again, and how often (BB4). */
object UploadRetry {
    /** Sends of one file at most, the first included: one that keeps failing is given up. */
    const val MAX_SENDS = 3

    /**
     * How long a file still waiting for its outcome is given while the
     * transfer service is idle: it may not have started yet; past it, the
     * service never ran the file, and it counts as failed.
     */
    const val START_GRACE_MS = 60_000L

    /**
     * Those of [handed] (each with how many times it has been sent) that go
     * again: failed, or never run, and sent fewer than [MAX_SENDS] times.
     * One cancelled, or sent, does not. Null while one is still waiting
     * within [START_GRACE_MS] of [handedAgoMs].
     */
    fun <T> again(handed: List<Pair<T, Int>>, handedAgoMs: Long, outcome: (T) -> SendOutcome?): List<Pair<T, Int>>? {
        val outcomes = handed.map { (item, _) -> outcome(item) }
        if (outcomes.any { it == SendOutcome.PENDING } && handedAgoMs < START_GRACE_MS) return null
        return handed.zip(outcomes)
            .filter { (sent, how) -> how != SendOutcome.SENT && how != SendOutcome.CANCELLED && sent.second < MAX_SENDS }
            .map { (sent, _) -> sent }
    }

    /** How many of [handed] failed for the last time, to be given up (review: not those sent at last). */
    fun <T> givenUp(handed: List<Pair<T, Int>>, outcome: (T) -> SendOutcome?): Int =
        handed.count { (item, sends) ->
            val how = outcome(item)
            sends >= MAX_SENDS && how != SendOutcome.SENT && how != SendOutcome.CANCELLED
        }
}
