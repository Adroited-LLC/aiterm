package com.fivelime.aiterm

import okhttp3.OkHttpClient
import java.security.MessageDigest
import java.security.SecureRandom
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import javax.net.ssl.SSLContext
import javax.net.ssl.X509TrustManager

/** The desktop's certificate is self-signed and the QR carried its SHA-256.
 *  That hash is the whole trust decision: the phone accepts exactly that
 *  certificate, on any address, and nothing signed by anyone. */
class PinnedCertTrustManager(private val fingerprintHex: String) : X509TrustManager {
    override fun checkServerTrusted(chain: Array<out X509Certificate>, authType: String) {
        val leaf = chain.firstOrNull() ?: throw CertificateException("no certificate presented")
        val seen = sha256Hex(leaf.encoded)
        if (!seen.equals(fingerprintHex, ignoreCase = true)) {
            throw CertificateException("This is not the desktop you paired with (certificate changed)")
        }
    }
    override fun checkClientTrusted(chain: Array<out X509Certificate>, authType: String) =
        throw CertificateException("not a server")
    override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()

    companion object {
        fun sha256Hex(bytes: ByteArray): String =
            MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
    }
}

/** An OkHttp client that trusts one certificate. Hostname checks are off on
 *  purpose: the pin is the identity, and the desktop is reached by whichever
 *  address happens to work. */
fun pinnedClient(fingerprintHex: String, builder: OkHttpClient.Builder): OkHttpClient {
    val tm = PinnedCertTrustManager(fingerprintHex)
    val ctx = SSLContext.getInstance("TLS")
    ctx.init(null, arrayOf(tm), SecureRandom())
    return builder
        .sslSocketFactory(ctx.socketFactory, tm)
        .hostnameVerifier { _, _ -> true }
        .build()
}
