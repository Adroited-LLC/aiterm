package com.adroited.aiterm.pairing

import kotlinx.serialization.ExperimentalSerializationApi
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.SerializationException
import kotlinx.serialization.cbor.ByteString
import kotlinx.serialization.cbor.Cbor

sealed interface PairingFrame

data class PairRequestFrame(
    val enrollmentSecret: ByteArray,
    val deviceName: String,
    val publicKey: ByteArray,
) : PairingFrame

data class PairPendingFrame(val requestId: String) : PairingFrame
data class PairApprovedFrame(val deviceId: String) : PairingFrame
class PairDeniedFrame : PairingFrame {
    override fun equals(other: Any?): Boolean = other is PairDeniedFrame
    override fun hashCode(): Int = javaClass.hashCode()
    override fun toString(): String = "PairDeniedFrame"
}

class PairingProtocolException(message: String, cause: Throwable? = null) : Exception(message, cause)

@OptIn(ExperimentalSerializationApi::class)
object PairingFrames {

    private const val MAX_PAIRING_FRAME_BYTES = 16 * 1_024

    private val cbor = Cbor {
        encodeDefaults = false
        ignoreUnknownKeys = false
        useDefiniteLengthEncoding = true
    }

    fun encode(frame: PairingFrame): ByteArray {
        val wire = when (frame) {
            is PairRequestFrame -> WireFrame(
                kind = "pair.request",
                enrollmentSecret = frame.enrollmentSecret,
                deviceName = frame.deviceName,
                publicKey = frame.publicKey,
            )
            is PairPendingFrame -> WireFrame(kind = "pair.pending", requestId = frame.requestId)
            is PairApprovedFrame -> WireFrame(kind = "pair.approved", deviceId = frame.deviceId)
            is PairDeniedFrame -> WireFrame(kind = "pair.denied")
        }
        return cbor.encodeToByteArray(WireFrame.serializer(), wire)
    }

    fun decode(bytes: ByteArray): PairingFrame {
        if (bytes.isEmpty() || bytes.size > MAX_PAIRING_FRAME_BYTES) {
            throw PairingProtocolException("invalid pairing frame size")
        }
        val wire = try {
            cbor.decodeFromByteArray(WireFrame.serializer(), bytes)
        } catch (error: SerializationException) {
            throw PairingProtocolException("malformed pairing frame", error)
        } catch (error: IllegalArgumentException) {
            throw PairingProtocolException("malformed pairing frame", error)
        }
        return wire.toFrame()
    }

    @Serializable
    private data class WireFrame(
        val kind: String,
        @SerialName("enrollment_secret") @ByteString
        val enrollmentSecret: ByteArray? = null,
        @SerialName("device_name")
        val deviceName: String? = null,
        @SerialName("public_key") @ByteString
        val publicKey: ByteArray? = null,
        @SerialName("request_id")
        val requestId: String? = null,
        @SerialName("device_id")
        val deviceId: String? = null,
    ) {
        fun toFrame(): PairingFrame = when (kind) {
            "pair.request" -> {
                requireFields("enrollment_secret", "device_name", "public_key")
                PairRequestFrame(
                    enrollmentSecret = enrollmentSecret!!,
                    deviceName = deviceName!!,
                    publicKey = publicKey!!,
                )
            }
            "pair.pending" -> {
                requireFields("request_id")
                PairPendingFrame(requestId!!)
            }
            "pair.approved" -> {
                requireFields("device_id")
                PairApprovedFrame(deviceId!!)
            }
            "pair.denied" -> {
                requireFields()
                PairDeniedFrame()
            }
            else -> throw PairingProtocolException("unknown pairing frame kind")
        }

        private fun requireFields(vararg required: String) {
            val populated = buildSet {
                if (enrollmentSecret != null) add("enrollment_secret")
                if (deviceName != null) add("device_name")
                if (publicKey != null) add("public_key")
                if (requestId != null) add("request_id")
                if (deviceId != null) add("device_id")
            }
            if (populated != required.toSet()) {
                throw PairingProtocolException("pairing frame has missing or unexpected fields")
            }
            when (kind) {
                "pair.pending" -> if (requestId.isNullOrBlank() || requestId.length > 128) invalidText()
                "pair.approved" -> if (deviceId.isNullOrBlank() || deviceId.length > 128) invalidText()
                "pair.request" -> if (
                    deviceName.isNullOrBlank() ||
                    deviceName.length > 128 ||
                    enrollmentSecret?.size != 32 ||
                    publicKey?.size != 33
                ) invalidText()
            }
        }

        private fun invalidText(): Nothing =
            throw PairingProtocolException("pairing frame contains an invalid field")
    }
}
