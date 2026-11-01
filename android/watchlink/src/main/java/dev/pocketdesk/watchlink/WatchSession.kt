package dev.pocketdesk.watchlink

/**
 * What the phone does when a watch asks. The phone's own code, the same the
 * home-screen widgets and tiles call, fills this in; the session below knows
 * nothing of Android, so every rule on what a watch may ask is tested.
 * Each call blocks (the phone's calls to its computer do), so the server
 * runs them off the main thread. A failure is a [WatchFailure] with a few
 * plain words.
 */
interface WatchActions {
    fun status(): WatchBodies.Status
    fun wake(): Result<Unit>
    fun ring(): Result<Unit>
    fun music(command: MusicCommand): Result<Unit>
    fun deckList(): Result<List<WatchBodies.Deck>>
    fun deckPress(index: Int, label: String): Result<Unit>
    fun asks(): List<WatchBodies.Ask>

    /** Say No to the ask [id]; fails for one that is not waiting. */
    fun deny(id: String): Result<Unit>
}

/**
 * A failure whose [words] are meant for the watch's screen: a few plain
 * words. Any other exception reaches the watch as "did not work", never its
 * own text, which may name a path or an address.
 */
class WatchFailure(val words: String) : Exception(words)

/**
 * How soon each op may repeat, kept for the whole phone, so a watch that
 * reconnects between asks does not start over. A ring is at most once a
 * minute, as the computer's is (RingRule); the rest are short, against a
 * button held down or a loop.
 */
class WatchLimits(private val now: () -> Long) {
    private val last = HashMap<WatchOp, Long>()

    private fun gapMs(op: WatchOp): Long = when (op) {
        WatchOp.Ring -> 60_000L
        WatchOp.Wake -> 10_000L
        WatchOp.DeckPress -> 1_500L
        WatchOp.Music -> 300L
        WatchOp.AskDeny -> 300L
        WatchOp.Status, WatchOp.DeckList, WatchOp.AskList -> 0L
    }

    /** Whether [op] may go now; if so it is counted. */
    @Synchronized
    fun allow(op: WatchOp): Boolean {
        val t = now()
        val before = last[op]
        if (before != null && t - before < gapMs(op)) return false
        last[op] = t
        return true
    }
}

/**
 * One connection from a watch: its frames in, the phone's answers out.
 * Anything that does not verify is dropped without an answer, and after
 * [BAD_LIMIT] of them in a row the connection should end ([dropped]).
 */
class WatchSession(
    private val key: ByteArray,
    private val challenge: ByteArray,
    private val actions: WatchActions,
    private val on: () -> Set<WatchButton>,
    private val limits: WatchLimits,
) {
    /** The newest counter taken: a frame must beat it. */
    private var counter = 0L
    private var bad = 0

    val dropped: Boolean get() = bad >= BAD_LIMIT

    /** The sealed answer to [frame], or null when it is not a valid, fresh request. */
    fun serve(frame: ByteArray): ByteArray? {
        val request = WatchWire.openRequest(key, challenge, frame)
        if (request == null || request.counter <= counter) {
            bad++
            return null
        }
        bad = 0
        counter = request.counter
        fun answer(status: WatchStatus, body: ByteArray = ByteArray(0)) =
            WatchWire.sealResponse(key, challenge, request.counter, status, body)

        val button = request.op.button
        if (button != null && button !in on()) return answer(WatchStatus.Refused)
        if (!limits.allow(request.op)) return answer(WatchStatus.Busy)
        return try {
            run(request, ::answer)
        } catch (_: WatchIn.Malformed) {
            answer(WatchStatus.Failed, WatchBodies.why("bad request"))
        } catch (e: Exception) {
            // Never the exception's text: it may name a path or an address.
            answer(WatchStatus.Failed, WatchBodies.why("did not work"))
        }
    }

    private fun run(request: WatchWire.Request, answer: (WatchStatus, ByteArray) -> ByteArray): ByteArray {
        fun done(result: Result<Unit>): ByteArray = result.fold(
            { answer(WatchStatus.Ok, ByteArray(0)) },
            { answer(WatchStatus.Failed, WatchBodies.why(wordsOf(it))) },
        )
        return when (request.op) {
            WatchOp.Status -> {
                // The buttons said are those that are on now: a stale tile never offers a switched-off one.
                val s = actions.status()
                answer(WatchStatus.Ok, WatchBodies.status(WatchBodies.Status(s.computer, on(), s.asks)))
            }
            WatchOp.Wake -> done(actions.wake())
            WatchOp.Ring -> done(actions.ring())
            WatchOp.Music -> {
                val command = WatchBodies.readMusic(request.body) ?: throw WatchIn.Malformed()
                done(actions.music(command))
            }
            WatchOp.DeckList -> actions.deckList().fold(
                { answer(WatchStatus.Ok, WatchBodies.deck(it)) },
                { answer(WatchStatus.Failed, WatchBodies.why(wordsOf(it))) },
            )
            WatchOp.DeckPress -> {
                val d = WatchBodies.readDeckPress(request.body) ?: throw WatchIn.Malformed()
                done(actions.deckPress(d.index, d.label))
            }
            WatchOp.AskList -> answer(WatchStatus.Ok, WatchBodies.asks(actions.asks()))
            WatchOp.AskDeny -> {
                val id = WatchBodies.readAskDeny(request.body) ?: throw WatchIn.Malformed()
                done(actions.deny(id))
            }
        }
    }

    private fun wordsOf(e: Throwable): String = (e as? WatchFailure)?.words ?: "did not work"

    companion object {
        const val BAD_LIMIT = 3
    }
}
