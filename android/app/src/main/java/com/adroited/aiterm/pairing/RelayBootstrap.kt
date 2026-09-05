package com.adroited.aiterm.pairing

import java.nio.ByteBuffer
import java.security.MessageDigest
import java.util.Base64
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/** Public commitments from the QR. Neither the pairing secret nor connector token is sent here. */
data class RelayBootstrap(val controlOrigin: String, val routeId: String, val tokenHash: String) {
    fun digest(fingerprint: String): ByteArray {
        val hash = MessageDigest.getInstance("SHA-256")
        hash.update("aiterm-relay-enrollment-v1\u0000".toByteArray(Charsets.UTF_8))
        for (field in listOf(
            controlOrigin.toByteArray(Charsets.UTF_8), routeId.toByteArray(Charsets.UTF_8),
            tokenHash.chunked(2).map { it.toInt(16).toByte() }.toByteArray(),
            Base64.getUrlDecoder().decode(fingerprint),
        )) {
            hash.update(ByteBuffer.allocate(4).putInt(field.size).array())
            hash.update(field)
        }
        return hash.digest()
    }

    companion object {
        fun parse(origin: String, tokenHash: String, relayHost: String): RelayBootstrap? {
            val url = origin.toHttpUrlOrNull() ?: return null
            if (!url.isHttps || url.username.isNotEmpty() || url.password.isNotEmpty() ||
                url.encodedPath != "/" || url.query != null || url.fragment != null ||
                url.toString().removeSuffix("/") != origin ||
                !tokenHash.matches(Regex("[0-9a-f]{64}"))) return null
            val route = relayHost.substringBefore('.')
            if (!route.matches(Regex("[a-z0-9][a-z0-9-]{6,61}[a-z0-9]")) || '.' !in relayHost) return null
            return RelayBootstrap(origin, route, tokenHash)
        }
    }
}

fun interface RelayProvisioner {
    suspend fun provision(bootstrap: RelayBootstrap, fingerprint: String, publicKey: ByteArray, signature: ByteArray): Boolean
}

class HttpRelayProvisioner(
    private val client: OkHttpClient = OkHttpClient.Builder()
        .followRedirects(false).followSslRedirects(false)
        .callTimeout(10, TimeUnit.SECONDS).build(),
) : RelayProvisioner {

    override suspend fun provision(bootstrap: RelayBootstrap, fingerprint: String, publicKey: ByteArray, signature: ByteArray): Boolean =
        withContext(Dispatchers.IO) {
            val base64 = Base64.getUrlEncoder().withoutPadding()
            val body = buildJsonObject {
                put("route_id", bootstrap.routeId)
                put("token_sha256", bootstrap.tokenHash)
                put("desktop_spki_sha256", fingerprint)
                put("authority_public_key", base64.encodeToString(publicKey))
                put("signature_der", base64.encodeToString(signature))
            }.toString().toRequestBody("application/json".toMediaType())
            client.newCall(Request.Builder().url("${bootstrap.controlOrigin}/v1/provision").post(body).build())
                .execute().use { response ->
                    // A prior scan may already have registered this route. Connection still requires
                    // the exact desktop TLS pin and ordinary desktop pairing approval.
                    response.isSuccessful || response.code == 409
                }
        }
}
