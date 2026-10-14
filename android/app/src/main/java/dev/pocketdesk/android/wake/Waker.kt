package dev.pocketdesk.android.wake

import android.content.Context
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.bridge.RustCore
import org.json.JSONObject
import java.net.Inet4Address
import java.net.NetworkInterface

/**
 * Wakes a saved computer the way its [WakePlan] says (N6): from the phone
 * itself on this network and from outside, or through another computer.
 * Blocking: call it off the main thread.
 */
object Waker {
    /** The plan for the computer saved at [address]. */
    fun planFor(ctx: Context, address: String): WakePlan.Plan =
        WakePlan.plan(
            HostStore.wakeSettings(ctx, address),
            HostStore.wakeInfo(ctx, address),
            wanAddrs(HostStore.wanIdentity(ctx, address)),
        )

    /** Whether [plan] can send anything: the card shows Wake only then. */
    fun canWake(plan: WakePlan.Plan): Boolean = plan is WakePlan.Plan.Direct || plan is WakePlan.Plan.Relay

    /**
     * Send the wake for the computer at [address]. Returns how many
     * addresses the phone sent it to (1 for a relay that took the request).
     */
    fun wake(ctx: Context, address: String): Result<Int> =
        when (val plan = planFor(ctx, address)) {
            is WakePlan.Plan.Direct -> RustCore.wake(plan.mac, plan.targets)
            is WakePlan.Plan.Relay -> {
                val relay = HostStore.hosts(ctx).firstOrNull { it.address == plan.relay }
                if (relay == null) {
                    Result.failure(IllegalStateException("the computer that sends it is no longer saved"))
                } else {
                    RustCore.wakeViaRelay(relay.address, relay.controlPort, plan.mac, plan.broadcast).map { 1 }
                }
            }
            WakePlan.Plan.Off -> Result.failure(IllegalStateException("wake is off for this computer"))
            WakePlan.Plan.NoMac ->
                Result.failure(IllegalStateException("its MAC address is not known yet: connect once while it is awake"))
            WakePlan.Plan.NoRelay ->
                Result.failure(IllegalStateException("no computer is chosen to send it"))
        }

    /** Whether the phone is on the network of the computer at [address]. */
    fun atHome(ctx: Context, address: String): Boolean =
        WakePlan.atHome(HostStore.wakeInfo(ctx, address), phoneBroadcasts())

    /** The broadcast address of each IPv4 subnet the phone is on. */
    fun phoneBroadcasts(): Set<String> =
        runCatching {
            NetworkInterface.getNetworkInterfaces().toList()
                .filter { it.isUp && !it.isLoopback }
                .flatMap { it.interfaceAddresses }
                .filter { it.address is Inet4Address }
                .mapNotNull { it.broadcast?.hostAddress }
                .toSet()
        }.getOrDefault(emptySet())

    /** The `directAddrs` of a cached WAN identity. */
    private fun wanAddrs(identityJson: String?): List<String> =
        identityJson?.let { json ->
            runCatching {
                val arr = JSONObject(json).optJSONArray("directAddrs") ?: return@runCatching emptyList()
                List(arr.length()) { arr.optString(it) }
            }.getOrNull()
        }.orEmpty()
}
