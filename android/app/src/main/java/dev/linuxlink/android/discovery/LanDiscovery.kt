package dev.linuxlink.android.discovery

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import android.util.Log
import androidx.annotation.RequiresApi
import androidx.core.content.ContextCompat
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.State
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import java.net.InetSocketAddress
import java.net.Socket
import java.util.concurrent.Executor
import java.util.concurrent.Executors

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
    // goodbye, and the phone's mDNS cache keeps it for up to 75 minutes. So
    // a computer is listed only once its control port answers. Bumped on
    // stop, so a probe from an earlier browse cannot add to this one.
    private val prober = Executors.newSingleThreadExecutor { r ->
        Thread(r, "lan-probe").apply { isDaemon = true }
    }
    private var generation = 0

    val computers: StateFlow<List<FoundComputer>> = found

    @Synchronized
    fun start() {
        val nsd = nsd ?: return
        if (listener != null) return
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
        byService.clear()
        found.value = emptyList()
    }

    @Synchronized
    private fun resolve(info: NsdServiceInfo) {
        val nsd = nsd ?: return
        if (listener == null) return
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            if (info.serviceName !in infoCallbacks) {
                infoCallbacks[info.serviceName] = watch(nsd, info)
            }
        } else {
            waiting.addLast(info)
            resolveNext()
        }
    }

    @RequiresApi(Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
    private fun watch(nsd: NsdManager, info: NsdServiceInfo): Any {
        val callback = object : NsdManager.ServiceInfoCallback {
            override fun onServiceInfoCallbackRegistrationFailed(errorCode: Int) {
                Log.w(TAG, "cannot resolve ${info.serviceName}: $errorCode")
            }

            override fun onServiceUpdated(resolved: NsdServiceInfo) {
                record(resolved, resolved.hostAddresses.mapNotNull { it.hostAddress })
            }

            override fun onServiceLost() = forget(info.serviceName)

            override fun onServiceInfoCallbackUnregistered() = Unit
        }
        nsd.registerServiceInfoCallback(info, mainExecutor, callback)
        return callback
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
        nsd.resolveService(
            next,
            object : NsdManager.ResolveListener {
                override fun onServiceResolved(resolved: NsdServiceInfo) {
                    record(resolved, listOfNotNull(resolved.host?.hostAddress))
                    done()
                }

                override fun onResolveFailed(failed: NsdServiceInfo, errorCode: Int) {
                    Log.w(TAG, "cannot resolve ${failed.serviceName}: $errorCode")
                    done()
                }

                private fun done() {
                    synchronized(this@LanDiscovery) { resolving = false }
                    resolveNext()
                }
            },
        )
    }

    @Synchronized
    private fun record(info: NsdServiceInfo, addresses: List<String>) {
        if (listener == null) return
        val txt = info.attributes.mapValues { (_, v) -> v?.toString(Charsets.UTF_8).orEmpty() }
        val computer = FoundComputers.fromRecord(info.serviceName, info.port, txt, addresses)
            ?: return
        val probeOf = generation
        prober.execute {
            val reachable = answers(computer)
            synchronized(this) {
                if (probeOf != generation || listener == null) return@execute
                if (reachable) {
                    Log.d(TAG, "found ${computer.name} at ${computer.address}:${computer.controlPort}")
                    byService[info.serviceName] = computer
                } else {
                    Log.d(TAG, "${computer.name} is announced but does not answer; not listed")
                    byService.remove(info.serviceName)
                }
                found.value = byService.values.toList()
            }
        }
    }

    /** Whether the computer's control port accepts a connection. */
    private fun answers(computer: FoundComputer): Boolean = runCatching {
        Socket().use { it.connect(InetSocketAddress(computer.address, computer.controlPort), PROBE_MS) }
    }.isSuccess

    @Synchronized
    private fun forget(serviceName: String) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            infoCallbacks.remove(serviceName)?.let { cb -> nsd?.let { unregister(it, cb) } }
        }
        if (byService.remove(serviceName) != null) {
            found.value = byService.values.toList()
        }
    }

    companion object {
        private const val TAG = "LanDiscovery"
        const val SERVICE_TYPE = "_linux-link._tcp"
        private const val PROBE_MS = 1500
    }
}

/**
 * The computers found on this network while the calling screen is started;
 * browsing stops when it leaves the screen or the app goes to the background.
 */
@Composable
fun rememberFoundComputers(): State<List<FoundComputer>> {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    val discovery = remember { LanDiscovery(context.applicationContext) }
    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_START -> discovery.start()
                Lifecycle.Event.ON_STOP -> discovery.stop()
                else -> Unit
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose {
            lifecycleOwner.lifecycle.removeObserver(observer)
            discovery.stop()
        }
    }
    return discovery.computers.collectAsState()
}
