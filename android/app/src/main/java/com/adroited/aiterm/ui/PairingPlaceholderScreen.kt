package com.adroited.aiterm.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.adroited.aiterm.R

/**
 * Stub destination for the pair action. Task 8 replaces it with the real
 * PairingScreen: CameraX preview, ML Kit `aiterm://pair` decode, desktop
 * fingerprint confirmation, and Keystore enrollment.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PairingPlaceholderScreen(onBack: () -> Unit) {
    Scaffold(
        topBar = { TopAppBar(title = { Text(stringResource(R.string.pairing_title)) }) },
    ) { innerPadding ->
        Column(
            modifier = Modifier.fillMaxSize().padding(innerPadding).padding(24.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text(text = stringResource(R.string.pairing_placeholder))
            TextButton(onClick = onBack) { Text(stringResource(R.string.action_back)) }
        }
    }
}
