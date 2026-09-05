package com.adroited.aiterm.pairing

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

@Serializable
enum class RemoteNetworkStack {
    @SerialName("aiterm") AITERM,
    @SerialName("iroh") IROH,
}

/**
 * A desktop this phone has already enrolled with.
 *
 * Only non-secret metadata lives here. The device private key and the desktop
 * credential stay in Android Keystore (Task 8); [serverSpkiFingerprint] is the
 * pinned SHA-256 SPKI fingerprint from the pairing QR and is safe to display.
 */
@Serializable
data class PairedDesktop(
    @SerialName("device_id")
    val deviceId: String,
    @SerialName("display_name")
    val displayName: String,
    val hosts: List<String>,
    val port: Int,
    @SerialName("server_spki_fingerprint")
    val serverSpkiFingerprint: String,
    @SerialName("last_seen_epoch_millis")
    val lastSeenEpochMillis: Long?,
    @SerialName("relay_host")
    val relayHost: String? = null,
    @SerialName("relay_port")
    val relayPort: Int? = null,
    @SerialName("network_stack")
    val networkStack: RemoteNetworkStack = RemoteNetworkStack.AITERM,
    @SerialName("iroh_node_id")
    val irohNodeId: String? = null,
    @SerialName("iroh_relay_url")
    val irohRelayUrl: String? = null,
    @SerialName("friendly_name")
    val friendlyName: String? = null,
) {
    val label: String get() = friendlyName ?: displayName
}
