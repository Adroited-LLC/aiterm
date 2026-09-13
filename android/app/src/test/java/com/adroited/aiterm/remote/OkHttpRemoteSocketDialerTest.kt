package com.adroited.aiterm.remote

import com.adroited.aiterm.pairing.PairedDesktop
import com.adroited.aiterm.security.SpkiFingerprint
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okhttp3.tls.HandshakeCertificates
import okhttp3.tls.HeldCertificate
import okio.ByteString
import okio.ByteString.Companion.toByteString
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

class OkHttpRemoteSocketDialerTest {
    @Test
    fun slowConsumerSurvivesBurstBeyondQueueCapacityWithoutLosingFrames() = burst(false)

    @Test
    fun closingWithAFullQueueUnblocksTheSocketReader() = burst(true)

    private fun burst(closeEarly: Boolean) = runBlocking {
        val certificate = HeldCertificate.Builder().addSubjectAlternativeName("localhost").ecdsa256().build()
        val tls = HandshakeCertificates.Builder().heldCertificate(certificate).build()
        MockWebServer().use { server ->
            server.useHttps(tls.sslSocketFactory())
            val sent = CountDownLatch(1)
            val acknowledged = CountDownLatch(1)
            val closed = CountDownLatch(1)
            val frames = (0 until 512).map { "frame-$it".toByteArray() }
            server.enqueue(MockResponse.Builder().webSocketUpgrade(object : WebSocketListener() {
                override fun onOpen(webSocket: WebSocket, response: Response) {
                    frames.forEach { check(webSocket.send(it.toByteString())) }
                    sent.countDown()
                }
                override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) { closed.countDown() }
                override fun onClosed(webSocket: WebSocket, code: Int, reason: String) { closed.countDown() }
                override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
                    if (bytes.utf8() == "ack") acknowledged.countDown()
                }
            }).build())
            server.start()
            val desktop = PairedDesktop("fixture", "Fixture", listOf("localhost"), server.port,
                SpkiFingerprint.of(certificate.certificate), null)
            val socket = OkHttpRemoteSocketDialer().open(desktop)
            try {
                assertTrue(sent.await(5, TimeUnit.SECONDS))
                // Let the callback fill its bounded queue before the app starts reading.
                delay(300)
                if (closeEarly) {
                    socket.close()
                    assertTrue("Socket reader must be released on close", closed.await(5, TimeUnit.SECONDS))
                } else {
                    withTimeout(10_000) { frames.forEach { assertArrayEquals(it, socket.receive()) } }
                    assertTrue(socket.send("ack".toByteArray()))
                    assertTrue(acknowledged.await(5, TimeUnit.SECONDS))
                }
            } finally { socket.close() }
        }
    }
}
