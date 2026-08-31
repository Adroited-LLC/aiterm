package com.fivelime.aiterm.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.QrCodeScanner
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import com.fivelime.aiterm.AppViewModel
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.codescanner.GmsBarcodeScannerOptions
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning

@Composable
fun PairScreen(vm: AppViewModel, padding: PaddingValues) {
    val ctx = LocalContext.current
    var pasted by remember { mutableStateOf("") }
    var showPaste by remember { mutableStateOf(false) }

    Column(
        modifier = Modifier.fillMaxSize().padding(padding).safeDrawingPadding().padding(horizontal = 28.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        val adding = vm.desktop != null
        Text(if (adding) "Add a desktop" else "AITerm", style = MaterialTheme.typography.displaySmall)
        Spacer(Modifier.height(8.dp))
        Text(
            if (adding) "Pairing another desktop keeps the ones you have.\nOn that desktop: Settings → Remote → Show QR."
            else "Your desktop sessions, from your phone.\nOn the desktop: Settings → Remote → Show QR.",
            style = MaterialTheme.typography.bodyLarge, color = Muted, textAlign = TextAlign.Center,
        )
        Spacer(Modifier.height(32.dp))
        if (vm.pairing) {
            CircularProgressIndicator()
            Spacer(Modifier.height(12.dp))
            Text("Reaching the desktop…", color = Muted)
        } else {
            Button(onClick = {
                val options = GmsBarcodeScannerOptions.Builder().setBarcodeFormats(Barcode.FORMAT_QR_CODE).build()
                GmsBarcodeScanning.getClient(ctx, options).startScan()
                    .addOnSuccessListener { code -> code.rawValue?.let(vm::pair) }
                    .addOnFailureListener { e -> vm.notice = "Scanner unavailable: ${e.message}" }
            }) {
                Icon(Icons.Filled.QrCodeScanner, contentDescription = null)
                Spacer(Modifier.height(0.dp))
                Text("  Scan the QR")
            }
            Spacer(Modifier.height(16.dp))
            if (showPaste) {
                OutlinedTextField(
                    value = pasted, onValueChange = { pasted = it },
                    label = { Text("aiterm://pair?…") }, singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                TextButton(onClick = { vm.pair(pasted) }, enabled = pasted.isNotBlank()) { Text("Pair") }
            } else {
                TextButton(onClick = { showPaste = true }) { Text("Paste a pairing link instead") }
            }
            if (adding) {
                Spacer(Modifier.height(8.dp))
                TextButton(onClick = { vm.showPair = false }) { Text("Cancel") }
            }
        }
    }
}
