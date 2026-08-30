package com.adroited.aiterm.remote

import com.adroited.aiterm.pairing.PairedDesktop
import com.adroited.aiterm.pairing.tls13Context
import com.adroited.aiterm.security.PinnedSpkiTrustManager
import java.security.SecureRandom
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.suspendCancellableCoroutine
import okhttp3.ConnectionSpec
import okhttp3.HttpUrl
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.TlsVersion
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okio.ByteString
import okio.ByteString.Companion.toByteString
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

/** Opens only TLS 1.3 WebSockets whose SPKI and hostname both match pairing. */
class OkHttpRemoteSocketDialer : RemoteSocketDialer {
    override suspend fun open(desktop: PairedDesktop): RemoteBinarySocket {
        require(desktop.hosts.isNotEmpty())
        val client = pinnedClient(desktop.serverSpkiFingerprint)
        var lastFailure: Exception? = null
        for (host in desktop.hosts) {
            try {
                return openEndpoint(client, endpoint(host, desktop.port))
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: Exception) {
                lastFailure = error
            }
        }
        throw RemoteProtocolException("no paired desktop address was reachable", lastFailure)
    }

    private fun pinnedClient(fingerprint: String): OkHttpClient {
        val trustManager = PinnedSpkiTrustManager(fingerprint)
        val sslContext = tls13Context().apply {
            init(null, arrayOf(trustManager), SecureRandom())
        }
        val tls13Only = ConnectionSpec.Builder(ConnectionSpec.RESTRICTED_TLS)
            .tlsVersions(TlsVersion.TLS_1_3)
            .build()
        // Intentionally do not install a custom HostnameVerifier. The pin
        // replaces CA path validation, never endpoint-name validation.
        return OkHttpClient.Builder()
            .connectTimeout(5, TimeUnit.SECONDS)
            .sslSocketFactory(sslContext.socketFactory, trustManager)
            .connectionSpecs(listOf(tls13Only))
            .build()
    }

    private fun endpoint(host: String, port: Int): HttpUrl = HttpUrl.Builder()
        .scheme("https")
        .host(host)
        .port(port)
        .addPathSegment("v1")
        .addPathSegment("ws")
        .build()

    private suspend fun openEndpoint(client: OkHttpClient, url: HttpUrl): RemoteBinarySocket =
        suspendCancellableCoroutine { continuation ->
            val incoming = Channel<ByteArray>(MAX_QUEUED_FRAMES)
            val reference = AtomicReference<WebSocket?>()
            val opened = AtomicReference<OkHttpBinarySocket?>()
            val listener = object : WebSocketListener() {
                override fun onOpen(webSocket: WebSocket, response: Response) {
                    reference.set(webSocket)
                    val socket = OkHttpBinarySocket(reference, incoming)
                    if (opened.compareAndSet(null, socket) && continuation.isActive) {
                        continuation.resume(socket)
                    }
                }

                override fun onMessage(webSocket: WebSocket, text: String) {
                    incoming.close(RemoteProtocolException("text remote frame received"))
                    webSocket.cancel()
                }

                override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
                    if (bytes.size <= 0 || bytes.size >= RemoteWireCodec.MAX_FRAME_BYTES ||
                        incoming.trySend(bytes.toByteArray()).isFailure
                    ) {
                        incoming.close(RemoteProtocolException("remote frame queue or size bound exceeded"))
                        webSocket.cancel()
                    }
                }

                override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                    incoming.close(RemoteProtocolException("desktop closed the remote connection"))
                    webSocket.cancel()
                }

                override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                    incoming.close(t)
                    if (continuation.isActive) continuation.resumeWithException(t)
                }
            }
            reference.set(client.newWebSocket(Request.Builder().url(url).build(), listener))
            continuation.invokeOnCancellation {
                reference.get()?.cancel()
                incoming.close()
            }
        }

    private class OkHttpBinarySocket(
        private val socket: AtomicReference<WebSocket?>,
        private val incoming: Channel<ByteArray>,
    ) : RemoteBinarySocket {
        override suspend fun receive(): ByteArray {
            val result = incoming.receiveCatching()
            return result.getOrNull()
                ?: throw RemoteProtocolException("remote socket closed", result.exceptionOrNull())
        }

        override fun send(bytes: ByteArray): Boolean =
            bytes.size in 1 until RemoteWireCodec.MAX_FRAME_BYTES &&
                socket.get()?.send(bytes.toByteString()) == true

        override fun close() {
            socket.getAndSet(null)?.cancel()
            incoming.close()
        }
    }

    private companion object {
        const val MAX_QUEUED_FRAMES = 64
    }
}
