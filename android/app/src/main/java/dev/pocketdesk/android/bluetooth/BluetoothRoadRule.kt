package dev.pocketdesk.android.bluetooth

import org.json.JSONObject
import java.util.UUID

/**
 * When the phone dials its computer by Bluetooth (the fallback road,
 * protocol.md 1.4a), as pure rules: [BluetoothRoad] reads the phone and
 * asks these.
 */
object BluetoothRoadRule {
    /** The road the bridge says a control link came by over Bluetooth (`nativeControlRoad`). */
    const val ROAD_BLUETOOTH = 2

    /** The largest file that crosses by Bluetooth, 8 MiB; a bigger one waits for Wi-Fi (the computer's `TRANSFER_MAX`). */
    const val TRANSFER_MAX = 8_388_608L

    /** The rest after the first failed dial; it doubles with each one after. */
    const val FIRST_REST_MS = 30_000L

    /** The longest rest between dials: a computer away or off is not dialed every few seconds all day. */
    const val MAX_REST_MS = 480_000L

    /**
     * How long a link request is answered for: the last link's channel may
     * still be closing. With the poll before it, well within the 10 s the
     * computer waits for the link.
     */
    const val LINK_TRY_MS = 6_000L

    /** The wait between tries to open a transfer link. */
    const val LINK_STEP_MS = 500L

    /** What a computer said of its Bluetooth road: its radio's [address], the control [service] and the transfer service ([link]). */
    data class Info(val address: String, val service: UUID, val link: UUID)

    /**
     * Whether to dial the computer by Bluetooth now: the switch is on
     * ([enabled]), the phone may use Bluetooth ([permitted]) and its radio
     * is on, the computer told the phone its radio's address
     * ([knownAddress]) and that device is bonded (nothing else bonded, such
     * as earbuds or a car, is ever dialed), the other roads failed or there
     * is no usable network to try them on, and the rest after the last
     * failed dial is over.
     */
    fun shouldDial(
        enabled: Boolean,
        permitted: Boolean,
        radioOn: Boolean,
        knownAddress: String?,
        bonded: Set<String>,
        hasUsableNetwork: Boolean,
        otherRoadsFailed: Boolean,
        now: Long,
        restUntil: Long,
    ): Boolean {
        if (!enabled || !permitted || !radioOn || knownAddress == null) return false
        val known = knownAddress.uppercase()
        if (bonded.none { it.uppercase() == known }) return false
        if (hasUsableNetwork && !otherRoadsFailed) return false
        return now >= restUntil
    }

    /**
     * Whether a transfer of [bytes] may start now on [road] (the control
     * link's, `nativeControlRoad`): on Bluetooth, a file over
     * [TRANSFER_MAX] waits for Wi-Fi, and any file waits while the phone
     * plays to a Bluetooth headset or is in a call ([audioBusy]), so the
     * radio stays the audio's. Home and the internet: always.
     */
    fun transferMayStart(road: Int, bytes: Long, audioBusy: Boolean): TransferVerdict = when {
        road != ROAD_BLUETOOTH -> TransferVerdict.Go
        bytes > TRANSFER_MAX -> TransferVerdict.WaitForWifi
        audioBusy -> TransferVerdict.WaitForAudio
        else -> TransferVerdict.Go
    }

    /** What the switch says under it. */
    enum class SwitchSays {
        /** On, and the phone may use Bluetooth. */
        WORKS,

        /** Off. */
        OFF,

        /** On, but without the nearby devices permission it does nothing: Settings says why. */
        NEEDS_PERMISSION,
    }

    /** What the switch says when it is [on] and the phone may ([permitted]) use Bluetooth. */
    fun switchSays(on: Boolean, permitted: Boolean): SwitchSays = when {
        !on -> SwitchSays.OFF
        !permitted -> SwitchSays.NEEDS_PERMISSION
        else -> SwitchSays.WORKS
    }

    /** The rest after [fails] failed dials in a row: 30 s, doubling, at most 8 minutes. */
    fun nextRest(fails: Int): Long {
        if (fails < 1) return 0L
        return (FIRST_REST_MS shl (fails - 1).coerceAtMost(8)).coerceAtMost(MAX_REST_MS)
    }

    /** Whether a control link on [road] stays on Bluetooth: not once home or the internet answers again. */
    fun keepBluetooth(road: Int, homeOrInternetUp: Boolean): Boolean =
        road == ROAD_BLUETOOTH && !homeOrInternetUp

    /**
     * Whether a live Bluetooth road checks home now: once on each network
     * change to a usable one ([networkChanged]), and on the periodic tick
     * only with a local network up ([localNetwork]: Wi-Fi, Ethernet or the
     * phone's hotspot), never every few seconds over mobile data alone.
     */
    fun probeHome(networkChanged: Boolean, usableNetwork: Boolean, localNetwork: Boolean): Boolean =
        usableNetwork && (networkChanged || localNetwork)

    /**
     * Whether to leave Bluetooth for the usual roads: home answers again
     * ([homeUp]), or, once on a network change, the computer has an
     * internet road ([internetRoad]) to try (the usual connect falls back
     * to Bluetooth if that fails too).
     */
    fun leaveBluetooth(homeUp: Boolean, networkChanged: Boolean, internetRoad: Boolean): Boolean =
        !keepBluetooth(ROAD_BLUETOOTH, homeOrInternetUp = homeUp || (networkChanged && internetRoad))

    /**
     * Whether what a Bluetooth dial got ([state], the bridge's connection
     * state) is the refusal no retry changes: a computer this phone has not
     * verified at home. Its reason is the one to show; any other failure,
     * retryable or not, is not.
     */
    fun finalRefusal(state: String?): Boolean =
        state != null && state.startsWith("Error(") && state.contains("is_retryable: false") &&
            state.contains("has not verified the computer at") && state.contains("connect at home first")

    /** Whether to take the next link request: the computer serves one transfer link at a time, so none while one is up. */
    fun takeLinkRequest(linkLive: Boolean): Boolean = !linkLive

    /** Whether another try to open a link started at [startedAt] may go at [now], a step later, within [LINK_TRY_MS]. */
    fun tryLinkAgain(startedAt: Long, now: Long): Boolean =
        now + LINK_STEP_MS < startedAt + LINK_TRY_MS

    /** The bridge's `{address, service, link}` (`nativeBluetoothInfo`), or null when it is empty or does not read. */
    fun info(json: String?): Info? {
        if (json.isNullOrBlank()) return null
        return runCatching {
            val o = JSONObject(json)
            val address = o.getString("address").uppercase()
            if (!ADDRESS.matches(address)) return null
            Info(address, UUID.fromString(o.getString("service")), UUID.fromString(o.getString("link")))
        }.getOrNull()
    }

    private val ADDRESS = Regex("^[0-9A-F]{2}(:[0-9A-F]{2}){5}$")
}

/** Whether a transfer goes now ([BluetoothRoadRule.transferMayStart]). */
enum class TransferVerdict {
    Go,

    /** Over 8 MiB on the Bluetooth road: it goes once Wi-Fi is back. */
    WaitForWifi,

    /** On the Bluetooth road while a call or music to a headset uses the radio. */
    WaitForAudio,
}
