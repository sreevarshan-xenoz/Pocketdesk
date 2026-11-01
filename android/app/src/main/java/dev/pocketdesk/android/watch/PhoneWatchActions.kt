package dev.pocketdesk.android.watch

import android.content.Context
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.ask.Asks
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.ring.PhoneRinger
import dev.pocketdesk.android.ui.Deck
import dev.pocketdesk.android.wake.WakePlan
import dev.pocketdesk.android.wake.Waker
import dev.pocketdesk.watchlink.MusicCommand
import dev.pocketdesk.watchlink.WatchActions
import dev.pocketdesk.watchlink.WatchBodies
import dev.pocketdesk.watchlink.WatchButton
import dev.pocketdesk.watchlink.WatchFailure
import org.json.JSONObject

/**
 * What a watch's buttons do on this phone: the very calls the home-screen
 * widgets and tiles make for the owner (the wake tile's, the music widget's,
 * the deck widget's, a tap on No), so the computer sees this phone and
 * nothing else, and each is held to the permission the computer already gave
 * the phone. Nothing here reaches the computer another way, and a watch has
 * no way to say Yes to a question: that stays with the phone's own
 * fingerprint. Blocking, off the main thread (the link's own thread).
 *
 * Failures are fixed plain words: a watch's screen never shows an address,
 * a path or the computer's own text.
 */
class PhoneWatchActions(private val app: Context) : WatchActions {
    private fun host(): HostStore.Host? = HostStore.lastHost(app)

    override fun status(): WatchBodies.Status {
        val h = host()
        return WatchBodies.Status(
            computer = h?.let { HostStore.shownName(app, it.address) }.orEmpty(),
            // The session says the buttons that are on, not this.
            buttons = emptySet<WatchButton>(),
            asks = Asks.waiting().size,
        )
    }

    override fun wake(): Result<Unit> {
        val h = host() ?: return noComputer()
        return runCatching {
            // The bridge is started first: a request may be the first thing to wake this process (as WidgetAsk does).
            RustCore.start(app)
            when (Waker.planFor(app, h.address)) {
                WakePlan.Plan.Off -> return failure("wake is off for it")
                WakePlan.Plan.NoMac -> return failure("its address is not known yet")
                WakePlan.Plan.NoRelay -> return failure("no computer is chosen to send it")
                WakePlan.Plan.Away -> return failure("away from its network")
                else -> Unit
            }
            Waker.wake(app, h.address).getOrThrow()
        }.fold({ Result.success(Unit) }, { if (it is WatchFailure) Result.failure(it) else failure("not sent") })
    }

    override fun ring(): Result<Unit> = runCatching {
        // The phone itself, with the same alarm and Stop as a ring from the computer.
        PhoneRinger.ring(app, fromWatch = true)
    }.fold({ Result.success(Unit) }, { failure("could not ring") })

    override fun music(command: MusicCommand): Result<Unit> {
        val h = host() ?: return noComputer()
        return runCatching {
            RustCore.start(app)
            val body = JSONObject().put("action", "mediaControl").put("command", command.command)
            HostStore.player(app, h.address)?.let { body.put("player", it) }
            val answer = RustCore.audioControl(h.address, h.controlPort, body.toString()).getOrThrow()
            // The computer's own refusal (audio off for this phone) is a no, not a failure to reach it.
            if (!JSONObject(answer).optBoolean("ok", true)) return failure("the computer refused")
        }.fold({ Result.success(Unit) }, { if (it is WatchFailure) Result.failure(it) else failure("not reached") })
    }

    override fun deckList(): Result<List<WatchBodies.Deck>> {
        val h = host() ?: return noComputer()
        return runCatching {
            RustCore.start(app)
            val json = RustCore.listDeck(h.address, h.controlPort).getOrThrow()
            Deck.parse(json).map { WatchBodies.Deck(it.index, it.label) }
        }.fold({ Result.success(it) }, { failure("not reached") })
    }

    override fun deckPress(index: Int, label: String): Result<Unit> {
        val h = host() ?: return noComputer()
        return runCatching {
            RustCore.start(app)
            val outcome = Deck.outcome(RustCore.pressDeck(h.address, h.controlPort, index, label).getOrThrow())
            when {
                // A task that asks the computer's owner first.
                outcome.waiting -> return failure("waiting for the computer")
                !outcome.ok -> return failure("the computer refused")
            }
        }.fold({ Result.success(Unit) }, { if (it is WatchFailure) Result.failure(it) else failure("not reached") })
    }

    override fun asks(): List<WatchBodies.Ask> =
        Asks.waiting().map { WatchBodies.Ask(it.id, it.approval, it.text) }

    override fun deny(id: String): Result<Unit> =
        Asks.deny(app, id).fold({ Result.success(Unit) }, { failure("not waiting") })

    private fun noComputer(): Result<Nothing> = failure("no computer saved")

    private fun failure(words: String): Result<Nothing> = Result.failure(WatchFailure(words))
}
