package com.adroited.aiterm.ui

import android.app.Application
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.net.Uri
import android.provider.Settings
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import androidx.compose.foundation.layout.Column
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.core.content.FileProvider
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.compose.viewModel
import com.adroited.aiterm.BuildConfig
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.security.KeyStore
import java.security.MessageDigest
import java.util.UUID
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

private const val UPDATE_API = "https://api.github.com/repos/Adroited-LLC/aiterm-releases"
internal val updateJson = Json { ignoreUnknownKeys = true }
@Serializable internal data class AndroidUpdatePackage(val version: String, val versionCode: Int, val asset: String, val sha256: String, val size: Long, val notes: String = "")
// Decode only Android's entry: the other platforms intentionally have no versionCode.
@Serializable private data class ReleaseAsset(val id: Long, val name: String, val size: Long)
@Serializable private data class PrivateRelease(val assets: List<ReleaseAsset>)
internal fun validateAndroidUpdate(p: AndroidUpdatePackage) {
    require(Regex("[0-9]+\\.[0-9]+\\.[0-9]+").matches(p.version)) { "Invalid update version" }
    require(p.versionCode > 0 && p.asset.endsWith(".apk") && !p.asset.contains('/') && !p.asset.contains('\\')) { "Invalid update package" }
    require(p.size in 1..1024L * 1024 * 1024 && Regex("[a-fA-F0-9]{64}").matches(p.sha256)) { "Invalid update verification data" }
}
private class UpdateCredential(private val app: Application) {
    private val prefs = app.getSharedPreferences("private-updates", 0)
    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        return (store.getKey("aiterm-updates", null) as? SecretKey) ?: KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
            init(KeyGenParameterSpec.Builder("aiterm-updates", KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).build())
        }.generateKey()
    }
    fun read(): String? {
        val stored = prefs.getString("credential", null) ?: return null
        val bytes = Base64.decode(stored, Base64.NO_WRAP)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, bytes.copyOfRange(0, 12)))
        return cipher.doFinal(bytes.copyOfRange(12, bytes.size)).toString(Charsets.UTF_8)
    }
    fun write(token: String) {
        if (token.isEmpty()) { check(prefs.edit().remove("credential").commit()); return }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key())
        check(prefs.edit().putString("credential", Base64.encodeToString(cipher.iv + cipher.doFinal(token.toByteArray()), Base64.NO_WRAP)).commit())
    }
}
private fun connection(url: String, token: String, binary: Boolean): HttpURLConnection {
    var target = URL(url)
    repeat(6) {
        require(target.protocol == "https") { "Update requests require HTTPS" }
        val c = (target.openConnection() as HttpURLConnection).apply {
            connectTimeout = 15000; readTimeout = 60000; instanceFollowRedirects = false
            setRequestProperty("User-Agent", "AITerm updater")
            setRequestProperty("Accept", if (binary) "application/octet-stream" else "application/vnd.github+json")
            if (target.host == "api.github.com") setRequestProperty("Authorization", "Bearer $token")
        }
        if (c.responseCode in listOf(301, 302, 303, 307, 308)) {
            val next = c.getHeaderField("Location") ?: error("Invalid update redirect")
            c.disconnect(); target = URL(target, next)
        } else {
            if (c.responseCode != 200) { val code = c.responseCode; c.disconnect(); error("GitHub returned $code. Check your token and repository access.") }
            return c
        }
    }
    error("Too many update redirects")
}
private fun readUpdate(url: String, token: String, binary: Boolean, limit: Int): String {
    val c = connection(url, token, binary)
    try { return c.inputStream.use { input ->
        val output = java.io.ByteArrayOutputStream()
        val buffer = ByteArray(8192)
        while (true) { val n = input.read(buffer); if (n < 0) break; require(output.size() + n <= limit) { "Update response is too large" }; output.write(buffer, 0, n) }
        output.toString("UTF-8")
    } } finally { c.disconnect() }
}
private fun hex(bytes: ByteArray) = bytes.joinToString("") { "%02x".format(it) }
private suspend fun latest(token: String): Pair<AndroidUpdatePackage, Long>? = withContext(Dispatchers.IO) {
    val release = updateJson.decodeFromString<PrivateRelease>(readUpdate("$UPDATE_API/releases/latest", token, false, 1024 * 1024))
    val feed = release.assets.single { it.name == "updates.json" }
    val root = updateJson.parseToJsonElement(readUpdate("$UPDATE_API/releases/assets/${feed.id}", token, true, 256 * 1024))
    val manifest = root as kotlinx.serialization.json.JsonObject
    require(manifest["schema"].toString() == "1") { "Unsupported update feed" }
    val entry = (manifest["platforms"] as? kotlinx.serialization.json.JsonObject)?.get("android-arm64") ?: return@withContext null
    val p = updateJson.decodeFromJsonElement(AndroidUpdatePackage.serializer(), entry)
    validateAndroidUpdate(p)
    val asset = release.assets.single { it.name == p.asset }
    require(asset.size == p.size) { "Package size does not match the manifest" }
    if (p.versionCode > BuildConfig.VERSION_CODE) p to asset.id else null
}
internal data class UpdateUiState(val connected: Boolean = false, val busy: Boolean = false, val message: String = "", val available: AndroidUpdatePackage? = null)
internal class AppUpdateViewModel(application: Application) : AndroidViewModel(application) {
    private val credential = UpdateCredential(application)
    val state = MutableStateFlow(UpdateUiState())
    private var pending: Pair<File, AndroidUpdatePackage>? = null
    private fun run(action: suspend () -> Unit) {
        if (state.value.busy) return
        state.value = state.value.copy(busy = true, message = "")
        viewModelScope.launch {
            try { action() } catch (e: Exception) { state.value = state.value.copy(message = e.message ?: "Update failed. Try again.") }
            finally { state.value = state.value.copy(busy = false) }
        }
    }
    fun check() = run {
        val token = withContext(Dispatchers.IO) { credential.read() }
        val found = token?.let { latest(it) }?.first
        state.value = state.value.copy(connected = token != null, available = found,
            message = if (token == null) "Connect your update access first." else if (found == null) "You’re up to date." else "Version ${found.version} is available.")
    }
    fun connect(token: String) = run {
        withContext(Dispatchers.IO) {
            if (token.isNotBlank()) readUpdate(UPDATE_API, token.trim(), false, 256 * 1024)
            credential.write(token.trim())
        }
        state.value = UpdateUiState(connected = token.isNotBlank(), busy = true, message = if (token.isBlank()) "Disconnected." else "Connected. Check for updates to continue.")
    }
    fun install() = run {
        val token = withContext(Dispatchers.IO) { credential.read() } ?: error("Connect your update access first")
        val (p, id) = latest(token) ?: error("No update available")
        val app = getApplication<Application>()
        val file = withContext(Dispatchers.IO) {
            val dir = File(app.cacheDir, "updates").apply { mkdirs() }
            val file = File(dir, "aiterm-${UUID.randomUUID()}.apk")
            val c = connection("$UPDATE_API/releases/assets/$id", token, true)
            try {
                val hash = MessageDigest.getInstance("SHA-256"); var size = 0L
                c.inputStream.use { input -> file.outputStream().use { output ->
                    val buffer = ByteArray(65536)
                    while (true) { val n = input.read(buffer); if (n < 0) break; size += n; require(size <= p.size) { "Update download exceeds expected size" }; hash.update(buffer, 0, n); output.write(buffer, 0, n) }
                } }
                require(size == p.size && hex(hash.digest()).equals(p.sha256, true)) { "Update verification failed" }
                validateApk(file, p)
                file
            } catch (e: Exception) { file.delete(); throw e } finally { c.disconnect() }
        }
        pending = file to p
        requestInstall()
    }
    @Suppress("DEPRECATION")
    private fun validateApk(file: File, p: AndroidUpdatePackage) {
        val app = getApplication<Application>(); val pm = app.packageManager
        val flags = if (Build.VERSION.SDK_INT >= 28) PackageManager.GET_SIGNING_CERTIFICATES else PackageManager.GET_SIGNATURES
        val downloaded = pm.getPackageArchiveInfo(file.path, flags) ?: error("Invalid APK")
        val installed = pm.getPackageInfo(app.packageName, flags)
        require(downloaded.packageName == app.packageName && (if (Build.VERSION.SDK_INT >= 28) downloaded.longVersionCode else downloaded.versionCode.toLong()) == p.versionCode.toLong() && downloaded.versionName == p.version) { "APK identity or version does not match" }
        val expected = (if (Build.VERSION.SDK_INT >= 28) installed.signingInfo?.apkContentsSigners else installed.signatures)?.map { hex(it.toByteArray()) }?.toSet()
        require(!expected.isNullOrEmpty() && (if (Build.VERSION.SDK_INT >= 28) downloaded.signingInfo?.apkContentsSigners else downloaded.signatures)?.map { hex(it.toByteArray()) }?.toSet() == expected) { "APK signing identity does not match this app" }
    }
    fun resumeInstall() { if (pending != null && getApplication<Application>().packageManager.canRequestPackageInstalls()) run { requestInstall() } }
    private fun requestInstall() {
        val app = getApplication<Application>(); val (file, p) = pending ?: return
        if (!app.packageManager.canRequestPackageInstalls()) {
            state.value = state.value.copy(message = "Allow AITerm to install updates, then return here.")
            app.startActivity(Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${app.packageName}")).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            return
        }
        validateApk(file, p)
        val uri = FileProvider.getUriForFile(app, "${app.packageName}.terminal-images", file)
        app.startActivity(Intent(Intent.ACTION_VIEW).setDataAndType(uri, "application/vnd.android.package-archive")
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_GRANT_READ_URI_PERMISSION))
        pending = null
        state.value = state.value.copy(message = "Continue in the Android installer.")
    }
}
private val showUpdates = MutableStateFlow(false)
@Composable internal fun AppUpdateButton() { TextButton(onClick = { showUpdates.value = true }) { Text("App updates") } }
@Composable internal fun AppUpdateHost() {
    val model: AppUpdateViewModel = viewModel()
    val state by model.state.collectAsStateWithLifecycle()
    val show by showUpdates.collectAsStateWithLifecycle()
    var token by remember { mutableStateOf("") }
    var dismissed by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) { while (true) { model.check(); delay(6 * 60 * 60 * 1000L) } }
    LifecycleEventEffect(Lifecycle.Event.ON_RESUME) { model.resumeInstall() }
    if (show || (state.available != null && dismissed != state.available?.version)) {
        AlertDialog(onDismissRequest = { if (!state.busy) { dismissed = state.available?.version; showUpdates.value = false } },
            title = { Text("App updates") },
            text = { Column {
                Text("Installed version ${BuildConfig.VERSION_NAME}")
                Text("Private updates from Adroited-LLC/aiterm-releases. Use a GitHub token with Contents: read access to this repository.")
                OutlinedTextField(value = token, onValueChange = { token = it }, label = { Text("GitHub access token") }, visualTransformation = PasswordVisualTransformation(), singleLine = true)
                TextButton(enabled = !state.busy && token.isNotBlank(), onClick = { model.connect(token); token = "" }) { Text("Connect") }
                if (state.connected) TextButton(enabled = !state.busy, onClick = { model.connect("") }) { Text("Disconnect") }
                Text(if (state.busy) "Checking or downloading…" else state.message)
                if (state.available != null) Button(enabled = !state.busy, onClick = model::install) { Text("Download and install ${state.available?.version}") }
            } },
            confirmButton = { TextButton(enabled = !state.busy, onClick = model::check) { Text("Check for updates") } },
            dismissButton = { TextButton(enabled = !state.busy, onClick = { dismissed = state.available?.version; showUpdates.value = false }) { Text("Close") } })
    }
}
