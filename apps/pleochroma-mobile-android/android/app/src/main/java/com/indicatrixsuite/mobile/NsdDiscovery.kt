package com.indicatrixsuite.mobile

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import java.net.Inet4Address

/**
 * Looks for the paired computer on the local network (`_pleochroma._tcp`, announced over
 * mDNS) so the app can follow it to a new address.
 *
 * Browsing over Wi-Fi needs a [WifiManager.MulticastLock]; it is taken in [start] (when the
 * device has Wi-Fi) and released in [stop], so it is held only while browsing. A browse also ends by itself after
 * [BROWSE_MS]. Every method runs on the main thread (the callers hop there).
 *
 * Each resolved service is handed to [onFound] as plain text: its name, the TXT values `v`
 * (protocol) and `ca` (the CA tag), and its IPv4 addresses as a comma-separated `ip:port`
 * list. Rust decides whether it is the paired computer and checks it with a real
 * connection; nothing here trusts an announcement. The TXT record holds no secret.
 *
 * Whenever a browse that ran ends (on the timeout, on [stop], or because it could not start)
 * [onEnded] is called once, so Rust can drop its session and start a new browse the next
 * time a connection fails.
 */
@Suppress("DEPRECATION")
class NsdDiscovery(
    context: Context,
    private val onFound: (name: String, version: String, ca: String, addresses: String) -> Unit,
    private val onEnded: () -> Unit,
) {
    private val appContext = context.applicationContext
    private val handler = Handler(Looper.getMainLooper())
    private var lock: WifiManager.MulticastLock? = null
    private var listener: NsdManager.DiscoveryListener? = null
    private val waiting = ArrayDeque<NsdServiceInfo>()
    private val reported = HashSet<String>()
    private var resolving = false
    private val timeout = Runnable { stop() }

    private val manager: NsdManager get() = appContext.getSystemService(NsdManager::class.java)

    /**
     * Starts browsing, or restarts the timer when a browse already runs. Services seen
     * before are forgotten, so Rust hears about them again if they answer again.
     */
    fun start() {
        handler.removeCallbacks(timeout)
        handler.postDelayed(timeout, BROWSE_MS)
        reported.clear()
        if (listener != null) return
        // A device without Wi-Fi (the manifest declares it optional) has no WifiManager; its
        // network (Ethernet, say) needs no multicast lock, so the browse runs without one.
        val wifi: WifiManager? = appContext.getSystemService(WifiManager::class.java)
        lock = wifi?.createMulticastLock(LOCK_TAG)?.apply {
            setReferenceCounted(false)
            acquire()
        }
        val discovery = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(serviceType: String) {}

            override fun onServiceFound(info: NsdServiceInfo) {
                handler.post { enqueue(info) }
            }

            override fun onServiceLost(info: NsdServiceInfo) {}

            override fun onDiscoveryStopped(serviceType: String) {}

            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
                Log.w(TAG, "service browsing could not start: $errorCode")
                handler.post { stop() }
            }

            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {
                Log.w(TAG, "service browsing could not stop: $errorCode")
            }
        }
        listener = discovery
        try {
            manager.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, discovery)
        } catch (e: Exception) {
            stop()
            throw e
        }
    }

    /** Stops browsing and releases the multicast lock. Safe to call at any time. */
    fun stop() {
        handler.removeCallbacks(timeout)
        waiting.clear()
        reported.clear()
        resolving = false
        val running = listener
        listener = null
        if (running != null) {
            try {
                manager.stopServiceDiscovery(running)
            } catch (e: Exception) {
                Log.w(TAG, "stopping the service browse failed", e)
            }
        }
        lock?.let { if (it.isHeld) it.release() }
        lock = null
        if (running != null) onEnded()
    }

    private fun enqueue(info: NsdServiceInfo) {
        if (listener == null) return
        waiting.addLast(info)
        resolveNext()
    }

    /** One resolve at a time: older Android versions refuse a second one in parallel. */
    private fun resolveNext() {
        if (resolving || listener == null) return
        val next = waiting.removeFirstOrNull() ?: return
        resolving = true
        try {
            manager.resolveService(next, object : NsdManager.ResolveListener {
                override fun onResolveFailed(info: NsdServiceInfo, errorCode: Int) {
                    handler.post { resolved(null) }
                }

                override fun onServiceResolved(info: NsdServiceInfo) {
                    handler.post { resolved(info) }
                }
            })
        } catch (e: Exception) {
            Log.w(TAG, "resolving a service failed", e)
            resolved(null)
        }
    }

    private fun resolved(info: NsdServiceInfo?) {
        resolving = false
        if (info != null && listener != null) {
            val version = text(info, "v")
            val ca = text(info, "ca")
            val addresses = addressesOf(info)
            // The same name with other addresses is a different announcement: a service
            // that copies the real name must not hide the real one.
            if (addresses.isNotEmpty() && reported.add("${info.serviceName}|$addresses")) {
                onFound(info.serviceName, version, ca, addresses)
            }
        }
        resolveNext()
    }

    private fun text(info: NsdServiceInfo, key: String): String =
        info.attributes[key]?.toString(Charsets.UTF_8) ?: ""

    /** The IPv4 addresses of a resolved service as `ip:port`, comma-separated. */
    private fun addressesOf(info: NsdServiceInfo): String {
        val hosts = if (Build.VERSION.SDK_INT >= 34) {
            info.hostAddresses
        } else {
            listOfNotNull(info.host)
        }
        return hosts.filterIsInstance<Inet4Address>()
            .mapNotNull { it.hostAddress }
            .distinct()
            .joinToString(",") { "$it:${info.port}" }
    }

    companion object {
        private const val TAG = "pleochroma"
        private const val LOCK_TAG = "pleochroma-discovery"

        /** The service type the coordinator announces (`discovery::txt::SERVICE_TYPE`). */
        private const val SERVICE_TYPE = "_pleochroma._tcp"

        /** A browse ends by itself after this long. */
        private const val BROWSE_MS = 30_000L
    }
}
