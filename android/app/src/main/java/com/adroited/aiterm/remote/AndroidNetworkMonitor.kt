package com.adroited.aiterm.remote

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import java.io.Closeable
import java.util.concurrent.atomic.AtomicReference
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.asSharedFlow

/** Emits when Android changes the process default network (for example Wi-Fi to cellular). */
class AndroidNetworkMonitor(context: Context) : Closeable {
    private val connectivityManager =
        context.applicationContext.getSystemService(ConnectivityManager::class.java)
    private val activeNetwork = AtomicReference(connectivityManager.activeNetwork)
    private val mutableChanges = MutableSharedFlow<Unit>(extraBufferCapacity = 1)
    val changes: Flow<Unit> = mutableChanges.asSharedFlow()
    private val mutableBlocked = MutableStateFlow(false)
    val blocked = mutableBlocked.asStateFlow()

    private val callback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
            if (activeNetwork.getAndSet(network) != network) { mutableBlocked.value = false; mutableChanges.tryEmit(Unit) }
        }

        override fun onBlockedStatusChanged(network: Network, blocked: Boolean) {
            if (activeNetwork.get() == network) mutableBlocked.value = blocked
        }

        override fun onLost(network: Network) {
            if (activeNetwork.compareAndSet(network, null)) { mutableBlocked.value = false; mutableChanges.tryEmit(Unit) }
        }
    }

    init {
        connectivityManager.registerDefaultNetworkCallback(callback)
    }

    override fun close() {
        runCatching { connectivityManager.unregisterNetworkCallback(callback) }
    }
}
