package com.adroited.aiterm.remote

import android.content.Context
import android.util.Base64
import android.util.Log
import computer.iroh.Connection
import computer.iroh.Endpoint
import computer.iroh.EndpointAddr
import computer.iroh.EndpointId
import computer.iroh.EndpointOptions
import computer.iroh.RelayMode
import computer.iroh.SecretKey
import computer.iroh.presetN0
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.security.MessageDigest
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

/** A loopback bridge into an Iroh bi-stream. Everything above this layer is
 * AITerm's existing pinned TLS, device-key authentication, and CBOR API. */
internal object IrohBridge {
    private val alpn = "aiterm/remote/0".toByteArray(Charsets.UTF_8)
    private const val tag = "AITermIroh"
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val lock = Mutex()
    private val endpoints = HashMap<String, Endpoint>()
    private val ports = HashMap<String, Int>()
    private val connections = HashMap<String, Connection>()

    suspend fun localPortFor(
        context: Context,
        nodeId: String,
        relayUrl: String? = null,
    ): Int = withContext(Dispatchers.IO) {
        lock.withLock {
            val relayKey = relayUrl.orEmpty()
            val routeKey = "$relayKey|$nodeId"
            ports[routeKey]?.let { return@withContext it }
            val ep = bind(context.applicationContext, relayUrl)
            val preferred = 40_000 + (routeKey.hashCode() and 0x7fffffff) % 20_000
            val server = runCatching {
                ServerSocket(preferred, 16, InetAddress.getByName("127.0.0.1"))
            }.getOrElse {
                ServerSocket(0, 16, InetAddress.getByName("127.0.0.1"))
            }
            ports[routeKey] = server.localPort
            scope.launch { acceptLoop(server, ep, routeKey, nodeId) }
            scope.launch {
                runCatching { connectionTo(ep, routeKey, nodeId) }
                    .onFailure { Log.w(tag, "pre-dial of ${nodeId.take(8)} failed: ${it.message}") }
            }
            server.localPort
        }
    }

    private suspend fun bind(context: Context, relayUrl: String?): Endpoint {
        val relayKey = relayUrl.orEmpty()
        endpoints[relayKey]?.let { return it }
        val relayMode = relayUrl?.takeIf(String::isNotBlank)?.let {
            RelayMode.customFromUrls(listOf(it))
        }
        val ep = Endpoint.bind(
            EndpointOptions(
                preset = presetN0(),
                secretKey = secret(context, relayKey),
                alpns = listOf(alpn),
                relayMode = relayMode,
            ),
        )
        endpoints[relayKey] = ep
        return ep
    }

    private fun secret(context: Context, relayKey: String): ByteArray {
        val prefs = context.getSharedPreferences("aiterm.iroh", Context.MODE_PRIVATE)
        val base = prefs.getString("secret", null)?.let { encoded ->
            runCatching {
                SecretKey.fromBytes(Base64.decode(encoded, Base64.NO_WRAP)).toBytes()
            }.getOrNull()
        }
        val secret = base ?: SecretKey.generate().toBytes().also { fresh ->
            prefs.edit().putString("secret", Base64.encodeToString(fresh, Base64.NO_WRAP)).apply()
        }
        if (relayKey.isEmpty()) return secret
        return MessageDigest.getInstance("SHA-256").run {
            update(secret)
            digest(relayKey.toByteArray(Charsets.UTF_8))
        }
    }

    private suspend fun acceptLoop(server: ServerSocket, ep: Endpoint, routeKey: String, nodeId: String) {
        while (true) {
            val socket = runCatching { server.accept() }.getOrNull() ?: return
            scope.launch {
                runCatching { bridge(ep, routeKey, nodeId, socket) }
                    .onFailure {
                        Log.w(tag, "bridge to ${nodeId.take(8)} failed: ${it.message}")
                        runCatching { socket.close() }
                    }
            }
        }
    }

    private suspend fun connectionTo(ep: Endpoint, routeKey: String, nodeId: String): Connection {
        lock.withLock {
            connections[routeKey]?.let { connection ->
                if (connection.closeReason() == null) return connection
                connections.remove(routeKey)
            }
            val address = EndpointAddr(EndpointId.fromString(nodeId), null, emptyList())
            val connection = kotlinx.coroutines.withTimeout(20_000) { ep.connect(address, alpn) }
            Log.i(
                tag,
                "dialed ${nodeId.take(8)}: ${connection.paths().joinToString { if (it.isRelay) "relay" else "direct" }}",
            )
            connections[routeKey] = connection
            return connection
        }
    }

    private suspend fun bridge(ep: Endpoint, routeKey: String, nodeId: String, socket: Socket) {
        val stream = try {
            connectionTo(ep, routeKey, nodeId).openBi()
        } catch (error: Throwable) {
            lock.withLock { connections.remove(routeKey) }
            connectionTo(ep, routeKey, nodeId).openBi()
        }
        socket.tcpNoDelay = true
        val upload = scope.launch {
            val input = socket.getInputStream()
            val buffer = ByteArray(16 * 1024)
            try {
                while (true) {
                    val count = input.read(buffer)
                    if (count < 0) break
                    if (count > 0) stream.send().writeAll(buffer.copyOf(count))
                }
                stream.send().finish()
            } catch (_: Throwable) {
            }
        }
        val download = scope.launch {
            val output = socket.getOutputStream()
            try {
                while (true) {
                    val chunk = stream.recv().read(64u * 1024u)
                    if (chunk.isEmpty()) break
                    output.write(chunk)
                    output.flush()
                }
            } catch (_: Throwable) {
            } finally {
                runCatching { socket.close() }
            }
        }
        upload.join()
        download.join()
        runCatching { socket.close() }
    }
}
