package com.adroited.aiterm.pairing

import kotlinx.coroutines.runBlocking
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import okhttp3.OkHttpClient
import okhttp3.tls.HandshakeCertificates
import okhttp3.tls.HeldCertificate
import org.junit.Assert.*
import org.junit.Test

class HttpRelayProvisionerTest {
    @Test fun provisionUsesHttpsAndSendsOnlyPublicCommitments() = runBlocking {
        val certificate = HeldCertificate.Builder().addSubjectAlternativeName("localhost").build()
        val serverTls = HandshakeCertificates.Builder().heldCertificate(certificate).build()
        val clientTls = HandshakeCertificates.Builder().addTrustedCertificate(certificate.certificate).build()
        MockWebServer().use { server ->
            server.useHttps(serverTls.sslSocketFactory())
            server.start()
            val client = OkHttpClient.Builder().sslSocketFactory(clientTls.sslSocketFactory(), clientTls.trustManager)
                .followRedirects(false).build()
            val provisioner = HttpRelayProvisioner(client)
            val bootstrap = RelayBootstrap(server.url("/").toString().removeSuffix("/"), "desktop-12345678", "ab".repeat(32))
            val pin = ByteArray(32) { 7 }.toBase64Url()
            server.enqueue(MockResponse(code = 200, body = "{}"))
            assertTrue(provisioner.provision(bootstrap, pin, byteArrayOf(2, 3), byteArrayOf(4, 5)))
            val request = server.takeRequest()
            assertEquals("/v1/provision", request.url.encodedPath)
            val body = request.body!!.utf8()
            assertTrue(body.contains("\"token_sha256\":\"${bootstrap.tokenHash}\""))
            assertTrue(body.contains("\"desktop_spki_sha256\":\"$pin\""))
            assertFalse(body.contains("\"token\":"))
            assertFalse(body.contains("secret"))
            server.enqueue(MockResponse(code = 409))
            assertTrue(provisioner.provision(bootstrap, pin, byteArrayOf(2, 3), byteArrayOf(4, 5)))
            server.enqueue(MockResponse(code = 429))
            assertFalse(provisioner.provision(bootstrap, pin, byteArrayOf(2, 3), byteArrayOf(4, 5)))
        }
    }
}
