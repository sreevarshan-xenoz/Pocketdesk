package dev.pocketdesk.android.files

import android.net.Uri
import java.util.concurrent.ConcurrentHashMap

/**
 * Where the camera's and the screenshots' marks go once what they handed to
 * the transfer service has gone (BB4): they were moved as it was handed
 * over, so one that failed (the computer went away mid-file, or unpaired
 * the phone) was never sent again.
 */
object UploadMarks {
    /**
     * The mark after [handed], media ids oldest first: past the leading run
     * that [arrived], or null when the first did not. One not yet known
     * (still going, or its service gone with the process) stops the run, so
     * it goes again rather than being passed unsent.
     */
    fun after(handed: List<Long>, arrived: (Long) -> Boolean?): Long? {
        var mark: Long? = null
        for (id in handed) {
            if (arrived(id) != true) break
            mark = id
        }
        return mark
    }
}

/**
 * How each file handed to the transfer service last went, for the uploaders
 * to settle their marks by (BB4). In memory: a process that dies mid-send
 * forgets, and its files go again.
 */
object SendOutcomes {
    private val outcomes = ConcurrentHashMap<String, Boolean>()

    fun record(uri: Uri, sent: Boolean) {
        outcomes[uri.toString()] = sent
    }

    /** How [uri] went, forgotten once read; null when not known. */
    fun take(uri: Uri): Boolean? = outcomes.remove(uri.toString())
}
