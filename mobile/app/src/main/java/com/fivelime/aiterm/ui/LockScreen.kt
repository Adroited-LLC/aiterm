package com.fivelime.aiterm.ui

import android.app.Activity
import android.os.Build
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Fingerprint
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import com.fivelime.aiterm.AppViewModel

/** The gate: nothing renders until the phone says it's you. Fingerprint
 *  first, the device PIN as fallback — the platform prompt handles both. */
@Composable
fun LockScreen(vm: AppViewModel, outer: PaddingValues) {
    val ctx = LocalContext.current
    fun prompt() {
        val activity = ctx as? Activity ?: return
        if (Build.VERSION.SDK_INT < 28) { vm.locked = false; return }
        val exec = activity.mainExecutor
        val b = android.hardware.biometrics.BiometricPrompt.Builder(activity)
            .setTitle("Unlock AITerm")
        // Face recognizes you and that's that — no extra Confirm tap.
        if (Build.VERSION.SDK_INT >= 29) b.setConfirmationRequired(false)
        if (Build.VERSION.SDK_INT >= 30) {
            // WEAK admits every enrolled biometric — face included — and
            // STRONG sensors satisfy it too; the PIN stays as fallback.
            b.setAllowedAuthenticators(
                android.hardware.biometrics.BiometricManager.Authenticators.BIOMETRIC_WEAK or
                    android.hardware.biometrics.BiometricManager.Authenticators.DEVICE_CREDENTIAL,
            )
        } else {
            @Suppress("DEPRECATION")
            b.setDeviceCredentialAllowed(true)
        }
        // A prompt that cannot start must never brick the app: catch,
        // surface, and leave the Unlock button to retry.
        runCatching {
            b.build().authenticate(
                android.os.CancellationSignal(),
                exec,
                object : android.hardware.biometrics.BiometricPrompt.AuthenticationCallback() {
                    override fun onAuthenticationSucceeded(result: android.hardware.biometrics.BiometricPrompt.AuthenticationResult?) {
                        vm.locked = false
                    }
                },
            )
        }.onFailure { vm.notice = "Can't show the unlock prompt: ${it.message}" }
    }
    LaunchedEffect(Unit) { prompt() }
    Box(Modifier.fillMaxSize().background(Bg).padding(outer), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Icon(Icons.Filled.Fingerprint, null, tint = Muted, modifier = Modifier.size(64.dp))
            Spacer(Modifier.height(16.dp))
            Text("AITerm is locked", style = MaterialTheme.typography.titleMedium)
            Spacer(Modifier.height(20.dp))
            Button(onClick = { prompt() }) { Text("Unlock") }
        }
    }
}
