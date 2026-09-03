package com.fivelime.aiterm.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.Close
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.fivelime.aiterm.AppViewModel
import kotlinx.coroutines.delay

/** A plain shell on the desktop, driven from the phone: the same blank
 *  terminal the desktop's home launcher opens. The screen is text — the
 *  desktop renders the real thing — polled while this is on screen; the
 *  input row sends a line at a time, and the key strip sends the control
 *  characters a line cannot carry. */
@OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
@Composable
fun TerminalScreen(vm: AppViewModel, tab: String, outer: PaddingValues) {
    var line by remember { mutableStateOf("") }
    val scroll = rememberScrollState()

    // The screen lives while it is looked at: poll fast, and stop the moment
    // the tab is gone (a 404 clears vm.terminalTab and this screen with it).
    LaunchedEffect(tab) {
        while (vm.terminalTab == tab) {
            vm.pollTerminal()
            delay(700)
        }
    }
    // New output lands at the bottom, which is where a terminal is read.
    LaunchedEffect(vm.terminalLines) { scroll.scrollTo(scroll.maxValue) }

    val send = {
        if (line.isNotBlank() || line.isEmpty()) {
            vm.sendTerminal(line)
            line = ""
        }
    }

    Scaffold(
        modifier = Modifier.padding(outer).imePadding(),
        topBar = {
            TopAppBar(
                navigationIcon = {
                    IconButton(onClick = { vm.terminalTab = null }) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back — the shell keeps running on the desktop")
                    }
                },
                title = { Text(vm.terminalTitle) },
                actions = {
                    IconButton(onClick = { vm.closeTerminal() }) {
                        Icon(Icons.Filled.Close, "End the shell", tint = Muted)
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = Bg),
            )
        },
        containerColor = Bg,
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding)) {
            Column(
                Modifier
                    .weight(1f)
                    .fillMaxWidth()
                    .verticalScroll(scroll)
                    .padding(horizontal = 10.dp, vertical = 6.dp),
            ) {
                val text = vm.terminalLines.joinToString("\n").trimEnd('\n')
                Text(
                    if (text.isEmpty()) "…" else text,
                    fontFamily = FontFamily.Monospace,
                    fontSize = 12.sp,
                    lineHeight = 15.sp,
                    color = MaterialTheme.colorScheme.onSurface,
                    softWrap = false,
                    modifier = Modifier.horizontalScroll(rememberScrollState()),
                )
            }
            TerminalKeys { k -> vm.sendTerminal(k, enter = false) }
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                OutlinedTextField(
                    value = line,
                    onValueChange = { line = it },
                    modifier = Modifier.weight(1f),
                    placeholder = { Text("command", color = Muted) },
                    textStyle = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send),
                    keyboardActions = KeyboardActions(onSend = { send() }),
                )
                IconButton(onClick = send) { Icon(Icons.AutoMirrored.Filled.Send, "Run", tint = Accent) }
            }
        }
    }
}

@Composable
private fun TerminalKeys(onKey: (String) -> Unit) {
    val keys = listOf(
        "Ctrl+C" to "\u0003", "Tab" to "\t", "Esc" to "\u001B",
        "\u2191" to "\u001B[A", "\u2193" to "\u001B[B", "Ctrl+D" to "\u0004",
        "Ctrl+L" to "\u000C", "Ctrl+Z" to "\u001A",
    )
    Row(
        Modifier.fillMaxWidth()
            .horizontalScroll(rememberScrollState())
            .padding(horizontal = 8.dp, vertical = 2.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        keys.forEach { (label, seq) ->
            Box(
                Modifier.background(Accent.copy(alpha = 0.12f), RoundedCornerShape(12.dp))
                    .clickable { onKey(seq) }
                    .padding(horizontal = 12.dp, vertical = 6.dp),
            ) { Text(label, style = MaterialTheme.typography.labelMedium, color = Accent) }
        }
    }
}
