package dev.linuxlink.android.discovery

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import android.util.Log
import androidx.annotation.RequiresApi
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.State
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import java.net.InetSocketAddress
import java.net.Socket
import java.util.concurrent.Executor
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/**
 * Browses the network for Pocketdesk computers (`_linux-link._tcp`, which
 * the server announces over mDNS) and keeps the list of those found.
 *
 * `NsdManager` takes the multicast lock itself. Its callbacks arrive on its
 * own threads, so all state is changed under this object's lock.
 */
class LanDiscovery(context: Context) {
    private val nsd: NsdManager? = context.getSystemService(NsdManager::class.java)
    private val mainExecutor: Executor = ContextCompat.getMainExecutor(context)
    private val byService = linkedMapOf<String, FoundComputer>()
    private val found = MutableStateFlow<List<FoundComputer>>(emptyList())
    private var listener: NsdManager.DiscoveryListener? = null

    // API 34+: one live info callback per service, which also reports
    // address changes. Older systems resolve one service at a time.
    private val infoCallbacks = mutableMapOf<String, Any>()
    private val waiting = ArrayDeque<NsdServiceInfo>()
    private var resolving = false

    // A record can outlive its computer: a server that crashed sent no
    // goodbye, and the phone's mDNS cache keeps it for up to 75 minutes. And
    // a server announces every address it has, bridges and VMs included. So
    // a computer is listed only at an address whose control port answers.
    // `generation` is bumped on stop, and each service's newest record has
    // a token, so a probe from an earlier browse or record changes nothing.
    private val prober = Executors.newSingleThreadScheduledExecutor { r ->
        Thread(r, "lan-probe").apply { isDaemon = true }
    }
    private var generation = 0
    private var nextToken = 0L
    private val probeToken = mutableMapOf<String, Long>()

    val computers: StateFlow<List<FoundComputer>> = found

    @Synchronized
    fun start() {
        val nsd = nsd ?: return
        if (listener != null || prober.isShutdown) return
        val l = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(serviceType: String) {
                Log.d(TAG, "browsing $serviceType")
            }

            override fun onServiceFound(info: NsdServiceInfo) = resolve(info)

            override fun onServiceLost(info: NsdServiceInfo) = forget(info.serviceName)

            override fun onDiscoveryStopped(serviceType: String) = Unit

            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
                Log.w(TAG, "discovery failed to start: $errorCode")
                synchronized(this@LanDiscovery) { listener = null }
            }

            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {
                Log.w(TAG, "discovery failed to stop: $errorCode")
            }
        }
        listener = l
        runCatching { nsd.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, l) }
            .onFailure {
                Log.w(TAG, "discovery unavailable", it)
                listener = null
            }
    }

    @Synchronized
    fun stop() {
        val nsd = nsd ?: return
        listener?.let { runCatching { nsd.stopServiceDiscovery(it) } }
        listener = null
        generation++
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            infoCallbacks.values.forEach { unregister(nsd, it) }
        }
        infoCallbacks.clear()
        waiting.clear()
        resolving = false
        probeToken.clear()
        byService.clear()
        found.value = emptyList()
    }

    /** Stop for good: the screen that owned this is gone. */
    fun close() {
        stop()
        prober.shutdownNow()
    }

    @Synchronized
    private fun resolve(info: NsdServiceInfo) {
        val nsd = nsd ?: return
        if (listener == null) return
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            if (info.serviceName !in infoCallbacks) {
                watch(nsd, info)?.let { infoCallbacks[info.serviceName] = it }
            }
        } else {
            waiting.addLast(info)
            resolveNext()
        }
    }

    @RequiresApi(Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
    private fun watch(nsd: NsdManager, info: NsdServiceInfo): Any? {
        val callback = object : NsdManager.ServiceInfoCallback {
            override fun onServiceInfoCallbackRegistrationFailed(errorCode: Int) {
                Log.w(TAG, "cannot resolve ${info.serviceName}: $errorCode")
                // Let a later sighting of this service try again.
                synchronized(this@LanDiscovery) { infoCallbacks.remove(info.serviceName) }
            }

            override fun onServiceUpdated(resolved: NsdServiceInfo) {
                record(resolved, resolved.hostAddresses.mapNotNull { it.hostAddress })
            }

            override fun onServiceLost() = forget(info.serviceName)

            override fun onServiceInfoCallbackUnregistered() = Unit
        }
        return runCatching { nsd.registerServiceInfoCallback(info, mainExecutor, callback) }
            .map { callback }
            .onFailure { Log.w(TAG, "cannot resolve ${info.serviceName}", it) }
            .getOrNull()
    }

    @RequiresApi(Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
    private fun unregister(nsd: NsdManager, callback: Any) {
        runCatching { nsd.unregisterServiceInfoCallback(callback as NsdManager.ServiceInfoCallback) }
    }

    @Suppress("DEPRECATION") // resolveService is the only way before API 34.
    @Synchronized
    private fun resolveNext() {
        val nsd = nsd ?: return
        if (resolving) return
        val next = waiting.removeFirstOrNull() ?: return
        resolving = true
        val resolveOf = generation
        // A resolve that never calls back would hold up every other one.
        val timeout = prober.schedule(
            Runnable { done(resolveOf) },
            RESOLVE_TIMEOUT_MS,
            TimeUnit.MILLISECONDS,
        )
        val started = runCatching {
            nsd.resolveService(
                next,
                object : NsdManager.ResolveListener {
                    override fun onServiceResolved(resolved: NsdServiceInfo) {
                        timeout.cancel(false)
                        record(resolved, listOfNotNull(resolved.host?.hostAddress))
                        done(resolveOf)
                    }

                    override fun onResolveFailed(failed: NsdServiceInfo, errorCode: Int) {
                        timeout.cancel(false)
                        Log.w(TAG, "cannot resolve ${failed.serviceName}: $errorCode")
                        done(resolveOf)
                    }
                },
            )
        }
        if (started.isFailure) {
            timeout.cancel(false)
            Log.w(TAG, "cannot resolve ${next.serviceName}", started.exceptionOrNull())
            resolving = false
            resolveNext()
        }
    }

    /** The resolve begun in browse [resolveOf] is over; start the next one. */
    @Synchronized
    private fun done(resolveOf: Int) {
        if (resolveOf != generation || !resolving) return
        resolving = false
        resolveNext()
    }

    @Synchronized
    private fun record(info: NsdServiceInfo, addresses: List<String>) {
        if (listener == null) return
        val txt = info.attributes.mapValues { (_, v) -> v?.toString(Charsets.UTF_8).orEmpty() }
        val computer = FoundComputers.fromRecord(info.serviceName, info.port, txt, addresses)
        val token = nextToken++
        probeToken[info.serviceName] = token
        if (computer == null) {
            // Updated to no usable address: it is no longer reachable here.
            if (byService.remove(info.serviceName) != null) found.value = byService.values.toList()
            return
        }
        probe(info.serviceName, computer, generation, token, attempt = 0)
    }

    private fun probe(serviceName: String, computer: FoundComputer, probeOf: Int, token: Long, attempt: Int) {
        if (prober.isShutdown) return
        runCatching {
            prober.schedule(
            Runnable {
                val answering = computer.candidates.firstOrNull { answers(it, computer.controlPort) }
                synchronized(this) {
                    val current = probeOf == generation && listener != null &&
                        probeToken[serviceName] == token
                    if (!current) return@Runnable
                    when {
                        answering != null -> {
                            Log.d(TAG, "found ${computer.name} at $answering:${computer.controlPort}")
                            byService[serviceName] = computer.copy(address = answering)
                        }
                        attempt + 1 < RETRY_DELAYS_MS.size -> {
                            // A server still starting, or a lost packet.
                            probe(serviceName, computer, probeOf, token, attempt + 1)
                            return@Runnable
                        }
                        else -> {
                            Log.d(TAG, "${computer.name} is announced but does not answer; not listed")
                            byService.remove(serviceName)
                        }
                    }
                    found.value = byService.values.toList()
                }
            },
            RETRY_DELAYS_MS[attempt],
            TimeUnit.MILLISECONDS,
            )
        }
    }

    /** Whether [address]'s control port accepts a connection. */
    private fun answers(address: String, port: Int): Boolean = runCatching {
        Socket().use { it.connect(InetSocketAddress(address, port), PROBE_MS) }
    }.isSuccess

    @Synchronized
    private fun forget(serviceName: String) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            infoCallbacks.remove(serviceName)?.let { cb -> nsd?.let { unregister(it, cb) } }
        }
        probeToken.remove(serviceName)
        if (byService.remove(serviceName) != null) {
            found.value = byService.values.toList()
        }
    }

    companion object {
        private const val TAG = "LanDiscovery"
        const val SERVICE_TYPE = "_linux-link._tcp"
        private const val PROBE_MS = 1500
        private const val RESOLVE_TIMEOUT_MS = 10_000L
        private val RETRY_DELAYS_MS = longArrayOf(0, 500, 1500, 4000)
    }
}

/**
 * The computers found on this network while the calling screen is in front;
 * browsing stops when it is covered or the app goes to the background, and
 * starts again on return (which also picks up a just-granted permission).
 */
@Composable
fun rememberFoundComputers(): State<List<FoundComputer>> {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    val discovery = remember { LanDiscovery(context.applicationContext) }
    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_RESUME -> discovery.start()
                Lifecycle.Event.ON_PAUSE -> discovery.stop()
                else -> Unit
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose {
            lifecycleOwner.lifecycle.removeObserver(observer)
            discovery.close()
        }
    }
    return discovery.computers.collectAsState()
}
