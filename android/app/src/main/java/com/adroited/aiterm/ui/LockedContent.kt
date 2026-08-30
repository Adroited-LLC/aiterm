package com.adroited.aiterm.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp

/** Contains no paired metadata by design. */
@Composable
fun LockedContent(
    onUnlock: () -> Unit,
    modifier: Modifier = Modifier,
    error: String? = null,
) {
    Column(
        modifier = modifier.fillMaxSize().padding(32.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text("AITerm is locked", style = MaterialTheme.typography.headlineMedium)
        Text(
            "Unlock with a strong biometric or your device PIN.",
            textAlign = TextAlign.Center,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        error?.let {
            Text(
                it,
                textAlign = TextAlign.Center,
                color = MaterialTheme.colorScheme.error,
            )
        }
        Button(onClick = onUnlock) { Text("Unlock AITerm") }
    }
}
