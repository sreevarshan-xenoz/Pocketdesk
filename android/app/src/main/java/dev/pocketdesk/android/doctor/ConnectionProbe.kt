package dev.pocketdesk.android.doctor

import android.Manifest
import android.app.ActivityManager
import android.content.Context
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.Build
import android.os.PowerManager
import android.system.ErrnoException
import android.system.OsConstants
import androidx.core.app.NotificationManagerCompat
import dev.pocketdesk.android.bridge.RustCore
import dev.pocketdesk.android.notify.PhoneNotificationPrefs
import dev.pocketdesk.android.notify.TextPrefs
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.withContext
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.Inet4Address
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.NoRouteToHostException
import java.net.PortUnreachableException
import java.net.Socket
import java.net.SocketTimeoutException
import java.net.UnknownHostException
import javax.net.SocketFactory

/**
 * The Connection Doctor's probes (T1): the phone's own state, read once, and
 * three probes of one computer, run side by side. Nothing here is periodic,
 * nothing scans the network, and the identity check never saves a pin.
 */
object ConnectionProbe {
    private const val TCP_TIMEOUT_MS = 4_000
    private const val UDP_WAIT_MS = 700
    private const val UDP_TRIES = 3
    private const val ANDROID_17 = 37

    /** The connections this phone shares, read from its interfaces (no permission needed). */
    private fun sharedConnections(): List<Shared> =
        runCatching {
            java.net.NetworkInterface.getNetworkInterfaces().toList()
                .filter { it.isUp }
                .flatMap { nic ->
                    val sharing = ConnectionChecks.sharingOf(nic.name) ?: return@flatMap emptyList()
                    nic.interfaceAddresses
                        .filter { it.address is Inet4Address }
                        .map { Shared(sharing, it.address.hostAddress ?: "", it.networkPrefixLength.toInt()) }
                }
        }.getOrDefault(emptyList())

    /** The phone's state, as the checks need it. */
    @Suppress("DEPRECATION") // allNetworks: the Wi-Fi under a VPN has no other public lookup.
    fun phoneState(ctx: Context): PhoneState {
        val cm = ctx.getSystemService(ConnectivityManager::class.java)
        val active = cm.activeNetwork
        val caps = active?.let { cm.getNetworkCapabilities(it) }
        val props = active?.let { cm.getLinkProperties(it) }
        val link = when {
            caps == null -> Link.NONE
            caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> Link.WIFI
            caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> Link.CELLULAR
            caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> Link.ETHERNET
            else -> lanNetwork(cm)?.let { cm.getNetworkCapabilities(it) }?.let { lanCaps ->
                when {
                    lanCaps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> Link.WIFI
                    lanCaps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> Link.ETHERNET
                    lanCaps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> Link.CELLULAR
                    else -> Link.OTHER
                }
            } ?: Link.OTHER
        }
        val lanProps = lanNetwork(cm)?.let { cm.getLinkProperties(it) }
        val prefixes = lanProps?.linkAddresses.orEmpty()
            .filter { it.address is Inet4Address }
            .map { (it.address.hostAddress ?: "") to it.prefixLength }
        val hasIpv4 = (props?.linkAddresses.orEmpty() + lanProps?.linkAddresses.orEmpty())
            .any { it.address is Inet4Address }
        val power = ctx.getSystemService(PowerManager::class.java)
        val activity = ctx.getSystemService(ActivityManager::class.java)
        val notificationsOn = PhoneNotificationPrefs.enabled(ctx)
        val textsOn = TextPrefs.show(ctx) || TextPrefs.reply(ctx)
        // The VPN is Tailscale's when it gives this phone a tailnet address.
        val vpnTailnet = active?.takeIf { caps?.hasTransport(NetworkCapabilities.TRANSPORT_VPN) == true }
            ?.let { cm.getLinkProperties(it) }?.linkAddresses.orEmpty()
            .any { ConnectionChecks.kindOf(it.address.hostAddress ?: "") == AddressKind.TAILNET }
        return PhoneState(
            link = link,
            captivePortal = caps?.hasCapability(NetworkCapabilities.NET_CAPABILITY_CAPTIVE_PORTAL) == true,
            validated = caps?.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED) == true,
            hasIpv4 = hasIpv4 || caps == null,
            vpn = caps?.hasTransport(NetworkCapabilities.TRANSPORT_VPN) == true,
            vpnTailnet = vpnTailnet,
            localNetworkAllowed = if (Build.VERSION.SDK_INT >= ANDROID_17) {
                ctx.checkSelfPermission(Manifest.permission.ACCESS_LOCAL_NETWORK) ==
                    PackageManager.PERMISSION_GRANTED
            } else {
                null
            },
            lanPrefixes = prefixes,
            shared = sharedConnections(),
            batteryOptimized = !power.isIgnoringBatteryOptimizations(ctx.packageName),
            backgroundRestricted = Build.VERSION.SDK_INT >= 28 && activity.isBackgroundRestricted,
            dataSaver = cm.restrictBackgroundStatus == ConnectivityManager.RESTRICT_BACKGROUND_STATUS_ENABLED,
            backgroundFeaturesOn = notificationsOn || textsOn,
            notificationsAllowed = NotificationManagerCompat.from(ctx).areNotificationsEnabled(),
            notificationAccess = if (notificationsOn || TextPrefs.reply(ctx)) {
                NotificationManagerCompat.getEnabledListenerPackages(ctx).contains(ctx.packageName)
            } else {
                null
            },
            smsAllowed = if (textsOn) {
                val granted = { permission: String ->
                    ctx.checkSelfPermission(permission) == PackageManager.PERMISSION_GRANTED
                }
                (!TextPrefs.show(ctx) || granted(Manifest.permission.RECEIVE_SMS)) &&
                    (!TextPrefs.reply(ctx) || granted(Manifest.permission.SEND_SMS))
            } else {
                null
            },
        )
    }

    /** The phone's Wi-Fi or Ethernet network, also when a VPN is the default. */
    @Suppress("DEPRECATION")
    private fun lanNetwork(cm: ConnectivityManager): Network? = cm.allNetworks.firstOrNull { network ->
        val caps = cm.getNetworkCapabilities(network) ?: return@firstOrNull false
        !caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN) &&
            (caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) ||
                caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET))
    }

    /** The three probes of one computer, side by side. */
    suspend fun probe(
        ctx: Context,
        address: String,
        controlPort: Int,
        streamPort: Int,
        vpn: Boolean,
    ): Probes = coroutineScope {
        val tcp = async(Dispatchers.IO) { tcp(address, controlPort, SocketFactory.getDefault()) }
        val viaWifi = async(Dispatchers.IO) {
            if (!vpn) {
                Tcp.NOT_RUN
            } else {
                val cm = ctx.getSystemService(ConnectivityManager::class.java)
                lanNetwork(cm)?.let { tcp(address, controlPort, it.socketFactory) } ?: Tcp.NOT_RUN
            }
        }
        val identity = async(Dispatchers.IO) { identity(address, controlPort) }
        val udp = async(Dispatchers.IO) { udp(address, streamPort) }
        Probes(tcp.await(), viaWifi.await(), identity.await(), udp.await())
    }

    /** A TCP connect to the control port, with why it failed. */
    fun tcp(address: String, port: Int, factory: SocketFactory): Tcp {
        val target = InetSocketAddress(address, port)
        if (target.isUnresolved) return Tcp.NO_NAME
        return try {
            factory.createSocket().use { it.connect(target, TCP_TIMEOUT_MS) }
            Tcp.OK
        } catch (_: SocketTimeoutException) {
            Tcp.TIMEOUT
        } catch (_: NoRouteToHostException) {
            Tcp.NO_ROUTE
        } catch (e: Exception) {
            when (errnoOf(e)) {
                OsConstants.ECONNREFUSED -> Tcp.REFUSED
                OsConstants.EHOSTUNREACH -> Tcp.NO_ROUTE
                OsConstants.ENETUNREACH -> Tcp.NET_UNREACHABLE
                OsConstants.EPERM, OsConstants.EACCES -> Tcp.DENIED
                OsConstants.ETIMEDOUT -> Tcp.TIMEOUT
                else -> if (e is UnknownHostException) Tcp.NO_NAME else Tcp.OTHER
            }
        }
    }

    private fun errnoOf(e: Throwable): Int? {
        var cause: Throwable? = e
        while (cause != null) {
            if (cause is ErrnoException) return cause.errno
            cause = cause.cause
        }
        return null
    }

    /** The compare-only identity check, through the bridge. */
    private fun identity(address: String, port: Int): Identity =
        when (RustCore.checkIdentity(address, port).getOrNull()) {
            "matches" -> Identity.MATCHES
            "changed" -> Identity.CHANGED
            "not_pinned" -> Identity.NOT_PINNED
            "not_pocketdesk" -> Identity.NOT_POCKETDESK
            "closed" -> Identity.CLOSED
            else -> Identity.FAILED
        }

    /**
     * The stream port, by a QUIC packet with a reserved version: the desktop
     * answers it with Version Negotiation, statelessly, without opening a
     * connection or spending its connection budget.
     */
    fun udp(address: String, port: Int): Udp {
        val target = runCatching { InetAddress.getByName(address) }.getOrNull() ?: return Udp.NOT_RUN
        return try {
            DatagramSocket().use { socket ->
                socket.connect(target, port)
                socket.soTimeout = UDP_WAIT_MS
                val probe = versionProbe()
                val reply = DatagramPacket(ByteArray(1500), 1500)
                repeat(UDP_TRIES) {
                    socket.send(DatagramPacket(probe, probe.size))
                    try {
                        socket.receive(reply)
                        if (isVersionNegotiation(reply.data, reply.length)) return Udp.ANSWERED
                    } catch (_: SocketTimeoutException) {
                        // Try again: one datagram may be lost.
                    }
                }
                Udp.SILENT
            }
        } catch (_: PortUnreachableException) {
            Udp.REFUSED
        } catch (_: Exception) {
            Udp.SILENT
        }
    }

    /** A QUIC Version Negotiation packet: a long header with version 0. */
    internal fun isVersionNegotiation(packet: ByteArray, length: Int): Boolean =
        length >= 7 && packet[0].toInt() and 0x80 != 0 && (1..4).all { packet[it].toInt() == 0 }

    /** A 1200-byte QUIC long-header packet with a reserved version (0x?a?a?a?a). */
    internal fun versionProbe(): ByteArray {
        val packet = ByteArray(1200)
        packet[0] = 0xC0.toByte() // long header, fixed bit
        packet[1] = 0x1A; packet[2] = 0x2A; packet[3] = 0x3A; packet[4] = 0x4A // reserved version
        packet[5] = 8 // destination connection id length
        for (i in 6 until 14) packet[i] = (0x50 + i).toByte()
        packet[14] = 8 // source connection id length
        for (i in 15 until 23) packet[i] = (0x60 + i).toByte()
        return packet
    }
}
