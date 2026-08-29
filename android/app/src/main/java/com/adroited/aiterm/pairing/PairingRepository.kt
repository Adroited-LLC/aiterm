package com.adroited.aiterm.pairing

import android.os.Build
import com.adroited.aiterm.security.DeviceKeyException
import com.adroited.aiterm.security.DeviceKeys
import com.adroited.aiterm.security.PinnedSpkiTrustManager
import com.adroited.aiterm.security.SpkiPinMismatchException
import java.security.SecureRandom
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLPeerUnverifiedException
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withTimeoutOrNull
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
import org.conscrypt.Conscrypt
import kotlin.coroutines.resume

data class PairingEndpoint(val host: String, val port: Int)

sealed interface EnrollmentOutcome {
    data class Approved(val deviceId: String) : EnrollmentOutcome
    data object Denied : EnrollmentOutcome
    data object Expired : EnrollmentOutcome
    data object ConsumedPayload : EnrollmentOutcome
    data object FingerprintMismatch : EnrollmentOutcome
    data object TlsIdentityMismatch : EnrollmentOutcome
    data class Unreachable(val detail: String) : EnrollmentOutcome
    data class Indeterminate(val detail: String) : EnrollmentOutcome
    data class ProtocolFailure(val detail: String) : EnrollmentOutcome
}

interface PairingTransport {
    suspend fun enroll(
        endpoint: PairingEndpoint,
        serverSpkiFingerprint: String,
        enrollmentSecret: EnrollmentSecret,
        deviceName: String,
        devicePublicKey: ByteArray,
        onPending: () -> Unit,
    ): EnrollmentOutcome
}

sealed interface PairingResult {
    data class Paired(val desktop: PairedDesktop) : PairingResult
    data class Rejected(val failure: PairingFailure) : PairingResult
}

class PairingRepository(
    private val transport: PairingTransport,
    private val deviceKeys: DeviceKeys,
    private val store: PairedDesktopStore,
) {

    suspend fun pair(
        payload: PairingPayload,
        deviceName: String,
        nowEpochMillis: Long,
        onAwaitingApproval: () -> Unit = {},
    ): PairingResult {
        if (payload.isExpired(nowEpochMillis)) {
            payload.discard()
            return PairingResult.Rejected(PairingFailure.EXPIRED_PAYLOAD)
        }
        if (
            deviceName.isBlank() ||
            deviceName.length > 128 ||
            deviceName.any(Char::isISOControl)
        ) {
            payload.discard()
            return PairingResult.Rejected(PairingFailure.PROTOCOL_ERROR)
        }

        if (!payload.secret.isAvailable()) {
            return PairingResult.Rejected(PairingFailure.CONSUMED_PAYLOAD)
        }
        val publicKey = try {
            deviceKeys.devicePublicKey()
        } catch (_: DeviceKeyException) {
            payload.discard()
            return PairingResult.Rejected(PairingFailure.KEY_UNAVAILABLE)
        }

        return try {
            pairUnclaimed(
                payload = payload,
                enrollmentSecret = payload.secret,
                deviceName = deviceName,
                devicePublicKey = publicKey,
                onAwaitingApproval = onAwaitingApproval,
            )
        } finally {
            // A scan is one attempt. If no candidate reached a valid opening
            // challenge, erase the still-unclaimed bytes when that attempt ends.
            payload.discard()
        }
    }

    private suspend fun pairUnclaimed(
        payload: PairingPayload,
        enrollmentSecret: EnrollmentSecret,
        deviceName: String,
        devicePublicKey: ByteArray,
        onAwaitingApproval: () -> Unit,
    ): PairingResult {
        for (host in payload.hosts) {
            val outcome = try {
                transport.enroll(
                    endpoint = PairingEndpoint(host, payload.port),
                    serverSpkiFingerprint = payload.serverSpkiFingerprint,
                    enrollmentSecret = enrollmentSecret,
                    deviceName = deviceName,
                    devicePublicKey = devicePublicKey,
                    onPending = onAwaitingApproval,
                )
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: Exception) {
                EnrollmentOutcome.Indeterminate("pairing transport failed")
            }

            when (outcome) {
                is EnrollmentOutcome.Approved -> {
                    if (
                        outcome.deviceId.isBlank() ||
                        outcome.deviceId.length > 128 ||
                        outcome.deviceId.any(Char::isISOControl)
                    ) {
                        return PairingResult.Rejected(PairingFailure.PROTOCOL_ERROR)
                    }
                    val desktop = PairedDesktop(
                        deviceId = outcome.deviceId,
                        displayName = payload.desktopName,
                        hosts = listOf(host) + payload.hosts.filterNot { it == host },
                        port = payload.port,
                        serverSpkiFingerprint = payload.serverSpkiFingerprint,
                        lastSeenEpochMillis = null,
                    )
                    try {
                        store.save(desktop)
                    } catch (_: PairedDesktopStoreException) {
                        return PairingResult.Rejected(PairingFailure.STORAGE_FAILURE)
                    }
                    return PairingResult.Paired(desktop)
                }
                EnrollmentOutcome.Denied ->
                    return PairingResult.Rejected(PairingFailure.DENIED_BY_DESKTOP)
                EnrollmentOutcome.Expired ->
                    return PairingResult.Rejected(PairingFailure.EXPIRED_PAYLOAD)
                EnrollmentOutcome.ConsumedPayload ->
                    return PairingResult.Rejected(PairingFailure.CONSUMED_PAYLOAD)
                EnrollmentOutcome.FingerprintMismatch ->
                    return PairingResult.Rejected(PairingFailure.FINGERPRINT_MISMATCH)
                EnrollmentOutcome.TlsIdentityMismatch ->
                    return PairingResult.Rejected(PairingFailure.TLS_IDENTITY_MISMATCH)
                is EnrollmentOutcome.Indeterminate ->
                    return PairingResult.Rejected(PairingFailure.ENROLLMENT_STATE_UNKNOWN)
                is EnrollmentOutcome.ProtocolFailure ->
                    return PairingResult.Rejected(PairingFailure.PROTOCOL_ERROR)
                is EnrollmentOutcome.Unreachable -> Unit
            }
        }
        return PairingResult.Rejected(PairingFailure.UNREACHABLE)
    }
}

class OkHttpPairingTransport : PairingTransport {

    private val baseClient = OkHttpClient.Builder()
        .connectTimeout(5, TimeUnit.SECONDS)
        .build()

    override suspend fun enroll(
        endpoint: PairingEndpoint,
        serverSpkiFingerprint: String,
        enrollmentSecret: EnrollmentSecret,
        deviceName: String,
        devicePublicKey: ByteArray,
        onPending: () -> Unit,
    ): EnrollmentOutcome {
        val pinnedClient = try {
            pinnedClient(serverSpkiFingerprint)
        } catch (_: Exception) {
            return EnrollmentOutcome.Unreachable("TLS 1.3 is unavailable")
        }
        val request = try {
            Request.Builder().url(webSocketUrl(endpoint)).build()
        } catch (_: IllegalArgumentException) {
            return EnrollmentOutcome.Unreachable("invalid candidate endpoint")
        }

        val outcome = withTimeoutOrNull(APPROVAL_TIMEOUT_MILLIS) {
            awaitEnrollment(
                client = pinnedClient.client,
                pinMismatchObserved = pinnedClient.trustManager::didObserveMismatch,
                request = request,
                enrollmentSecret = enrollmentSecret,
                deviceName = deviceName,
                devicePublicKey = devicePublicKey,
                onPending = onPending,
            )
        }
        return outcome ?: if (enrollmentSecret.isAvailable()) {
            EnrollmentOutcome.Unreachable("opening challenge timed out")
        } else {
            EnrollmentOutcome.Indeterminate("approval timed out")
        }
    }

    private fun pinnedClient(fingerprint: String): PinnedClient {
        val trustManager = PinnedSpkiTrustManager(fingerprint)
        val sslContext = tls13Context().apply {
            init(null, arrayOf(trustManager), SecureRandom())
        }
        val tls13Only = ConnectionSpec.Builder(ConnectionSpec.RESTRICTED_TLS)
            .tlsVersions(TlsVersion.TLS_1_3)
            .build()
        // Deliberately retain OkHttp's default hostname verifier. The SPKI pin
        // replaces CA path validation, not endpoint-name validation.
        val client = baseClient.newBuilder()
            .sslSocketFactory(sslContext.socketFactory, trustManager)
            .connectionSpecs(listOf(tls13Only))
            .build()
        return PinnedClient(client, trustManager)
    }

    private fun webSocketUrl(endpoint: PairingEndpoint): HttpUrl =
        HttpUrl.Builder()
            .scheme("https")
            .host(endpoint.host)
            .port(endpoint.port)
            .addPathSegment("v1")
            .addPathSegment("ws")
            .build()

    private suspend fun awaitEnrollment(
        client: OkHttpClient,
        pinMismatchObserved: () -> Boolean,
        request: Request,
        enrollmentSecret: EnrollmentSecret,
        deviceName: String,
        devicePublicKey: ByteArray,
        onPending: () -> Unit,
    ): EnrollmentOutcome = suspendCancellableCoroutine { continuation ->
        val completed = AtomicBoolean(false)
        val secretSent = AtomicBoolean(false)
        val state = AtomicReference(State.WAITING_FOR_OPEN)
        val socket = AtomicReference<WebSocket?>(null)

        fun finish(outcome: EnrollmentOutcome, closeSocket: Boolean = true) {
            if (completed.compareAndSet(false, true)) {
                state.set(State.FINISHED)
                if (closeSocket) socket.get()?.cancel()
                if (continuation.isActive) continuation.resume(outcome)
            }
        }

        val listener = object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                socket.compareAndSet(null, webSocket)
                if (!state.compareAndSet(State.WAITING_FOR_OPEN, State.WAITING_FOR_CHALLENGE)) {
                    finish(EnrollmentOutcome.ProtocolFailure("duplicate open callback"))
                }
            }

            override fun onMessage(webSocket: WebSocket, text: String) {
                finish(EnrollmentOutcome.ProtocolFailure("text pairing frame received"))
            }

            override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
                socket.compareAndSet(null, webSocket)
                if (completed.get()) return
                val frame = try {
                    PairingFrames.decode(bytes.toByteArray())
                } catch (_: PairingProtocolException) {
                    finish(EnrollmentOutcome.ProtocolFailure("malformed pairing response"))
                    return
                }
                when (state.get()) {
                    State.WAITING_FOR_CHALLENGE -> when (frame) {
                        is AuthChallengeFrame -> {
                            // The Rust gateway's nonce is only signed by an
                            // already-enrolled auth.proof. A pair.request does
                            // not echo or sign it; validation gates secret use.
                            val sendFailure = when (
                                val consumption = enrollmentSecret.consume { secret ->
                                    val encoded = try {
                                        PairingFrames.encode(
                                            PairRequestFrame(
                                                enrollmentSecret = secret,
                                                deviceName = deviceName,
                                                publicKey = devicePublicKey,
                                            ),
                                        )
                                    } catch (_: Exception) {
                                        return@consume EnrollmentOutcome.ProtocolFailure(
                                            "request encoding failed",
                                        )
                                    }
                                    val message = encoded.toByteString()
                                    encoded.fill(0)
                                    state.set(State.WAITING_FOR_PENDING)
                                    // Conservatively classify any teardown
                                    // from this point as indeterminate: send()
                                    // may have queued enrollment material.
                                    secretSent.set(true)
                                    val sent = runCatching { webSocket.send(message) }
                                        .getOrDefault(false)
                                    if (sent) {
                                        null
                                    } else {
                                        EnrollmentOutcome.Indeterminate("enrollment send failed")
                                    }
                                }
                            ) {
                                is EnrollmentSecret.Consumption.Used -> consumption.value
                                EnrollmentSecret.Consumption.AlreadyConsumed ->
                                    EnrollmentOutcome.ConsumedPayload
                            }
                            if (sendFailure != null) finish(sendFailure)
                        }
                        else -> finish(
                            EnrollmentOutcome.ProtocolFailure("opening challenge required"),
                        )
                    }
                    State.WAITING_FOR_PENDING -> when (frame) {
                        is PairPendingFrame -> {
                            state.set(State.WAITING_FOR_DECISION)
                            runCatching(onPending)
                        }
                        else -> finish(EnrollmentOutcome.ProtocolFailure("pending response required"))
                    }
                    State.WAITING_FOR_DECISION -> when (frame) {
                        is PairApprovedFrame -> finish(EnrollmentOutcome.Approved(frame.deviceId))
                        is PairDeniedFrame -> finish(EnrollmentOutcome.Denied)
                        is PairExpiredFrame -> finish(EnrollmentOutcome.Expired)
                        else -> finish(EnrollmentOutcome.ProtocolFailure("decision response required"))
                    }
                    State.WAITING_FOR_OPEN ->
                        finish(EnrollmentOutcome.ProtocolFailure("response arrived before open"))
                    State.FINISHED -> Unit
                }
            }

            override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                state.set(State.FINISHED)
                finish(disconnectOutcome(secretSent.get()), closeSocket = false)
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                state.set(State.FINISHED)
                finish(
                    failureOutcome(t, secretSent.get(), pinMismatchObserved()),
                    closeSocket = false,
                )
            }
        }

        val webSocket = client.newWebSocket(request, listener)
        socket.compareAndSet(null, webSocket)
        if (completed.get()) webSocket.cancel()
        continuation.invokeOnCancellation { webSocket.cancel() }
    }

    private fun failureOutcome(
        error: Throwable,
        secretSent: Boolean,
        pinMismatchObserved: Boolean,
    ): EnrollmentOutcome {
        if (pinMismatchObserved || error.causedBy<SpkiPinMismatchException>()) {
            return EnrollmentOutcome.FingerprintMismatch
        }
        if (error.causedBy<SSLPeerUnverifiedException>()) {
            return EnrollmentOutcome.TlsIdentityMismatch
        }
        return disconnectOutcome(secretSent)
    }

    private fun disconnectOutcome(secretSent: Boolean): EnrollmentOutcome =
        if (secretSent) {
            EnrollmentOutcome.Indeterminate("connection ended after enrollment was sent")
        } else {
            EnrollmentOutcome.Unreachable("connection ended before enrollment was sent")
        }

    private inline fun <reified T : Throwable> Throwable.causedBy(): Boolean {
        var current: Throwable? = this
        while (current != null) {
            if (current is T) return true
            current = current.cause
        }
        return false
    }

    private enum class State {
        WAITING_FOR_OPEN,
        WAITING_FOR_CHALLENGE,
        WAITING_FOR_PENDING,
        WAITING_FOR_DECISION,
        FINISHED,
    }

    private data class PinnedClient(
        val client: OkHttpClient,
        val trustManager: PinnedSpkiTrustManager,
    )

    companion object {
        // Rust starts its 300-second approval timer only after TLS, the opening
        // challenge, and pair.request. Keep a small transport grace period so
        // its terminal pair.expired frame wins the boundary race.
        private const val APPROVAL_TIMEOUT_MILLIS = (5 * 60 + 15) * 1_000L
    }
}

private fun tls13Context(): SSLContext {
    val useBundledProvider = shouldUseBundledTls13Provider(
        isAndroidRuntime = isAndroidRuntime(),
        sdkInt = Build.VERSION.SDK_INT,
    )
    return if (useBundledProvider) {
        SSLContext.getInstance("TLSv1.3", Conscrypt.newProvider())
    } else {
        SSLContext.getInstance("TLSv1.3")
    }
}

internal fun shouldUseBundledTls13Provider(isAndroidRuntime: Boolean, sdkInt: Int): Boolean =
    isAndroidRuntime && sdkInt in 26 until 29

private fun isAndroidRuntime(): Boolean =
    System.getProperty("java.runtime.name") == "Android Runtime" ||
        System.getProperty("java.vm.name") == "Dalvik"
