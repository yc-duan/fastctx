package dev.fastctx.world

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.Proxy
import android.os.Build
import org.json.JSONArray
import org.json.JSONObject
import java.net.InetSocketAddress
import java.net.ProxySelector
import java.net.URI

class WorldNetwork(private val context: Context) {
    private val manager = context.getSystemService(ConnectivityManager::class.java)
    private data class Entry(var capabilities: NetworkCapabilities? = null, var properties: LinkProperties? = null)
    private val networks = mutableMapOf<Network, Entry>()
    private var defaultNetwork: Network? = null
    private var defaultCapabilities: NetworkCapabilities? = null
    private val lock = Any()

    private fun changed() = WorldRuntime.reconnect()
    private fun routeChanged(before: NetworkCapabilities?, after: NetworkCapabilities): Boolean =
        before == null || listOf(NetworkCapabilities.NET_CAPABILITY_INTERNET,
            NetworkCapabilities.NET_CAPABILITY_NOT_VPN, NetworkCapabilities.NET_CAPABILITY_VALIDATED)
            .any { before.hasCapability(it) != after.hasCapability(it) }
    private val physical = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) { synchronized(lock) { networks.putIfAbsent(network, Entry()) } }
        override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) {
            val changed = synchronized(lock) {
                val entry = networks.getOrPut(network) { Entry() }
                val changed = routeChanged(entry.capabilities, capabilities)
                entry.capabilities = capabilities
                changed
            }
            if (changed) changed()
        }
        override fun onLinkPropertiesChanged(network: Network, properties: LinkProperties) {
            val changed = synchronized(lock) {
                val entry = networks.getOrPut(network) { Entry() }
                val changed = entry.properties != properties
                entry.properties = properties
                changed
            }
            if (changed) changed()
        }
        override fun onLost(network: Network) { synchronized(lock) { networks.remove(network) }; changed() }
    }
    private val defaults = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) { synchronized(lock) { defaultNetwork = network; defaultCapabilities = null }; changed() }
        override fun onLost(network: Network) {
            synchronized(lock) { if (defaultNetwork == network) defaultNetwork = null }
            changed()
        }
        override fun onLinkPropertiesChanged(network: Network, properties: LinkProperties) = changed()
        override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) {
            val changed = synchronized(lock) {
                val changed = routeChanged(defaultCapabilities, capabilities)
                defaultCapabilities = capabilities
                changed
            }
            if (changed) changed()
        }
    }
    private val proxy = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) = changed()
    }

    /** Follows the phone's networks. Should a step fail, what was followed is let go, so trying
     *  again does not leave a second set of callbacks behind. */
    fun start() {
        try {
            manager.registerNetworkCallback(NetworkRequest.Builder()
                .addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
                .addCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN).build(), physical)
            manager.registerDefaultNetworkCallback(defaults)
            if (Build.VERSION.SDK_INT >= 33) context.registerReceiver(proxy, IntentFilter(Proxy.PROXY_CHANGE_ACTION), Context.RECEIVER_NOT_EXPORTED)
            else context.registerReceiver(proxy, IntentFilter(Proxy.PROXY_CHANGE_ACTION))
        } catch (error: RuntimeException) {
            runCatching { manager.unregisterNetworkCallback(physical) }
            runCatching { manager.unregisterNetworkCallback(defaults) }
            runCatching { context.unregisterReceiver(proxy) }
            throw error
        }
    }

    fun choices(target: String): String {
        val available = synchronized(lock) {
            networks.entries.filter { (_, entry) ->
                entry.capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) == true &&
                entry.capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN) == true &&
                entry.properties?.linkAddresses?.isNotEmpty() == true
            }.sortedBy { if (it.key == defaultNetwork) 0 else 1 }.map { (network, entry) ->
                JSONObject().put("handle", network.networkHandle).put("name", entry.properties!!.interfaceName ?: network.toString())
            }
        }
        val proxies = ProxySelector.getDefault().select(URI("https://$target/"))
        val selected = proxies.map { proxy ->
            when (proxy.type()) {
                java.net.Proxy.Type.DIRECT -> JSONObject().put("kind", "direct")
                java.net.Proxy.Type.HTTP -> {
                    val endpoint = proxy.address() as InetSocketAddress
                    JSONObject().put("kind", "http").put("host", endpoint.hostString).put("port", endpoint.port)
                }
                else -> JSONObject().put("kind", "unsupported").put("description", proxy.type().toString())
            }
        }
        return JSONObject().put("networks", JSONArray(available)).put("proxies", JSONArray(selected)).toString()
    }
}
