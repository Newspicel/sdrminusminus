package dev.newspicel.sdrmm.pair

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import androidx.annotation.RequiresApi
import dev.newspicel.sdrmm.ffi.DiscoveredServer
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import java.net.Inet4Address
import java.net.Inet6Address
import java.net.InetAddress
import java.util.concurrent.Executor

interface Discovery {
    val found: StateFlow<List<DiscoveredServer>>
    val failed: StateFlow<Boolean>

    fun start()

    fun stop()
}

class NsdDiscovery(
    context: Context,
) : Discovery {
    private val manager = context.getSystemService(NsdManager::class.java)
    private val main = Handler(Looper.getMainLooper())
    private val executor = Executor { main.post(it) }
    private val servers = MutableStateFlow<Map<String, DiscoveredServer>>(emptyMap())
    private val list = MutableStateFlow<List<DiscoveredServer>>(emptyList())
    private val broken = MutableStateFlow(false)
    private val watchers = mutableMapOf<String, NsdManager.ServiceInfoCallback>()
    private val waiting = ArrayDeque<NsdServiceInfo>()
    private var resolving = false
    private var users = 0
    private var listener: NsdManager.DiscoveryListener? = null

    override val found: StateFlow<List<DiscoveredServer>> = list.asStateFlow()
    override val failed: StateFlow<Boolean> = broken.asStateFlow()

    override fun start() {
        main.post {
            users += 1
            if (users == 1) begin()
        }
    }

    override fun stop() {
        main.post {
            users = (users - 1).coerceAtLeast(0)
            if (users == 0) end()
        }
    }

    private fun begin() {
        if (manager == null) {
            broken.value = true
            return
        }
        broken.value = false
        val started = discoveryListener()
        listener = started
        manager.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, started)
    }

    private fun end() {
        val running = listener
        listener = null
        if (running != null) whileListenerExists { manager?.stopServiceDiscovery(running) }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            watchers.values.forEach { whileListenerExists { manager?.unregisterServiceInfoCallback(it) } }
        }
        watchers.clear()
        waiting.clear()
        resolving = false
        publish { emptyMap() }
    }

    private fun discoveryListener() = object : NsdManager.DiscoveryListener {
        override fun onDiscoveryStarted(serviceType: String) = Unit

        override fun onDiscoveryStopped(serviceType: String) = Unit

        override fun onStartDiscoveryFailed(
            serviceType: String,
            errorCode: Int,
        ) {
            Log.w(TAG, "NSD discovery failed with $errorCode")
            main.post {
                listener = null
                broken.value = true
            }
        }

        override fun onStopDiscoveryFailed(
            serviceType: String,
            errorCode: Int,
        ) {
            Log.w(TAG, "NSD stop failed with $errorCode")
        }

        override fun onServiceFound(info: NsdServiceInfo) {
            main.post { resolve(info) }
        }

        override fun onServiceLost(info: NsdServiceInfo) {
            main.post { lost(info.serviceName) }
        }
    }

    private fun resolve(info: NsdServiceInfo) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            watch(info)
        } else {
            waiting.addLast(info)
            resolveNext()
        }
    }

    @RequiresApi(Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
    private fun watch(info: NsdServiceInfo) {
        if (manager == null || info.serviceName in watchers) return
        val callback =
            object : NsdManager.ServiceInfoCallback {
                override fun onServiceInfoCallbackRegistrationFailed(errorCode: Int) {
                    Log.w(TAG, "NSD resolve failed with $errorCode")
                }

                override fun onServiceUpdated(serviceInfo: NsdServiceInfo) {
                    put(serviceInfo, serviceInfo.hostAddresses)
                }

                override fun onServiceLost() {
                    lost(info.serviceName)
                }

                override fun onServiceInfoCallbackUnregistered() = Unit
            }
        watchers[info.serviceName] = callback
        manager.registerServiceInfoCallback(info, executor, callback)
    }

    @Suppress("DEPRECATION")
    private fun resolveNext() {
        if (resolving || manager == null) return
        val next = waiting.removeFirstOrNull() ?: return
        resolving = true
        manager.resolveService(
            next,
            object : NsdManager.ResolveListener {
                override fun onResolveFailed(
                    serviceInfo: NsdServiceInfo,
                    errorCode: Int,
                ) {
                    main.post {
                        resolving = false
                        if (errorCode == NsdManager.FAILURE_ALREADY_ACTIVE) {
                            waiting.addFirst(next)
                            main.postDelayed(::resolveNext, RETRY_MS)
                        } else {
                            Log.w(TAG, "NSD resolve failed with $errorCode")
                            resolveNext()
                        }
                    }
                }

                override fun onServiceResolved(serviceInfo: NsdServiceInfo) {
                    main.post {
                        resolving = false
                        put(serviceInfo, listOfNotNull(serviceInfo.host))
                        resolveNext()
                    }
                }
            },
        )
    }

    private fun put(
        info: NsdServiceInfo,
        addresses: List<InetAddress>,
    ) {
        if (listener == null) return
        val hosts = hostsOf(addresses, info.port)
        if (hosts.isEmpty()) return
        val txt = info.attributes.mapValues { (_, value) -> value?.toString(Charsets.UTF_8) ?: "" }
        publish { it + (info.serviceName to DiscoveredServer(info.serviceName, hosts, txt)) }
    }

    private fun lost(name: String) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            watchers.remove(name)?.let { callback -> whileListenerExists { manager?.unregisterServiceInfoCallback(callback) } }
        }
        publish { it - name }
    }

    private fun publish(change: (Map<String, DiscoveredServer>) -> Map<String, DiscoveredServer>) {
        servers.update(change)
        list.value = servers.value.values.sortedBy { it.name }
    }

    private inline fun whileListenerExists(block: () -> Unit) {
        try {
            block()
        } catch (error: IllegalArgumentException) {
            Log.w(TAG, "NSD listener already gone", error)
        }
    }

    companion object {
        const val SERVICE_TYPE = "_sdrmm._tcp"
        private const val TAG = "SdrmmDiscovery"
        private const val RETRY_MS = 200L

        fun hostsOf(
            addresses: List<InetAddress>,
            port: Int,
        ): List<String> {
            val v4 = addresses.filterIsInstance<Inet4Address>().map { "${it.hostAddress}:$port" }
            val v6 =
                addresses
                    .filterIsInstance<Inet6Address>()
                    .filterNot { it.isLinkLocalAddress }
                    .map { "[${it.hostAddress?.substringBefore('%')}]:$port" }
            return (v4 + v6).distinct()
        }
    }
}
