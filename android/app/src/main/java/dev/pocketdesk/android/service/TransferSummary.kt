package dev.pocketdesk.android.service

/**
 * What the transfer notification says once a batch is over (F7b): files
 * sent to the computer, files saved from it, or how many failed. Kept
 * apart from Android so it is tested.
 */
object TransferSummary {
    sealed interface Outcome {
        /** Every file went to the computer. */
        data class Sent(val count: Int) : Outcome

        /** Every file from the computer is in Downloads. */
        data class Saved(val count: Int) : Outcome

        /** Both, and nothing failed. */
        data class Mixed(val sent: Int, val saved: Int) : Outcome

        /** Only sends, and some did not go: the wording F7a shipped. */
        data class NotSent(val failed: Int, val count: Int) : Outcome

        /** Some transfer failed in a batch that received. */
        data class Failed(val failed: Int, val count: Int) : Outcome
    }

    /**
     * The line under the title: the last error when something failed, else
     * a [notice] the person should still see (what a folder left out).
     */
    fun detail(failed: Boolean, lastError: String?, notice: String?): String? =
        if (failed) lastError ?: notice else notice

    fun of(sent: Int, sends: Int, saved: Int, receives: Int): Outcome {
        val failed = (sends - sent) + (receives - saved)
        return when {
            failed > 0 && receives == 0 -> Outcome.NotSent(failed, sends)
            failed > 0 -> Outcome.Failed(failed, sends + receives)
            receives == 0 -> Outcome.Sent(sent)
            sends == 0 -> Outcome.Saved(saved)
            else -> Outcome.Mixed(sent, saved)
        }
    }
}
